#![allow(unused)]

use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CounterHandle, CpuDelta, FrameFlags, FrameInfo, LibraryHandle,
    LibraryInfo, MarkerHandle, MarkerTiming, ProcessHandle, Profile, ProfilerMarker, Symbol,
    ThreadHandle, Timestamp,
};
use uuid::Uuid;

use super::coreclr::CoreClrContext;
use super::winutils;
use crate::shared::context_switch::{
    ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData,
};
use crate::shared::included_processes::IncludedProcesses;
use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::lib_mappings::LibMappingOpQueue;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{
    SampleOrMarker, UnresolvedSamples, UnresolvedStackHandle, UnresolvedStacks,
};

/// An on- or off-cpu-sample for which the user stack is not known yet.
/// Consumed once the user stack arrives.
#[derive(Debug, Clone)]
pub struct PendingStack {
    /// The timestamp of the SampleProf or CSwitch event
    pub timestamp: u64,
    /// Starts out as None. Once we encounter the kernel stack (if any), we put it here.
    pub kernel_stack: Option<Vec<StackFrame>>,
    pub off_cpu_sample_group: Option<OffCpuSampleGroup>,
    pub on_cpu_sample_cpu_delta: Option<CpuDelta>,
}

#[derive(Debug)]
pub struct MemoryUsage {
    pub counter: CounterHandle,
    pub value: f64,
}

#[derive(Debug)]
pub struct ProcessJitInfo {
    pub lib_handle: LibraryHandle,
    pub jit_mapping_ops: LibMappingOpQueue,
    pub next_relative_address: u32,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug)]
pub struct PendingMarker {
    pub text: String,
    pub start: Timestamp,
}

#[derive(Debug)]
pub struct ThreadState {
    // When merging threads `handle` is the global thread handle and we use `merge_name` to store the name
    pub handle: ThreadHandle,
    pub merge_name: Option<String>,
    pub pending_stacks: VecDeque<PendingStack>,
    pub context_switch_data: ThreadContextSwitchData,
    pub memory_usage: Option<MemoryUsage>,
    pub thread_id: u32,
    pub process_id: u32,
    pub pending_markers: HashMap<String, PendingMarker>,
}

