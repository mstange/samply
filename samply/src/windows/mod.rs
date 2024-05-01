#![allow(unused)]

mod etw_gecko;
mod etw_reader;
pub mod profiler;
mod winutils;

use crate::shared::context_switch::{OffCpuSampleGroup, ThreadContextSwitchData};
use crate::shared::lib_mappings::LibMappingOpQueue;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{UnresolvedSamples, UnresolvedStacks};
use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryColor, CategoryPairHandle, CounterHandle, CpuDelta, FrameFlags, FrameInfo,
    LibraryHandle, LibraryInfo, ProcessHandle, Profile, Symbol, ThreadHandle, Timestamp,
};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// An on- or off-cpu-sample for which the user stack is not known yet.
/// Consumed once the user stack arrives.
#[derive(Debug, Clone)]
struct PendingStack {
    /// The timestamp of the SampleProf or CSwitch event
    timestamp: u64,
    /// Starts out as None. Once we encounter the kernel stack (if any), we put it here.
    kernel_stack: Option<Vec<StackFrame>>,
    off_cpu_sample_group: Option<OffCpuSampleGroup>,
    on_cpu_sample_cpu_delta: Option<CpuDelta>,
}

#[derive(Debug)]
struct MemoryUsage {
    counter: CounterHandle,
    value: f64,
}

#[derive(Debug)]
struct ProcessJitInfo {
    lib_handle: LibraryHandle,
    jit_mapping_ops: LibMappingOpQueue,
    next_relative_address: u32,
    symbols: Vec<Symbol>,
}

#[derive(Debug)]
struct ThreadState {
    // When merging threads `handle` is the global thread handle and we use `merge_name` to store the name
    handle: ThreadHandle,
    merge_name: Option<String>,
    pending_stacks: VecDeque<PendingStack>,
    context_switch_data: ThreadContextSwitchData,
    memory_usage: Option<MemoryUsage>,
    thread_id: u32,
    process_id: u32,
}

impl ThreadState {
    fn new(handle: ThreadHandle, pid: u32, tid: u32) -> Self {
        ThreadState {
            handle,
            merge_name: None,
            pending_stacks: VecDeque::new(),
            context_switch_data: Default::default(),
            memory_usage: None,
            thread_id: tid,
            process_id: pid,
        }
    }

    fn display_name(&self) -> String {
        self.merge_name
            .as_ref()
            .map(|x| strip_thread_numbers(x).to_owned())
            .unwrap_or_else(|| format!("thread {}", self.thread_id))
    }
}

struct ProcessState {
    handle: ProcessHandle,
    unresolved_samples: UnresolvedSamples,
    regular_lib_mapping_ops: LibMappingOpQueue,
    main_thread_handle: Option<ThreadHandle>,
    pending_libraries: HashMap<u64, LibraryInfo>,
    memory_usage: Option<MemoryUsage>,
    process_id: u32,
    parent_id: u32,
}

impl ProcessState {
    pub fn new(handle: ProcessHandle, pid: u32, ppid: u32) -> Self {
        Self {
            handle,
            unresolved_samples: UnresolvedSamples::default(),
            regular_lib_mapping_ops: LibMappingOpQueue::default(),
            main_thread_handle: None,
            pending_libraries: HashMap::new(),
            memory_usage: None,
            process_id: pid,
            parent_id: ppid,
        }
    }
}

fn strip_thread_numbers(name: &str) -> &str {
    if let Some(hash) = name.find('#') {
        let (prefix, suffix) = name.split_at(hash);
        if suffix[1..].parse::<i32>().is_ok() {
            return prefix.trim();
        }
    }
    name
}

fn expand_full_filename_with_cwd(filename: &Path) -> PathBuf {
    if filename.is_absolute() {
        filename.to_path_buf()
    } else {
        let mut fullpath = std::env::current_dir().unwrap();
        fullpath.push(filename);
        fullpath
    }
}

struct ProfileContext {
    //profile: Arc<Mutex<Profile>>,
    profile: RefCell<Profile>,

    timebase_nanos: u64,

    // state -- keep track of the processes etc we've seen as we're processing,
    // and their associated handles in the json profile
    processes: HashMap<u32, RefCell<ProcessState>>,
    threads: HashMap<u32, RefCell<ThreadState>>,
    memory_usage: HashMap<u32, RefCell<MemoryUsage>>,

