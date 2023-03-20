use framehop::{
    CacheNative, MayAllocateDuringUnwind, Module, ModuleUnwindData, TextByteData, Unwinder,
    UnwinderNative,
};
use fxprof_processed_profile::debugid::DebugId;
use fxprof_processed_profile::{
    CategoryPairHandle, LibraryInfo, ProcessHandle, Profile, Timestamp,
};
use mach::mach_types::thread_act_port_array_t;
use mach::mach_types::thread_act_t;
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::task::task_threads;
use mach::traps::mach_task_self;
use mach::vm::mach_vm_deallocate;
use mach::vm_types::{mach_vm_address_t, mach_vm_size_t};
use object::{CompressedFileRange, CompressionFormat, Object, ObjectSection};
use samply_symbols::{object, DebugIdExt};
use wholesym::samply_symbols;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::Deref;
use std::path::Path;

use super::error::SamplingError;
use super::kernel_error::{IntoResult, KernelError};
use super::proc_maps::{DyldInfo, DyldInfoManager, Modification, StackwalkerRef, VmSubData};
use super::thread_profiler::{get_thread_id, ThreadProfiler};

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
    live_threads: HashMap<thread_act_t, ThreadProfiler>,
    dead_threads: Vec<ThreadProfiler>,
    lib_info_manager: DyldInfoManager,
    executable_lib: Option<DyldInfo>,
    command_name: String,
    profile_process: ProcessHandle,
    ignored_errors: Vec<SamplingError>,
    unwinder: UnwinderNative<UnwindSectionBytes, MayAllocateDuringUnwind>,
    default_category: CategoryPairHandle,
}

impl TaskProfiler {
    pub fn new(
        task: mach_port_t,
        pid: u32,
        start_time: Timestamp,
        command_name: &str,
        profile: &mut Profile,
        default_category: CategoryPairHandle,
    ) -> Result<Self, SamplingError> {
        let thread_acts = get_thread_list(task)?;
        let profile_process = profile.add_process(command_name, pid, start_time);
        let mut live_threads = HashMap::new();
        for (i, thread_act) in thread_acts.into_iter().enumerate() {
            // Assume that the first thread is the main thread. This seems to hold true in practice.
            let is_main = i == 0;
            if let Ok((tid, _is_libdispatch_thread)) = get_thread_id(thread_act) {
                let profile_thread = profile.add_thread(profile_process, tid, start_time, is_main);
                let thread =
                    ThreadProfiler::new(task, tid, profile_thread, thread_act, default_category);
                live_threads.insert(thread_act, thread);
            }
        }
        Ok(TaskProfiler {
            task,
            pid,
            live_threads,
            dead_threads: Vec::new(),
            lib_info_manager: DyldInfoManager::new(task),
            command_name: command_name.to_owned(),
            profile_process,
            executable_lib: None,
            ignored_errors: Vec::new(),
            unwinder: UnwinderNative::new(),
            default_category,
        })
    }

    pub fn sample(
        &mut self,
        now: Timestamp,
        unwinder_cache: &mut UnwinderCache,
        profile: &mut Profile,
    ) -> Result<bool, SamplingError> {
        let result = self.sample_impl(now, unwinder_cache, profile);
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
        now: Timestamp,
        unwinder_cache: &mut UnwinderCache,
        profile: &mut Profile,
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
                    let path = Path::new(&lib.file);
                    if self.executable_lib.is_none() && lib.is_executable {
                        self.executable_lib = Some(lib.clone());
                        self.command_name = path
                            .components()
                            .next_back()
                            .unwrap()
                            .as_os_str()
                            .to_string_lossy()
                            .to_string();
                        profile.set_process_name(self.profile_process, &self.command_name);
                    }

                    if let Some(name) = path.file_name() {
                        let name = name.to_string_lossy();
                        let path = path.to_string_lossy();
                        let lib_handle = profile.add_lib(LibraryInfo {
                            name: name.to_string(),
                            debug_name: name.to_string(),
                            path: path.to_string(),
                            debug_path: path.to_string(),
                            debug_id: lib.debug_id.unwrap(),
                            code_id: lib.code_id.map(|ci| ci.to_string()),
                            arch: lib.arch.map(ToOwned::to_owned),
                            symbol_table: None,
                        });
                        profile.add_lib_mapping(
                            self.profile_process,
                            lib_handle,
                            lib.base_avma,
                            lib.base_avma + lib.vmsize,
                            0,
                        );
                    }
                }
                Modification::Removed(lib) => {
                    profile.remove_lib_mapping(self.profile_process, lib.base_avma);
                }
            }
        }

        // Enumerate threads.
        let thread_acts = get_thread_list(self.task)?;
        let previously_live_threads: HashSet<_> = self.live_threads.keys().cloned().collect();
        let mut now_live_threads = HashSet::new();
        for thread_act in thread_acts {
            let mut entry = self.live_threads.entry(thread_act);
            let thread = match entry {
                Entry::Occupied(ref mut entry) => entry.get_mut(),
                Entry::Vacant(entry) => {
                    if let Ok((tid, _is_libdispatch_thread)) = get_thread_id(thread_act) {
                        let profile_thread =
                            profile.add_thread(self.profile_process, tid, now, false);
                        let thread = ThreadProfiler::new(
                            self.task,
                            tid,
                            profile_thread,
                            thread_act,
                            self.default_category,
                        );
                        entry.insert(thread)
                    } else {
                        continue;
                    }
                }
            };
            // Grab a sample from the thread.
            let stackwalker = StackwalkerRef::new(&self.unwinder, unwinder_cache);
            let still_alive = thread.sample(stackwalker, now, profile)?;
            if still_alive {
                now_live_threads.insert(thread_act);
            }
        }
        let dead_threads = previously_live_threads.difference(&now_live_threads);
        for thread_act in dead_threads {
            let mut thread = self.live_threads.remove(thread_act).unwrap();
            thread.notify_dead(now, profile);
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

    pub fn notify_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        for (_, mut thread) in self.live_threads.drain() {
            thread.notify_dead(end_time, profile);
            self.dead_threads.push(thread);
        }
        profile.set_process_end_time(self.profile_process, end_time);
        self.lib_info_manager.unmap_memory();
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
