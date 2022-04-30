use crate::error::SamplingError;
use crate::kernel_error::{IntoResult, KernelError};
use crate::proc_maps::{DyldInfo, DyldInfoManager, Modification, StackwalkerRef, VmSubData};
use crate::thread_profiler::ThreadProfiler;
use framehop::{
    CacheNative, MayAllocateDuringUnwind, Module, ModuleUnwindData, TextByteData, Unwinder,
    UnwinderNative,
};
use gecko_profile::debugid::DebugId;
use mach::mach_types::thread_act_port_array_t;
use mach::mach_types::thread_act_t;
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::task::task_threads;
use mach::traps::mach_task_self;
use mach::vm::mach_vm_deallocate;
use mach::vm_types::{mach_vm_address_t, mach_vm_size_t};
use object::{CompressedFileRange, CompressionFormat, Object, ObjectSection};
use profiler_get_symbols::DebugIdExt;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::Deref;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use gecko_profile::ProfileBuilder;

pub enum UnwindSectionBytes {
    Remapped(VmSubData),
    Mmap(MmapSubData),
    Allocated(Vec<u8>),
}

impl Deref for UnwindSectionBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            UnwindSectionBytes::Remapped(vm_sub_data) => vm_sub_data.deref(),
            UnwindSectionBytes::Mmap(mmap_sub_data) => mmap_sub_data.deref(),
            UnwindSectionBytes::Allocated(vec) => vec.deref(),
        }
    }
}

pub struct MmapSubData {
    mmap: memmap2::Mmap,
    offset: usize,
    len: usize,
}

impl MmapSubData {
    pub fn try_new(mmap: memmap2::Mmap, offset: usize, len: usize) -> Option<Self> {
        let end_addr = offset.checked_add(len)?;
        if end_addr <= mmap.len() {
            Some(Self { mmap, offset, len })
        } else {
            None
        }
    }
}

impl Deref for MmapSubData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.mmap.deref()[self.offset..][..self.len]
    }
}

pub type UnwinderCache = CacheNative<UnwindSectionBytes, MayAllocateDuringUnwind>;

pub struct TaskProfiler {
    task: mach_port_t,
    pid: u32,
    interval: Duration,
    start_time: Instant,
    start_time_system: SystemTime,
    end_time: Option<Instant>,
    live_threads: HashMap<thread_act_t, ThreadProfiler>,
    dead_threads: Vec<ThreadProfiler>,
    lib_info_manager: DyldInfoManager,
    libs: Vec<DyldInfo>,
    executable_lib: Option<DyldInfo>,
    command_name: String,
    ignored_errors: Vec<SamplingError>,
    unwinder: UnwinderNative<UnwindSectionBytes, MayAllocateDuringUnwind>,
}

impl TaskProfiler {
    pub fn new(
        task: mach_port_t,
        pid: u32,
        now: Instant,
        now_system: SystemTime,
        command_name: &str,
        interval: Duration,
    ) -> Option<Self> {
        let thread_acts = get_thread_list(task).ok()?;
        let mut live_threads = HashMap::new();
        for (i, thread_act) in thread_acts.into_iter().enumerate() {
            // Pretend that the first thread is the main thread. Might not be true.
            let is_main = i == 0;
            if let Some(thread) = ThreadProfiler::new(task, pid, thread_act, now, is_main) {
                live_threads.insert(thread_act, thread);
            }
        }
        Some(TaskProfiler {
            task,
            pid,
            interval,
            start_time: now,
            start_time_system: now_system,
            end_time: None,
            live_threads,
            dead_threads: Vec::new(),
            lib_info_manager: DyldInfoManager::new(task),
            libs: Vec::new(),
            command_name: command_name.to_owned(),
            executable_lib: None,
            ignored_errors: Vec::new(),
            unwinder: UnwinderNative::new(),
        })
    }

    pub fn sample(
        &mut self,
        now: Instant,
        unwinder_cache: &mut UnwinderCache,
    ) -> Result<bool, SamplingError> {
        let result = self.sample_impl(now, unwinder_cache);
        match result {
            Ok(()) => Ok(true),
            Err(SamplingError::ProcessTerminated(_, _)) => Ok(false),
            Err(err @ SamplingError::Ignorable(_, _)) => {
                self.ignored_errors.push(err);
                if self.ignored_errors.len() >= 10 {
                    println!(
                        "Treating process \"{}\" [pid: {}] as terminated after 10 unknown errors:",
                        self.command_name, self.pid
                    );
                    println!("{:#?}", self.ignored_errors);
                    Ok(false)
                } else {
                    // Pretend that sampling worked and that the thread is still alive.
                    Ok(true)
                }
            }
            Err(err) => Err(err),
        }
    }