impl ThreadState {
    fn new(handle: ThreadHandle, pid: u32, tid: u32) -> Self {
        ThreadState {
            handle,
            merge_name: None,
            pending_stacks: VecDeque::new(),
            context_switch_data: Default::default(),
            pending_markers: HashMap::new(),
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

pub struct ProcessState {
    pub handle: ProcessHandle,
    pub unresolved_samples: UnresolvedSamples,
    pub regular_lib_mapping_ops: LibMappingOpQueue,
    pub main_thread_handle: Option<ThreadHandle>,
    pub pending_libraries: HashMap<u64, LibraryInfo>,
    pub memory_usage: Option<MemoryUsage>,
    pub process_id: u32,
    pub parent_id: u32,
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

// Known profiler categories, lazy-created
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum KnownCategory {
    Default,
    User,
    Kernel,
    System,
    D3DVideoSubmitDecoderBuffers,
    CoreClrR2r,
    CoreClrJit,
    CoreClrGc,
    Unknown,
}

pub struct ProfileContext {
    pub profile: RefCell<Profile>,

    timebase_nanos: u64,

    // state -- keep track of the processes etc we've seen as we're processing,
    // and their associated handles in the json profile
    pub processes: HashMap<u32, RefCell<ProcessState>>,
    pub threads: HashMap<u32, RefCell<ThreadState>>,
    pub memory_usage: HashMap<u32, RefCell<MemoryUsage>>,
    pub process_jit_infos: HashMap<u32, RefCell<ProcessJitInfo>>,

    pub unresolved_stacks: RefCell<UnresolvedStacks>,

    // track VM alloc/frees per thread? counter may be inaccurate because memory
    // can be allocated on one thread and freed on another
    per_thread_memory: bool,

    idle_thread_handle: Option<ThreadHandle>,
    other_thread_handle: Option<ThreadHandle>,

    // some special threads
    gpu_thread_handle: Option<ThreadHandle>,

    libs: HashMap<String, LibraryHandle>,

    // These are the processes + their descendants that we want to write into
    // the profile.json. If it's None, include everything.
    included_processes: Option<IncludedProcesses>,

    // default categories
    categories: RefCell<HashMap<KnownCategory, CategoryHandle>>,

    pub js_category_manager: RefCell<JitCategoryManager>,
    pub context_switch_handler: RefCell<ContextSwitchHandler>,

    // cache of device mappings
    device_mappings: HashMap<String, String>, // map of \Device\HarddiskVolume4 -> C:\

    // the minimum address for kernel drivers, so that we can assign kernel_category to the frame
    // TODO why is this needed -- kernel libs are at global addresses, why do I need to indicate
    // this per-frame; shouldn't there be some kernel override?
    pub kernel_min: u64,

    // architecture to record in the trace. will be the system architecture for now.
    // TODO no idea how to handle "I'm on aarch64 windows but I'm recording a win64 process".
    // I have no idea how stack traces work in that case anyway, so this is probably moot.
    pub arch: String,

    pub coreclr_context: RefCell<CoreClrContext>,
}

impl ProfileContext {
    pub fn new(
        mut profile: Profile,
        arch: &str,
        included_processes: Option<IncludedProcesses>,
    ) -> Self {
        // On 64-bit systems, the kernel address space always has 0xF in the first 16 bits.
        // The actual kernel address space is much higher, but we just need this to disambiguate kernel and user
        // stacks. Use add_kernel_drivers to get accurate mappings.
        let kernel_min: u64 = if arch == "x86" {
            0x8000_0000
        } else {
            0xF000_0000_0000_0000
        };

        Self {
            profile: RefCell::new(profile),
            timebase_nanos: 0,
            processes: HashMap::new(),
            threads: HashMap::new(),
            memory_usage: HashMap::new(),
            process_jit_infos: HashMap::new(),
            unresolved_stacks: RefCell::new(UnresolvedStacks::default()),
            idle_thread_handle: None,
            other_thread_handle: None,
            gpu_thread_handle: None,
            per_thread_memory: false,
            libs: HashMap::new(),
            included_processes,
            categories: RefCell::new(HashMap::new()),
            js_category_manager: RefCell::new(JitCategoryManager::new()),
            context_switch_handler: RefCell::new(ContextSwitchHandler::new(122100)), // hardcoded, but replaced once TraceStart is received
            device_mappings: winutils::get_dos_device_mappings(),
            kernel_min,
            arch: arch.to_string(),
            coreclr_context: RefCell::new(CoreClrContext::new()),
        }
    }

    #[rustfmt::skip]
    const CATEGORIES: &'static [(KnownCategory, &'static str, CategoryColor)] = &[
        (KnownCategory::User, "User", CategoryColor::Yellow),
        (KnownCategory::Kernel, "Kernel", CategoryColor::LightRed),
        (KnownCategory::System, "System Libraries", CategoryColor::Orange),
        (KnownCategory::D3DVideoSubmitDecoderBuffers, "D3D Video Submit Decoder Buffers", CategoryColor::Transparent),
        (KnownCategory::CoreClrR2r, "CoreCLR R2R", CategoryColor::Blue),
        (KnownCategory::CoreClrJit, "CoreCLR JIT", CategoryColor::Purple),
        (KnownCategory::CoreClrGc, "CoreCLR GC", CategoryColor::Red),
        (KnownCategory::Unknown, "Other", CategoryColor::DarkGray),
    ];

    pub fn get_category(&self, category: KnownCategory) -> CategoryHandle {
        let category = if category == KnownCategory::Default {
            KnownCategory::User
        } else {
            category
        };

        *self
            .categories
            .borrow_mut()
            .entry(category)
            .or_insert_with(|| {
                let (category_name, color) = Self::CATEGORIES
                    .iter()
                    .find(|(c, _, _)| *c == category)
                    .map(|(_, name, color)| (*name, *color))
                    .unwrap();
                self.profile.borrow_mut().add_category(category_name, color)
            })
    }

    pub fn ensure_process_jit_info(&mut self, pid: u32) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.process_jit_infos.entry(pid) {
            let jitname = format!("JIT-{}", pid);
            let jitlib = self.profile.borrow_mut().add_lib(LibraryInfo {
                name: jitname.clone(),
                debug_name: jitname.clone(),
                path: jitname.clone(),
                debug_path: jitname.clone(),
                debug_id: DebugId::nil(),
                code_id: None,
                arch: None,
                symbol_table: None,
            });
            e.insert(RefCell::new(ProcessJitInfo {
                lib_handle: jitlib,
                jit_mapping_ops: LibMappingOpQueue::default(),
                next_relative_address: 0,
                symbols: Vec::new(),
            }));
        }
    }

    pub fn get_process_jit_info(&self, pid: u32) -> RefMut<ProcessJitInfo> {
        self.process_jit_infos.get(&pid).unwrap().borrow_mut()
    }

    // add_process and add_thread always add a process/thread (and thread expects process to exist)
    pub fn add_process(&mut self, pid: u32, parent_id: u32, name: &str, start_time: Timestamp) {
        let process_handle = self.profile.borrow_mut().add_process(name, pid, start_time);
        let process = ProcessState::new(process_handle, pid, parent_id);
        self.processes.insert(pid, RefCell::new(process));
    }

    pub fn remove_process(
        &mut self,
        pid: u32,
        timestamp: Option<Timestamp>,
    ) -> Option<ProcessHandle> {
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

    pub fn get_process(&self, pid: u32) -> Option<Ref<'_, ProcessState>> {
        self.processes.get(&pid).map(|p| p.borrow())
    }

    pub fn get_process_mut(&self, pid: u32) -> Option<RefMut<'_, ProcessState>> {
        self.processes.get(&pid).map(|p| p.borrow_mut())
    }