    unresolved_stacks: RefCell<UnresolvedStacks>,

    // track VM alloc/frees per thread? counter may be inaccurate because memory
    // can be allocated on one thread and freed on another
    per_thread_memory: bool,

    // If process and threads are being squished into one global one, here's the squished handles.
    // We still allocate a ThreadState/ProcessState per thread
    merge_threads: bool,
    global_process_handle: Option<ProcessHandle>,
    global_thread_handle: Option<ThreadHandle>,
    idle_thread_handle: Option<ThreadHandle>,
    other_thread_handle: Option<ThreadHandle>,

    // some special threads
    gpu_thread_handle: Option<ThreadHandle>,

    libs: HashMap<String, LibraryHandle>,

    // These are the processes + their children that we want to write into
    // the profile.json. If it's empty, trace everything.
    interesting_process_ids: HashSet<u32>,
    interesting_process_names: HashSet<String>,

    // default categories
    default_category: CategoryPairHandle,
    kernel_category: CategoryPairHandle,

    // cache of device mappings
    device_mappings: HashMap<String, String>, // map of \Device\HarddiskVolume4 -> C:\

    // the minimum address for kernel drivers, so that we can assign kernel_category to the frame
    // TODO why is this needed -- kernel libs are at global addresses, why do I need to indicate
    // this per-frame; shouldn't there be some kernel override?
    kernel_min: u64,

    // architecture to record in the trace. will be the system architecture for now.
    // TODO no idea how to handle "I'm on aarch64 windows but I'm recording a win64 process".
    // I have no idea how stack traces work in that case anyway, so this is probably moot.
    arch: String,

    // the ETL file we're either recording to or parsing from
    etl_file: Option<PathBuf>,

    // if xperf is currently running
    xperf_running: bool,
}

impl ProfileContext {
    const K_GLOBAL_MERGED_PROCESS_ID: u32 = 0;
    const K_GLOBAL_MERGED_THREAD_ID: u32 = 1;
    const K_GLOBAL_IDLE_THREAD_ID: u32 = 0;
    const K_GLOBAL_OTHER_THREAD_ID: u32 = u32::MAX;

    fn new(mut profile: Profile, arch: &str, merge_threads: bool, include_idle: bool) -> Self {
        let default_category =
            CategoryPairHandle::from(profile.add_category("User", CategoryColor::Yellow));
        let kernel_category =
            CategoryPairHandle::from(profile.add_category("Kernel", CategoryColor::Orange));

        // On 64-bit systems, the kernel address space always has 0xF in the first 16 bits.
        // The actual kernel address space is much higher, but we just need this to disambiguate kernel and user
        // stacks. Use add_kernel_drivers to get accurate mappings.
        let kernel_min: u64 = if arch == "x86" {
            0x8000_0000
        } else {
            0xF000_0000_0000_0000
        };

        let mut result = Self {
            profile: RefCell::new(profile),
            timebase_nanos: 0,
            processes: HashMap::new(),
            threads: HashMap::new(),
            memory_usage: HashMap::new(),
            unresolved_stacks: RefCell::new(UnresolvedStacks::default()),
            global_process_handle: None,
            global_thread_handle: None,
            idle_thread_handle: None,
            other_thread_handle: None,
            gpu_thread_handle: None,
            per_thread_memory: false,
            merge_threads,
            libs: HashMap::new(),
            interesting_process_ids: HashSet::new(),
            interesting_process_names: HashSet::new(),
            default_category,
            kernel_category,
            device_mappings: winutils::get_dos_device_mappings(),
            kernel_min,
            arch: arch.to_string(),
            etl_file: None,
            xperf_running: false,
        };

        if merge_threads {
            let start_instant = Timestamp::from_nanos_since_reference(0);
            let mut profile = result.profile.borrow_mut();

            let global_process_handle = profile.add_process(
                "All processes",
                Self::K_GLOBAL_MERGED_PROCESS_ID,
                start_instant,
            );
            let global_thread_handle = profile.add_thread(
                global_process_handle,
                Self::K_GLOBAL_MERGED_THREAD_ID,
                start_instant,
                true,
            );
            profile.set_thread_name(global_thread_handle, "All threads");

            result.global_process_handle = Some(global_process_handle);
            result.global_thread_handle = Some(global_thread_handle);

            if include_idle {
                let idle_thread_handle = profile.add_thread(
                    global_process_handle,
                    Self::K_GLOBAL_IDLE_THREAD_ID,
                    start_instant,
                    false,
                );
                profile.set_thread_name(idle_thread_handle, "Idle");
                let other_thread_handle = profile.add_thread(
                    global_process_handle,
                    Self::K_GLOBAL_OTHER_THREAD_ID,
                    start_instant,
                    false,
                );
                profile.set_thread_name(other_thread_handle, "Other");

                result.idle_thread_handle = Some(idle_thread_handle);
                result.other_thread_handle = Some(other_thread_handle);
            }
        }

        result
    }