    fn sample_impl(
        &mut self,
        now: Instant,
        unwinder_cache: &mut UnwinderCache,
    ) -> Result<(), SamplingError> {
        // First, check for any newly-loaded libraries.
        let changes = self
            .lib_info_manager
            .check_for_changes()
            .unwrap_or_else(|_| Vec::new());
        for change in changes {
            match change {
                Modification::Added(mut lib) => {
                    self.add_lib_to_unwinder_and_ensure_debug_id(&mut lib);
                    if self.executable_lib.is_none() && lib.is_executable {
                        self.executable_lib = Some(lib.clone());
                    }
                    self.libs.push(lib)
                }
                Modification::Removed(_) => {
                    // Ignore, and hope that the address ranges won't be reused by other libraries
                    // during the rest of the recording...
                }
            }
        }

        // Enumerate threads.
        let thread_acts = get_thread_list(self.task)?;
        let previously_live_threads: HashSet<_> =
            self.live_threads.iter().map(|(t, _)| *t).collect();
        let mut now_live_threads = HashSet::new();
        for thread_act in thread_acts {
            let mut entry = self.live_threads.entry(thread_act);
            let thread = match entry {
                Entry::Occupied(ref mut entry) => entry.get_mut(),
                Entry::Vacant(entry) => {
                    if let Some(thread) =
                        ThreadProfiler::new(self.task, self.pid, thread_act, now, false)
                    {
                        entry.insert(thread)
                    } else {
                        continue;
                    }
                }
            };
            // Grab a sample from the thread.
            let stackwalker = StackwalkerRef::new(&self.unwinder, unwinder_cache);
            let still_alive = thread.sample(stackwalker, now)?;
            if still_alive {
                now_live_threads.insert(thread_act);
            }
        }
        let dead_threads = previously_live_threads.difference(&now_live_threads);
        for thread_act in dead_threads {
            let mut thread = self.live_threads.remove(thread_act).unwrap();
            thread.notify_dead(now);
            self.dead_threads.push(thread);
        }
        Ok(())
    }

    fn add_lib_to_unwinder_and_ensure_debug_id(&mut self, lib: &mut DyldInfo) {
        let base_svma = lib.svma_info.base_svma;
        let base_avma = lib.base_avma;
        let unwind_info_data = lib
            .unwind_sections
            .unwind_info_section
            .and_then(|(svma, size)| {
                VmSubData::map_from_task(self.task, svma - base_svma + base_avma, size).ok()
            });
        let eh_frame_data = lib
            .unwind_sections
            .eh_frame_section
            .and_then(|(svma, size)| {
                VmSubData::map_from_task(self.task, svma - base_svma + base_avma, size).ok()
            });
        let text_data = lib.unwind_sections.text_segment.and_then(|(svma, size)| {
            let avma = svma - base_svma + base_avma;
            VmSubData::map_from_task(self.task, avma, size)
                .ok()
                .map(|data| {
                    TextByteData::new(UnwindSectionBytes::Remapped(data), avma..avma + size)
                })
        });

        if lib.debug_id.is_none() {
            if let (Some(text_data), Some(text_section)) =
                (text_data.as_ref(), lib.svma_info.text.clone())
            {
                let text_section_start_avma = text_section.start - base_svma + base_avma;
                let text_section_first_page_end_avma = text_section_start_avma.wrapping_add(4096);
                let debug_id = if let Some(text_first_page) =
                    text_data.get_bytes(text_section_start_avma..text_section_first_page_end_avma)
                {
                    // Generate a debug ID from the __text section.
                    DebugId::from_text_first_page(text_first_page, true)
                } else {
                    DebugId::nil()
                };
                lib.debug_id = Some(debug_id);
            }
        }

        let unwind_data = match (unwind_info_data, eh_frame_data) {
            (Some(unwind_info), eh_frame) => ModuleUnwindData::CompactUnwindInfoAndEhFrame(
                UnwindSectionBytes::Remapped(unwind_info),
                eh_frame.map(UnwindSectionBytes::Remapped),
            ),
            (None, Some(eh_frame)) => {
                ModuleUnwindData::EhFrame(UnwindSectionBytes::Remapped(eh_frame))
            }
            (None, None) => {
                // Have no unwind information.
                // Let's try to open the file and use debug_frame.
                if let Some(debug_frame) = get_debug_frame(&lib.file) {
                    ModuleUnwindData::DebugFrame(debug_frame)
                } else {
                    ModuleUnwindData::None
                }
            }
        };

        let module = Module::new(
            lib.file.clone(),
            lib.base_avma..(lib.base_avma + lib.vmsize),
            lib.base_avma,
            lib.svma_info.clone(),
            unwind_data,
            text_data,
        );
        self.unwinder.add_module(module);
    }

    pub fn notify_dead(&mut self, end_time: Instant) {
        for (_, mut thread) in self.live_threads.drain() {
            thread.notify_dead(end_time);
            self.dead_threads.push(thread);
        }
        self.end_time = Some(end_time);
        self.lib_info_manager.unmap_memory();
    }