    pub fn get_process_handle(&self, pid: u32) -> Option<ProcessHandle> {
        self.get_process(pid).map(|p| p.handle)
    }

    pub fn add_thread(&mut self, pid: u32, tid: u32, start_time: Timestamp) {
        if !self.processes.contains_key(&pid) {
            log::warn!("Adding thread {tid} for unknown pid {pid}");
            return;
        }

        let mut process = self.processes.get_mut(&pid).unwrap().borrow_mut();
        let is_main = process.main_thread_handle.is_none();
        let thread_handle =
            self.profile
                .borrow_mut()
                .add_thread(process.handle, tid, start_time, is_main);
        if is_main {
            process.main_thread_handle = Some(thread_handle);
        }

        let thread = ThreadState::new(thread_handle, pid, tid);
        self.threads.insert(tid, RefCell::new(thread));
    }

    pub fn remove_thread(
        &mut self,
        tid: u32,
        timestamp: Option<Timestamp>,
    ) -> Option<ThreadHandle> {
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

    pub fn get_process_for_thread(&self, tid: u32) -> Option<Ref<'_, ProcessState>> {
        let pid = self.threads.get(&tid)?.borrow().process_id;
        self.processes.get(&pid).map(|p| p.borrow())
    }

    pub fn get_process_for_thread_mut(&self, tid: u32) -> Option<RefMut<'_, ProcessState>> {
        let pid = self.threads.get(&tid)?.borrow().process_id;
        self.processes.get(&pid).map(|p| p.borrow_mut())
    }

    pub fn get_thread(&self, tid: u32) -> Option<Ref<'_, ThreadState>> {
        self.threads.get(&tid).map(|p| p.borrow())
    }

    pub fn get_thread_mut(&self, tid: u32) -> Option<RefMut<'_, ThreadState>> {
        self.threads.get(&tid).map(|p| p.borrow_mut())
    }

    pub fn get_thread_handle(&self, tid: u32) -> Option<ThreadHandle> {
        self.get_thread(tid).map(|t| t.handle)
    }

    pub fn set_thread_name(&self, tid: u32, name: &str) {
        if let Some(mut thread) = self.get_thread_mut(tid) {
            self.profile
                .borrow_mut()
                .set_thread_name(thread.handle, name);
            thread.merge_name = Some(name.to_string());
        }
    }

    pub fn get_or_create_memory_usage_counter(&mut self, tid: u32) -> Option<CounterHandle> {
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
    pub fn add_sample(
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
        let thread = self.get_thread(tid).unwrap().handle;
        let mut process = self.get_process_mut(pid).unwrap();
        process.unresolved_samples.add_sample(
            thread,
            timestamp,
            timestamp_raw,
            stack_index,
            cpu_delta,
            weight,
            None,
        );
    }

    pub fn unresolved_stack_handle_for_stack(&self, stack: &[StackFrame]) -> UnresolvedStackHandle {
        self.unresolved_stacks
            .borrow_mut()
            .convert(stack.iter().rev().cloned())
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

    pub fn is_interesting_process(&self, pid: u32, ppid: Option<u32>, name: Option<&str>) -> bool {
        if pid == 0 {
            return false;
        }

        // already tracking this process or its parent?
        if self.processes.contains_key(&pid)
            || ppid.is_some_and(|k| self.processes.contains_key(&k))
        {
            return true;
        }

        match &self.included_processes {
            Some(incl) => incl.should_include(name, pid),
            None => true,
        }
    }

    fn add_kernel_drivers(&mut self) {
        for (path, start_avma, end_avma) in winutils::iter_kernel_drivers() {
            let path = self.map_device_path(&path);
            log::info!("kernel driver: {} {:x} {:x}", path, start_avma, end_avma);
            let lib_info = self.get_library_info_for_path(&path);
            let lib_handle = self.profile.borrow_mut().add_lib(lib_info);
            self.profile
                .borrow_mut()
                .add_kernel_lib_mapping(lib_handle, start_avma, end_avma, 0);
        }
    }

    pub fn stack_mode_for_address(&self, address: u64) -> StackMode {
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
    pub fn map_device_path(&self, path: &str) -> String {
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

    pub fn add_thread_marker(
        &self,
        thread_id: u32,
        timestamp: Timestamp,
        category: CategoryHandle,
        name: &str,
        marker: impl ProfilerMarker,
    ) -> MarkerHandle {
        let timing = MarkerTiming::Instant(timestamp);
        let thread_handle = self.get_thread_handle(thread_id).unwrap();
        self.profile
            .borrow_mut()
            .add_marker(thread_handle, category, name, marker, timing)
    }

    pub fn get_coreclr_context(&self) -> RefMut<CoreClrContext> {
        self.coreclr_context.borrow_mut()
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