    fn with_profile<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut Profile) -> T,
    {
        func(&mut self.profile.borrow_mut())
    }

    // add_process and add_thread always add a process/thread (and thread expects process to exist)
    fn add_process(&mut self, pid: u32, parent_id: u32, name: &str, start_time: Timestamp) {
        let process_handle = if self.merge_threads {
            self.global_process_handle.unwrap()
        } else {
            self.profile.borrow_mut().add_process(name, pid, start_time)
        };
        let process = ProcessState::new(process_handle, pid, parent_id);
        self.processes.insert(pid, RefCell::new(process));
    }

    fn remove_process(&mut self, pid: u32, timestamp: Option<Timestamp>) -> Option<ProcessHandle> {
        if let Some(process) = self.processes.remove(&pid) {
            let process = process.into_inner();
            if let Some(timestamp) = timestamp {
                self.profile
                    .borrow_mut()
                    .set_process_end_time(process.handle, timestamp);
            }

            Some(process.handle)
        } else {
            None
        }
    }

    fn get_process(&self, pid: u32) -> Option<Ref<'_, ProcessState>> {
        self.processes.get(&pid).map(|p| p.borrow())
    }

    fn get_process_mut(&self, pid: u32) -> Option<RefMut<'_, ProcessState>> {
        self.processes.get(&pid).map(|p| p.borrow_mut())
    }

    fn get_process_handle(&self, pid: u32) -> Option<ProcessHandle> {
        self.get_process(pid).map(|p| p.handle)
    }

    fn add_thread(&mut self, pid: u32, tid: u32, start_time: Timestamp) {
        assert!(
            self.processes.contains_key(&pid),
            "Adding thread for non-existent process"
        );

        let thread_handle = if self.merge_threads {
            self.global_thread_handle.unwrap()
        } else {
            let mut process = self.processes.get_mut(&pid).unwrap().borrow_mut();
            let is_main = process.main_thread_handle.is_none();
            let thread_handle =
                self.profile
                    .borrow_mut()
                    .add_thread(process.handle, tid, start_time, is_main);
            if is_main {
                process.main_thread_handle = Some(thread_handle);
            }
            thread_handle
        };

        let thread = ThreadState::new(thread_handle, pid, tid);
        self.threads.insert(tid, RefCell::new(thread));
    }

    fn remove_thread(&mut self, tid: u32, timestamp: Option<Timestamp>) -> Option<ThreadHandle> {
        if let Some(thread) = self.threads.remove(&tid) {
            let thread = thread.into_inner();
            if let Some(timestamp) = timestamp {
                self.profile
                    .borrow_mut()
                    .set_thread_end_time(thread.handle, timestamp);
            }

            Some(thread.handle)
        } else {
            None
        }
    }

    fn get_process_for_thread(&self, tid: u32) -> Option<Ref<'_, ProcessState>> {
        let pid = self.threads.get(&tid)?.borrow().process_id;
        self.processes.get(&pid).map(|p| p.borrow())
    }

    fn get_process_for_thread_mut(&self, tid: u32) -> Option<RefMut<'_, ProcessState>> {
        let pid = self.threads.get(&tid)?.borrow().process_id;
        self.processes.get(&pid).map(|p| p.borrow_mut())
    }

    fn get_thread(&self, tid: u32) -> Option<Ref<'_, ThreadState>> {
        self.threads.get(&tid).map(|p| p.borrow())
    }

    fn get_thread_mut(&self, tid: u32) -> Option<RefMut<'_, ThreadState>> {
        self.threads.get(&tid).map(|p| p.borrow_mut())
    }

    fn get_thread_handle(&self, tid: u32) -> Option<ThreadHandle> {
        self.get_thread(tid).map(|t| t.handle)
    }

    // If we should use an
    // an Idle/Other thread)
    fn get_idle_handle_if_appropriate(&self, tid: u32) -> Option<ThreadHandle> {
        if self.threads.contains_key(&tid) || !self.merge_threads {
            None
        } else if tid == Self::K_GLOBAL_IDLE_THREAD_ID {
            self.idle_thread_handle
        } else {
            self.other_thread_handle
        }
    }

    fn set_thread_name(&self, tid: u32, name: &str) {
        if let Some(mut thread) = self.get_thread_mut(tid) {
            self.profile
                .borrow_mut()
                .set_thread_name(thread.handle, name);
            thread.merge_name = Some(name.to_string());
        }
    }

    fn get_or_create_memory_usage_counter(&mut self, tid: u32) -> Option<CounterHandle> {
        // kinda hate this. ProfileContext should really manage adjusting the counter,
        // so that it can do things like keep global + per-thread in sync

        if self.per_thread_memory {
            let process = self.get_process_for_thread(tid)?;
            let process_handle = process.handle;
            let mut thread = self.get_thread_mut(tid).unwrap();
            let memory_usage = thread.memory_usage.get_or_insert_with(|| {
                let counter = self.profile.borrow_mut().add_counter(
                    process_handle,
                    "VM",
                    &format!("Memory (Thread {})", tid),
                    "Amount of VirtualAlloc allocated memory",
                );
                MemoryUsage {
                    counter,
                    value: 0.0,
                }
            });
            Some(memory_usage.counter)
        } else {
            let mut process = self.get_process_for_thread_mut(tid)?;
            let process_handle = process.handle;
            let memory_usage = process.memory_usage.get_or_insert_with(|| {
                let counter = self.profile.borrow_mut().add_counter(
                    process_handle,
                    "VM",
                    "Memory",
                    "Amount of VirtualAlloc allocated memory",
                );
                MemoryUsage {
                    counter,
                    value: 0.0,
                }
            });
            Some(memory_usage.counter)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_sample(
        &self,
        pid: u32,
        tid: u32,
        timestamp: Timestamp,
        timestamp_raw: u64,
        cpu_delta: CpuDelta,
        weight: i32,
        stack: Vec<StackFrame>,
    ) {
        let mut profile = self.profile.borrow_mut();
        let stack_index = self
            .unresolved_stacks
            .borrow_mut()
            .convert(stack.into_iter().rev());
        let thread = self.get_thread(tid).unwrap();
        let extra_label_frame = if self.merge_threads {
            let display_name = thread.display_name();
            Some(FrameInfo {
                frame: fxprof_processed_profile::Frame::Label(profile.intern_string(&display_name)),
                category_pair: self.default_category,
                flags: FrameFlags::empty(),
            })
        } else {
            None
        };
        let thread = thread.handle;
        self.get_process_mut(pid)
            .unwrap()
            .unresolved_samples
            .add_sample(
                thread,
                timestamp,
                timestamp_raw,
                stack_index,
                cpu_delta,
                weight,
                extra_label_frame,
            );
    }

    fn get_or_add_lib_simple(&mut self, filename: &str) -> LibraryHandle {
        if let Some(&handle) = self.libs.get(filename) {
            handle
        } else {
            let lib_info = self.get_library_info_for_path(filename);
            let handle = self.profile.borrow_mut().add_lib(lib_info);
            self.libs.insert(filename.to_string(), handle);
            handle
        }
    }

    fn try_get_library_info_for_path(&self, path: &str) -> Option<LibraryInfo> {
        let path = self.map_device_path(path);
        let name = PathBuf::from(&path)
            .file_name()?
            .to_string_lossy()
            .to_string();
        let file = std::fs::File::open(&path).ok()?;
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }.ok()?;
        let object = object::File::parse(&mmap[..]).ok()?;
        let debug_id = wholesym::samply_symbols::debug_id_for_object(&object);
        use object::Object;
        let arch = object_arch_to_string(object.architecture()).map(ToOwned::to_owned);
        let pe_info = match &object {
            object::File::Pe32(pe_file) => Some(pe_info(pe_file)),
            object::File::Pe64(pe_file) => Some(pe_info(pe_file)),
            _ => None,
        };
        let info = LibraryInfo {
            name: name.to_string(),
            path: path.to_string(),
            debug_name: pe_info
                .as_ref()
                .and_then(|pi| pi.pdb_name.clone())
                .unwrap_or_else(|| name.to_string()),
            debug_path: pe_info
                .as_ref()
                .and_then(|pi| pi.pdb_path.clone())
                .unwrap_or_else(|| path.to_string()),
            debug_id: debug_id.unwrap_or_else(debugid::DebugId::nil),
            code_id: pe_info.as_ref().map(|pi| pi.code_id.to_string()),
            arch,
            symbol_table: None,
        };
        Some(info)
    }

    fn get_library_info_for_path(&self, path: &str) -> LibraryInfo {
        if let Some(info) = self.try_get_library_info_for_path(path) {
            info
        } else {
            // Not found; dummy
            LibraryInfo {
                name: PathBuf::from(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string(),
                debug_name: "".to_owned(),
                debug_path: "".to_owned(),
                debug_id: DebugId::from_uuid(Uuid::new_v4()),
                code_id: None,
                arch: Some(self.arch.clone()),
                symbol_table: None,
            }
        }
    }

    fn add_interesting_process_name(&mut self, name: &str) {
        self.interesting_process_names.insert(name.to_lowercase());
    }

    fn add_interesting_process_id(&mut self, pid: u32) {
        self.interesting_process_ids.insert(pid);
    }

    fn is_interesting_process(&self, pid: u32, ppid: Option<u32>, name: Option<&str>) -> bool {
        if pid == 0 {
            return false;
        }

        // already tracking?
        if self.processes.contains_key(&pid) {
            return true;
        }

        // all processes if nothing specified
        if self.interesting_process_ids.is_empty() && self.interesting_process_names.is_empty() {
            return true;
        }

        // if pid or ppid are explicitly interesting
        if self.interesting_process_ids.contains(&pid)
            || ppid.is_some_and(|k| self.processes.contains_key(&k))
        {
            return true;
        }

        // if the name contains any of the interesting names
        if let Some(name) = name {
            let name = name.to_lowercase();
            for target in &self.interesting_process_names {
                if name.contains(target) {
                    return true;
                }
            }
        }

        // nope, boring process
        false
    }

    fn new_with_existing_recording(profile: Profile, arch: &str, etl_file: &Path) -> Self {
        let mut context = Self::new(profile, arch, false, false);
        context.etl_file = Some(PathBuf::from(etl_file));
        context
    }

    fn add_kernel_drivers(&mut self) {
        for (path, start_avma, end_avma) in winutils::iter_kernel_drivers() {
            let path = self.map_device_path(&path);
            eprintln!("kernel driver: {} {:x} {:x}", path, start_avma, end_avma);
            let lib_info = self.get_library_info_for_path(&path);
            let lib_handle = self.profile.borrow_mut().add_lib(lib_info);
            self.profile
                .borrow_mut()
                .add_kernel_lib_mapping(lib_handle, start_avma, end_avma, 0);
        }
    }

    fn stack_mode_for_address(&self, address: u64) -> StackMode {
        if address >= self.kernel_min {
            StackMode::Kernel
        } else {
            StackMode::User
        }
    }

    // The filename is a NT kernel path (https://chrisdenton.github.io/omnipath/NT.html) which isn't direclty
    // usable from user space.  perfview goes through a dance to convert it to a regular user space path
    // https://github.com/microsoft/perfview/blob/4fb9ec6947cb4e68ac7cb5e80f50ae3757d0ede4/src/TraceEvent/Parsers/KernelTraceEventParser.cs#L3461
    // and we do a bit of it here, just for dos drive mappings. Everything else we prefix with \\?\GLOBALROOT\
    fn map_device_path(&self, path: &str) -> String {
        for (k, v) in &self.device_mappings {
            if path.starts_with(k) {
                let r = format!("{}{}", v, path.split_at(k.len()).1);
                return r;
            }
        }

        // if we didn't translate (still have a \\ path), prefix with GLOBALROOT as
        // an escape
        if path.starts_with("\\\\") {
            format!("\\\\?\\GLOBALROOT{}", path)
        } else {
            path.into()
        }
    }

    fn start_xperf(&mut self, output_file: &Path) {
        // start xperf.exe, logging to the same location as the output file, just with a .etl
        // extension.
        let mut etl_file = output_file.to_path_buf();
        etl_file.set_extension("unmerged-etl");

        let mut xperf = runas::Command::new("xperf");
        // Virtualised ARM64 Windows crashes out on PROFILE tracing, and that's what I'm developing
        // on, so these are hacky args to get me a useful profile that I can work with.
        xperf.arg("-on");
        if self.arch != "aarch64" {
            xperf.arg("PROC_THREAD+LOADER+PROFILE+CSWITCH");
        } else {
            xperf.arg("PROC_THREAD+LOADER+CSWITCH+SYSCALL+VIRT_ALLOC+OB_HANDLE");
        }
        xperf.arg("-stackwalk");
        if self.arch != "aarch64" {
            xperf.arg("PROFILE+CSWITCH");
        } else {
            xperf.arg("VirtualAlloc+VirtualFree+HandleCreate+HandleClose");
        }
        xperf.arg("-f");
        xperf.arg(expand_full_filename_with_cwd(&etl_file));

        let _ = xperf.status().expect("failed to execute xperf");

        eprintln!("xperf session running...");

        self.etl_file = Some(PathBuf::from(&etl_file));
        self.xperf_running = true;
    }

    fn stop_xperf(&mut self) {
        let unmerged_etl = self.etl_file.take().unwrap();
        let merged_etl = unmerged_etl.with_extension("etl");

        let mut xperf = runas::Command::new("xperf");
        xperf.arg("-stop");
        xperf.arg("-d");
        xperf.arg(expand_full_filename_with_cwd(&merged_etl));

        let _ = xperf
            .status()
            .expect("Failed to execute xperf -stop! xperf may still be recording.");

        eprintln!("xperf session stopped.");

        std::fs::remove_file(&unmerged_etl).unwrap_or_else(|_| {
            panic!(
                "Failed to delete unmerged ETL file {:?}",
                unmerged_etl.to_str().unwrap()
            )
        });

        self.etl_file = Some(merged_etl);
        self.xperf_running = false;
    }
}

impl Drop for ProfileContext {
    fn drop(&mut self) {
        if self.xperf_running {
            self.stop_xperf();
        }
    }
}

struct PeInfo {
    code_id: wholesym::CodeId,
    pdb_path: Option<String>,
    pdb_name: Option<String>,
}

fn pe_info<'a, Pe: object::read::pe::ImageNtHeaders, R: object::ReadRef<'a>>(
    pe: &object::read::pe::PeFile<'a, Pe, R>,
) -> PeInfo {
    // The code identifier consists of the `time_date_stamp` field id the COFF header, followed by
    // the `size_of_image` field in the optional header. If the optional PE header is not present,
    // this identifier is `None`.
    let header = pe.nt_headers();
    let timestamp = header
        .file_header()
        .time_date_stamp
        .get(object::LittleEndian);
    use object::read::pe::ImageOptionalHeader;
    let image_size = header.optional_header().size_of_image();
    let code_id = wholesym::CodeId::PeCodeId(wholesym::PeCodeId {
        timestamp,
        image_size,
    });

    use object::Object;
    let pdb_path: Option<String> = pe.pdb_info().ok().and_then(|pdb_info| {
        let pdb_path = std::str::from_utf8(pdb_info?.path()).ok()?;
        Some(pdb_path.to_string())
    });

    let pdb_name = pdb_path
        .as_deref()
        .map(|pdb_path| match pdb_path.rsplit_once(['/', '\\']) {
            Some((_base, file_name)) => file_name.to_string(),
            None => pdb_path.to_string(),
        });

    PeInfo {
        code_id,
        pdb_path,
        pdb_name,
    }
}

fn object_arch_to_string(arch: object::Architecture) -> Option<&'static str> {
    let s = match arch {
        object::Architecture::Arm => "arm",
        object::Architecture::Aarch64 => "arm64",
        object::Architecture::I386 => "x86",
        object::Architecture::X86_64 => "x86_64",
        _ => return None,
    };
    Some(s)
}