    pub fn into_profile(self, subtasks: Vec<TaskProfiler>) -> ProfileBuilder {
        let name = self
            .executable_lib
            .map(|l| {
                Path::new(&l.file)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .unwrap_or(self.command_name);

        let mut profile_builder = ProfileBuilder::new(
            self.start_time,
            self.start_time_system,
            &name,
            self.pid,
            self.interval,
        );
        let all_threads = self
            .live_threads
            .into_iter()
            .map(|(_, t)| t)
            .chain(self.dead_threads.into_iter())
            .map(|t| t.into_profile_thread());
        for thread in all_threads {
            profile_builder.add_thread(thread);
        }

        if let Some(end_time) = self.end_time {
            profile_builder.set_end_time(end_time);
        }

        for DyldInfo {
            file,
            debug_id,
            base_avma,
            vmsize,
            arch,
            ..
        } in self.libs
        {
            let (debug_id, arch) = match (debug_id, arch) {
                (Some(debug_id), Some(arch)) => (debug_id, arch),
                _ => continue,
            };
            let path = Path::new(&file);
            let address_range = base_avma..(base_avma + vmsize);
            profile_builder.add_lib(
                path,
                None,
                path,
                debug_id,
                Some(arch),
                base_avma,
                address_range,
            );
        }

        for subtask in subtasks {
            profile_builder.add_subprocess(subtask.into_profile(Vec::new()));
        }

        profile_builder
    }
}

fn get_debug_frame(file_path: &str) -> Option<UnwindSectionBytes> {
    let file = std::fs::File::open(file_path).ok()?;
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file).ok()? };
    let data = &mmap[..];
    let obj = object::read::File::parse(data).ok()?;
    let compressed_range = if let Some(zdebug_frame_section) = obj.section_by_name("__zdebug_frame")
    {
        // Go binaries use compressed sections of the __zdebug_* type even on macOS,
        // where doing so is quite uncommon. Object's mach-O support does not handle them.
        // But we want to handle them.
        let (file_range_start, file_range_size) = zdebug_frame_section.file_range()?;
        let section_data = zdebug_frame_section.data().ok()?;
        if !section_data.starts_with(b"ZLIB\0\0\0\0") {
            return None;
        }
        let b = section_data.get(8..12)?;
        let uncompressed_size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        CompressedFileRange {
            format: CompressionFormat::Zlib,
            offset: file_range_start + 12,
            compressed_size: file_range_size - 12,
            uncompressed_size: uncompressed_size.into(),
        }
    } else {
        let debug_frame_section = obj.section_by_name("__debug_frame")?;
        debug_frame_section.compressed_file_range().ok()?
    };
    match compressed_range.format {
        CompressionFormat::None => Some(UnwindSectionBytes::Mmap(MmapSubData::try_new(
            mmap,
            compressed_range.offset as usize,
            compressed_range.uncompressed_size as usize,
        )?)),
        CompressionFormat::Unknown => None,
        CompressionFormat::Zlib => {
            let compressed_bytes = &mmap[compressed_range.offset as usize..]
                [..compressed_range.compressed_size as usize];

            let mut decompressed = Vec::with_capacity(compressed_range.uncompressed_size as usize);
            let mut decompress = flate2::Decompress::new(true);
            decompress
                .decompress_vec(
                    compressed_bytes,
                    &mut decompressed,
                    flate2::FlushDecompress::Finish,
                )
                .ok()?;
            Some(UnwindSectionBytes::Allocated(decompressed))
        }
        _ => None,
    }
}

fn get_thread_list(task: mach_port_t) -> Result<Vec<thread_act_t>, SamplingError> {
    let mut thread_list: thread_act_port_array_t = std::ptr::null_mut();
    let mut thread_count: mach_msg_type_number_t = Default::default();
    unsafe { task_threads(task, &mut thread_list, &mut thread_count) }
        .into_result()
        .map_err(|err| match err {
            KernelError::InvalidArgument
            | KernelError::MachSendInvalidDest
            | KernelError::Terminated => {
                SamplingError::ProcessTerminated("task_threads in get_thread_list", err)
            }
            err => SamplingError::Ignorable("task_threads in get_thread_list", err),
        })?;

    let thread_acts =
        unsafe { std::slice::from_raw_parts(thread_list, thread_count as usize) }.to_owned();

    unsafe {
        mach_vm_deallocate(
            mach_task_self(),
            thread_list as usize as mach_vm_address_t,
            (thread_count as usize * mem::size_of::<thread_act_t>()) as mach_vm_size_t,
        )
    }
    .into_result()
    .map_err(|err| SamplingError::Fatal("mach_vm_deallocate in get_thread_list", err))?;

    Ok(thread_acts)
}
