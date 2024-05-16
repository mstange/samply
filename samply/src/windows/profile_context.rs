use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CounterHandle, CpuDelta, LibraryHandle, LibraryInfo,
    MarkerDynamicField, MarkerFieldFormat, MarkerHandle, MarkerLocation, MarkerSchema,
    MarkerSchemaField, MarkerTiming, ProcessHandle, Profile, ProfilerMarker, SamplingInterval,
    Symbol, SymbolTable, ThreadHandle, Timestamp,
};
use serde_json::{json, Value};
use uuid::Uuid;

use super::chrome_etw_flags::KeywordNames;
use super::winutils;
use crate::shared::context_switch::{
    ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData,
};
use crate::shared::included_processes::IncludedProcesses;
use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::jit_function_add_marker::JitFunctionAddMarker;
use crate::shared::lib_mappings::{LibMappingAdd, LibMappingInfo, LibMappingOp, LibMappingOpQueue};
use crate::shared::process_sample_data::{ProcessSampleData, SimpleMarker, UserTimingMarker};
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{UnresolvedSamples, UnresolvedStacks};
use crate::windows::firefox_etw_flags::{
    PHASE_INSTANT, PHASE_INTERVAL, PHASE_INTERVAL_END, PHASE_INTERVAL_START,
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
    profile: RefCell<Profile>,

    // state -- keep track of the processes etc we've seen as we're processing,
    // and their associated handles in the json profile
    processes: HashMap<u32, RefCell<ProcessState>>,
    threads: HashMap<u32, RefCell<ThreadState>>,
    process_jit_infos: HashMap<u32, RefCell<ProcessJitInfo>>,

    unresolved_stacks: RefCell<UnresolvedStacks>,

    // track VM alloc/frees per thread? counter may be inaccurate because memory
    // can be allocated on one thread and freed on another
    per_thread_memory: bool,

    // some special threads
    gpu_thread_handle: Option<ThreadHandle>,

    libs_with_pending_debugid: HashMap<(u32, u64), (String, u32, u32)>,
    kernel_pending_libraries: HashMap<u64, LibraryInfo>,

    // These are the processes + their descendants that we want to write into
    // the profile.json. If it's None, include everything.
    included_processes: Option<IncludedProcesses>,

    // default categories
    categories: RefCell<HashMap<KnownCategory, CategoryHandle>>,

    js_category_manager: RefCell<JitCategoryManager>,
    context_switch_handler: RefCell<ContextSwitchHandler>,

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

    sample_count: usize,
    stack_sample_count: usize,
    event_count: usize,

    timestamp_converter: TimestampConverter,
    event_timestamps_are_qpc: bool,
}

impl ProfileContext {
    pub fn new(
        profile: Profile,
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
            processes: HashMap::new(),
            threads: HashMap::new(),
            process_jit_infos: HashMap::new(),
            unresolved_stacks: RefCell::new(UnresolvedStacks::default()),
            gpu_thread_handle: None,
            per_thread_memory: false,
            libs_with_pending_debugid: HashMap::new(),
            kernel_pending_libraries: HashMap::new(),
            included_processes,
            categories: RefCell::new(HashMap::new()),
            js_category_manager: RefCell::new(JitCategoryManager::new()),
            context_switch_handler: RefCell::new(ContextSwitchHandler::new(122100)), // hardcoded, but replaced once TraceStart is received
            device_mappings: winutils::get_dos_device_mappings(),
            kernel_min,
            arch: arch.to_string(),
            sample_count: 0,
            stack_sample_count: 0,
            event_count: 0,
            // Dummy, will be replaced once we see the header
            timestamp_converter: TimestampConverter {
                reference_raw: 0,
                raw_to_ns_factor: 1,
            },
            event_timestamps_are_qpc: false,
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

    pub fn is_arm64(&self) -> bool {
        self.arch == "arm64"
    }

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

    pub fn get_process_mut(&self, pid: u32) -> Option<RefMut<'_, ProcessState>> {
        self.processes.get(&pid).map(|p| p.borrow_mut())
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

    pub fn has_thread(&self, tid: u32) -> bool {
        self.threads.contains_key(&tid)
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

    #[allow(unused)]
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

    #[allow(unused)]
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

    pub fn add_thread_instant_marker(
        &self,
        timestamp_raw: u64,
        thread_id: u32,
        known_category: KnownCategory,
        name: &str,
        marker: impl ProfilerMarker,
    ) -> MarkerHandle {
        let category = self.get_category(known_category);
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let timing = MarkerTiming::Instant(timestamp);
        let thread_handle = self.get_thread_handle(thread_id).unwrap();
        self.profile
            .borrow_mut()
            .add_marker(thread_handle, category, name, marker, timing)
    }

    pub fn add_thread_interval_marker(
        &self,
        start_timestamp_raw: u64,
        end_timestamp_raw: u64,
        thread_id: u32,
        known_category: KnownCategory,
        name: &str,
        marker: impl ProfilerMarker,
    ) -> MarkerHandle {
        let category = self.get_category(known_category);
        let start_timestamp = self.timestamp_converter.convert_time(start_timestamp_raw);
        let end_timestamp = self.timestamp_converter.convert_time(end_timestamp_raw);
        let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
        let thread_handle = self.get_thread_handle(thread_id).unwrap();
        self.profile
            .borrow_mut()
            .add_marker(thread_handle, category, name, marker, timing)
    }

    pub fn handle_header(&mut self, timestamp_raw: u64, perf_freq: u64, clock_type: u32) {
        if clock_type != 1 {
            log::warn!("QPC not used as clock");
            self.event_timestamps_are_qpc = false;
        } else {
            self.event_timestamps_are_qpc = true;
        }

        self.timestamp_converter = TimestampConverter {
            reference_raw: timestamp_raw,
            raw_to_ns_factor: 1000 * 1000 * 1000 / perf_freq,
        };
    }

    pub fn handle_collection_start(&mut self, interval_raw: u32) {
        let interval_nanos = interval_raw as u64 * 100;
        let interval = SamplingInterval::from_nanos(interval_nanos);
        log::info!("Sample rate {}ms", interval.as_secs_f64() * 1000.);
        self.profile.borrow_mut().set_interval(interval);
        self.context_switch_handler
            .replace(ContextSwitchHandler::new(interval_raw as u64));
    }

    pub fn handle_thread_dcstart(
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        pid: u32,
        name: Option<String>,
    ) {
        if !self.is_interesting_process(pid, None, None) {
            return;
        }

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);

        // if there's an existing thread, remove it, assume we dropped an end thread event
        self.remove_thread(tid, Some(timestamp));
        self.add_thread(pid, tid, timestamp);

        if let Some(thread_name) = name {
            if !thread_name.is_empty() {
                self.set_thread_name(tid, &thread_name);
            }
        }
    }

    pub fn handle_thread_start(
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        pid: u32,
        name: Option<String>,
    ) {
        if !self.is_interesting_process(pid, None, None) {
            return;
        }

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);

        // if there's an existing thread, remove it, assume we dropped an end thread event
        self.remove_thread(tid, Some(timestamp));
        self.add_thread(pid, tid, timestamp);

        if let Some(thread_name) = name {
            if !thread_name.is_empty() {
                self.set_thread_name(tid, &thread_name);
            }
        }
    }

    pub fn handle_thread_set_name(&mut self, _timestamp_raw: u64, tid: u32, name: String) {
        if !name.is_empty() {
            self.set_thread_name(tid, &name);
        }
    }

    pub fn handle_thread_end(&mut self, timestamp_raw: u64, tid: u32) {
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.remove_thread(tid, Some(timestamp));
    }

    pub fn handle_thread_dcend(&mut self, timestamp_raw: u64, tid: u32) {
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.remove_thread(tid, Some(timestamp));
    }

    pub fn handle_process_dcstart(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        parent_pid: u32,
        image_file_name: String,
    ) {
        if !self.is_interesting_process(pid, Some(parent_pid), Some(&image_file_name)) {
            return;
        }

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.add_process(
            pid,
            parent_pid,
            &self.map_device_path(&image_file_name),
            timestamp,
        );
    }

    pub fn handle_process_start(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        parent_pid: u32,
        image_file_name: String,
    ) {
        if !self.is_interesting_process(pid, Some(parent_pid), Some(&image_file_name)) {
            return;
        }

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.add_process(
            pid,
            parent_pid,
            &self.map_device_path(&image_file_name),
            timestamp,
        );
    }

    pub fn handle_process_end(&mut self, _timestamp_raw: u64, _pid: u32) {
        // TODO
    }

    pub fn handle_process_dcend(&mut self, _timestamp_raw: u64, _pid: u32) {
        // TODO
    }

    /// Attach a stack to an existing marker.
    ///
    /// CoreCLR emits these stacks after the corresponding marker.
    pub fn handle_coreclr_stack(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        tid: u32,
        stack_address_iter: impl Iterator<Item = u64>,
        marker_handle: MarkerHandle,
    ) {
        let stack: Vec<StackFrame> = stack_address_iter
            .enumerate()
            .map(|(i, addr)| {
                if i == 0 {
                    StackFrame::InstructionPointer(addr, self.stack_mode_for_address(addr))
                } else {
                    StackFrame::ReturnAddress(addr, self.stack_mode_for_address(addr))
                }
            })
            .collect();

        let stack_index = self
            .unresolved_stacks
            .borrow_mut()
            .convert(stack.into_iter().rev());
        let thread_handle = self.get_thread(tid).unwrap().handle;
        //eprintln!("event: StackWalk stack: {:?}", stack);

        // Note: we don't add these as actual samples, and instead just attach them to the marker.
        // If we added them as samples, it would throw off the profile counting, because they arrive
        // in between regular interval samples. In the future, maybe we can support fractional samples
        // somehow (fractional weight), but for now, we just attach them to the marker.

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.get_process_mut(pid)
            .unwrap()
            .unresolved_samples
            .attach_stack_to_marker(
                thread_handle,
                timestamp,
                timestamp_raw,
                stack_index,
                marker_handle,
            );
    }

    pub fn handle_stack_arm64(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        tid: u32,
        stack_address_iter: impl Iterator<Item = u64>,
    ) {
        if !self.threads.contains_key(&tid) {
            return;
        }

        // On ARM64, this seems to be simpler -- stacks come in with full kernel and user frames.
        // At least, I've never seen a kernel stack come in separately.
        // TODO -- is this because I can't use PROFILE events in the VM?

        // Iterate over the stack addresses, starting with the instruction pointer
        let stack: Vec<StackFrame> = stack_address_iter
            .enumerate()
            .map(|(i, addr)| {
                if i == 0 {
                    StackFrame::InstructionPointer(addr, self.stack_mode_for_address(addr))
                } else {
                    StackFrame::ReturnAddress(addr, self.stack_mode_for_address(addr))
                }
            })
            .collect();

        let cpu_delta_raw = self
            .context_switch_handler
            .borrow_mut()
            .consume_cpu_delta(&mut self.get_thread_mut(tid).unwrap().context_switch_data);
        let cpu_delta =
            CpuDelta::from_nanos(cpu_delta_raw * self.timestamp_converter.raw_to_ns_factor);
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.add_sample(pid, tid, timestamp, timestamp_raw, cpu_delta, 1, stack);
    }

    pub fn handle_stack_x86(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        tid: u32,
        stack_len: usize,
        stack_address_iter: impl Iterator<Item = u64>,
    ) {
        let mut stack: Vec<StackFrame> = Vec::with_capacity(stack_len);
        let mut address_iter = stack_address_iter;
        let Some(first_frame_address) = address_iter.next() else {
            return;
        };
        let first_frame_stack_mode = self.stack_mode_for_address(first_frame_address);
        stack.push(StackFrame::InstructionPointer(
            first_frame_address,
            first_frame_stack_mode,
        ));
        for frame_address in address_iter {
            let stack_mode = self.stack_mode_for_address(frame_address);
            stack.push(StackFrame::ReturnAddress(frame_address, stack_mode));
        }

        if first_frame_stack_mode == StackMode::Kernel {
            let mut thread = self.get_thread_mut(tid).unwrap();
            if let Some(pending_stack) = thread
                .pending_stacks
                .iter_mut()
                .rev()
                .find(|s| s.timestamp == timestamp_raw)
            {
                if let Some(kernel_stack) = pending_stack.kernel_stack.as_mut() {
                    log::warn!(
                        "Multiple kernel stacks for timestamp {timestamp_raw} on thread {tid}"
                    );
                    kernel_stack.extend(&stack);
                } else {
                    pending_stack.kernel_stack = Some(stack);
                }
            }
            return;
        }

        // We now know that we have a user stack. User stacks always come last. Consume
        // the pending stack with matching timestamp.

        // the number of pending stacks at or before our timestamp
        let num_pending_stacks = self
            .get_thread(tid)
            .unwrap()
            .pending_stacks
            .iter()
            .take_while(|s| s.timestamp <= timestamp_raw)
            .count();

        let pending_stacks: VecDeque<_> = self
            .get_thread_mut(tid)
            .unwrap()
            .pending_stacks
            .drain(..num_pending_stacks)
            .collect();

        // Use this user stack for all pending stacks from this thread.
        for pending_stack in pending_stacks {
            let PendingStack {
                timestamp: timestamp_raw,
                kernel_stack,
                off_cpu_sample_group,
                on_cpu_sample_cpu_delta,
            } = pending_stack;
            let timestamp = self.timestamp_converter.convert_time(timestamp_raw);

            if let Some(off_cpu_sample_group) = off_cpu_sample_group {
                let OffCpuSampleGroup {
                    begin_timestamp: begin_timestamp_raw,
                    end_timestamp: end_timestamp_raw,
                    sample_count,
                } = off_cpu_sample_group;

                let cpu_delta_raw = {
                    let mut thread = self.get_thread_mut(tid).unwrap();
                    self.context_switch_handler
                        .borrow_mut()
                        .consume_cpu_delta(&mut thread.context_switch_data)
                };
                let cpu_delta =
                    CpuDelta::from_nanos(cpu_delta_raw * self.timestamp_converter.raw_to_ns_factor);

                // Add a sample at the beginning of the paused range.
                // This "first sample" will carry any leftover accumulated running time ("cpu delta").
                let begin_timestamp = self.timestamp_converter.convert_time(begin_timestamp_raw);
                self.add_sample(
                    pid,
                    tid,
                    begin_timestamp,
                    begin_timestamp_raw,
                    cpu_delta,
                    1,
                    stack.clone(),
                );

                if sample_count > 1 {
                    // Emit a "rest sample" with a CPU delta of zero covering the rest of the paused range.
                    let weight = i32::try_from(sample_count - 1).unwrap_or(0);
                    let end_timestamp = self.timestamp_converter.convert_time(end_timestamp_raw);
                    self.add_sample(
                        pid,
                        tid,
                        end_timestamp,
                        end_timestamp_raw,
                        CpuDelta::ZERO,
                        weight,
                        stack.clone(),
                    );
                }
            }

            if let Some(cpu_delta) = on_cpu_sample_cpu_delta {
                if let Some(mut combined_stack) = kernel_stack {
                    combined_stack.extend_from_slice(&stack[..]);
                    self.add_sample(
                        pid,
                        tid,
                        timestamp,
                        timestamp_raw,
                        cpu_delta,
                        1,
                        combined_stack,
                    );
                } else {
                    self.add_sample(
                        pid,
                        tid,
                        timestamp,
                        timestamp_raw,
                        cpu_delta,
                        1,
                        stack.clone(),
                    );
                }
                self.stack_sample_count += 1;
            }
        }
    }

    pub fn handle_sample(&mut self, timestamp_raw: u64, tid: u32) {
        self.sample_count += 1;

        let Some(mut thread) = self.get_thread_mut(tid) else {
            return;
        };

        let off_cpu_sample_group = self
            .context_switch_handler
            .borrow_mut()
            .handle_on_cpu_sample(timestamp_raw, &mut thread.context_switch_data);
        let delta = self
            .context_switch_handler
            .borrow_mut()
            .consume_cpu_delta(&mut thread.context_switch_data);
        let cpu_delta = CpuDelta::from_nanos(delta * self.timestamp_converter.raw_to_ns_factor);
        thread.pending_stacks.push_back(PendingStack {
            timestamp: timestamp_raw,
            kernel_stack: None,
            off_cpu_sample_group,
            on_cpu_sample_cpu_delta: Some(cpu_delta),
        });
    }

    pub fn handle_virtual_alloc_free(
        &mut self,
        timestamp_raw: u64,
        is_free: bool,
        pid: u32,
        tid: u32,
        region_size: u64,
        stringified_properties: String,
    ) {
        if !self.is_interesting_process(pid, None, None) {
            return;
        }

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let delta_size = if is_free {
            -(region_size as f64)
        } else {
            region_size as f64
        };
        let op_name = if is_free {
            "VirtualFree"
        } else {
            "VirtualAlloc"
        };

        let Some(memory_usage_counter) = self.get_or_create_memory_usage_counter(tid) else {
            return;
        };
        let Some(thread_handle) = self.get_thread(tid).map(|t| t.handle) else {
            return;
        };
        self.profile.borrow_mut().add_counter_sample(
            memory_usage_counter,
            timestamp,
            delta_size,
            1,
        );
        self.profile.borrow_mut().add_marker(
            thread_handle,
            CategoryHandle::OTHER,
            op_name,
            SimpleMarker(stringified_properties),
            MarkerTiming::Instant(timestamp),
        );
    }

    pub fn handle_image_id(
        &mut self,
        pid: u32,
        image_base: u64,
        image_timestamp: u32,
        image_size: u32,
        image_path: String,
    ) {
        if !self.is_interesting_process(pid, None, None) && pid != 0 {
            return;
        }
        //eprintln!("ImageID pid: {} 0x{:x} {} {} {}", pid, image_base, image_path, image_size, image_timestamp);
        // "libs" is used as a cache to store the image path and size until we get the DbgID_RSDS event
        if self
            .libs_with_pending_debugid
            .contains_key(&(pid, image_base))
        {
            // I see odd entries like this with no corresponding DbgID_RSDS:
            //   ImageID pid: 3156 0xf70000 com.docker.cli.exe 49819648 0
            // they all come from something docker-related. So don't panic on the duplicate.
            //panic!("libs_with_pending_debugid already contains key 0x{:x} for pid {}", image_base, pid);
        }
        self.libs_with_pending_debugid
            .insert((pid, image_base), (image_path, image_size, image_timestamp));
    }

    pub fn handle_image_debug_id(
        &mut self,
        pid: u32,
        image_base: u64,
        debug_id: DebugId,
        pdb_path: String,
    ) {
        if !self.is_interesting_process(pid, None, None) && pid != 0 {
            return;
        }

        //let pdb_path = Path::new(&pdb_path);
        let Some((ref path, image_size, timestamp)) =
            self.libs_with_pending_debugid.remove(&(pid, image_base))
        else {
            log::warn!(
                "DbID_RSDS for image at 0x{:x} for pid {}, but has no entry in libs",
                image_base,
                pid
            );
            return;
        };
        //eprintln!("DbgID_RSDS pid: {} 0x{:x} {} {} {} {}", pid, image_base, path, debug_id, pdb_path, age);
        let code_id = Some(format!("{timestamp:08X}{image_size:x}"));
        let name = Path::new(path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let debug_name = Path::new(&pdb_path)
            .file_name()
            .map(|f| f.to_str().unwrap().to_owned())
            .unwrap_or(name.clone());
        let info = LibraryInfo {
            name,
            debug_name,
            path: path.clone(),
            code_id,
            symbol_table: None,
            debug_path: pdb_path,
            debug_id,
            arch: Some(self.arch.to_owned()),
        };
        if pid == 0 || image_base >= self.kernel_min {
            if let Some(oldinfo) = self.kernel_pending_libraries.get(&image_base) {
                assert_eq!(*oldinfo, info);
            } else {
                self.kernel_pending_libraries.insert(image_base, info);
            }
        } else if let Some(mut process) = self.get_process_mut(pid) {
            process.pending_libraries.insert(image_base, info);
        } else {
            log::warn!("No process for pid {pid}");
        }
    }

    pub fn handle_image_load(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        image_base: u64,
        image_size: u64,
        path: String,
    ) {
        // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized from MSNT_SystemTrace/Image/Load
        // but don't contain the full path of the binary. We go through a bit of a dance to store the information from those events
        // in pending_libraries and deal with it here. We assume that the KernelTraceControl events come before the Image/Load event.

        // the ProcessId field doesn't necessarily match s.process_id();
        if !self.is_interesting_process(pid, None, None) && pid != 0 {
            return;
        }

        let path = self.map_device_path(&path);

        let info = if pid == 0 {
            self.kernel_pending_libraries.remove(&image_base)
        } else if let Some(mut process) = self.get_process_mut(pid) {
            process.pending_libraries.remove(&image_base)
        } else {
            log::warn!("Received image load for unknown pid {pid}");
            return;
        };

        // If the file doesn't exist on disk we won't have KernelTraceControl/ImageID events
        // This happens for the ghost drivers mentioned here: https://devblogs.microsoft.com/oldnewthing/20160913-00/?p=94305
        let Some(mut info) = info else { return };

        info.path = path;

        // attempt to categorize the library based on the path
        let path_lower = info.path.to_lowercase();
        let debug_path_lower = info.debug_path.to_lowercase();

        let known_category = if debug_path_lower.contains(".ni.pdb") {
            KnownCategory::CoreClrR2r
        } else if path_lower.contains("windows\\system32") || path_lower.contains("windows\\winsxs")
        {
            KnownCategory::System
        } else {
            KnownCategory::Unknown
        };

        let lib_handle = self.profile.borrow_mut().add_lib(info);
        if pid == 0 || image_base >= self.kernel_min {
            self.profile.borrow_mut().add_kernel_lib_mapping(
                lib_handle,
                image_base,
                image_base + image_size,
                0,
            );
            return;
        }

        let info = if known_category != KnownCategory::Unknown {
            let category = self.get_category(known_category);
            LibMappingInfo::new_lib_with_category(lib_handle, category.into())
        } else {
            LibMappingInfo::new_lib(lib_handle)
        };

        self.get_process_mut(pid)
            .unwrap()
            .regular_lib_mapping_ops
            .push(
                timestamp_raw,
                LibMappingOp::Add(LibMappingAdd {
                    start_avma: image_base,
                    end_avma: image_base + image_size,
                    relative_address_at_start: 0,
                    info,
                }),
            );
    }

    pub fn handle_vsync(&mut self, timestamp_raw: u64) {
        #[derive(Debug, Clone)]
        pub struct VSyncMarker;

        impl ProfilerMarker for VSyncMarker {
            const MARKER_TYPE_NAME: &'static str = "Vsync";

            fn json_marker_data(&self) -> Value {
                json!({
                    "type": Self::MARKER_TYPE_NAME,
                    "name": ""
                })
            }

            fn schema() -> MarkerSchema {
                MarkerSchema {
                    type_name: Self::MARKER_TYPE_NAME,
                    locations: vec![
                        MarkerLocation::MarkerChart,
                        MarkerLocation::MarkerTable,
                        MarkerLocation::TimelineOverview,
                    ],
                    chart_label: Some("{marker.data.name}"),
                    tooltip_label: None,
                    table_label: Some("{marker.name} - {marker.data.name}"),
                    fields: vec![MarkerSchemaField::Dynamic(MarkerDynamicField {
                        key: "name",
                        label: "Details",
                        format: MarkerFieldFormat::String,
                        searchable: false,
                    })],
                }
            }
        }

        let gpu_thread = self.gpu_thread_handle.get_or_insert_with(|| {
            let start_timestamp = Timestamp::from_nanos_since_reference(0);
            let gpu = self
                .profile
                .borrow_mut()
                .add_process("GPU", 1, start_timestamp);
            self.profile
                .borrow_mut()
                .add_thread(gpu, 1, start_timestamp, false)
        });
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.profile.borrow_mut().add_marker(
            *gpu_thread,
            CategoryHandle::OTHER,
            "Vsync",
            VSyncMarker {},
            MarkerTiming::Instant(timestamp),
        );
    }

    pub fn handle_cswitch(&mut self, timestamp_raw: u64, old_tid: u32, new_tid: u32) {
        // println!("CSwitch {} -> {} @ {} on {}", old_tid, old_tid, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });

        if let Some(mut old_thread) = self.get_thread_mut(old_tid) {
            self.context_switch_handler
                .borrow_mut()
                .handle_switch_out(timestamp_raw, &mut old_thread.context_switch_data);
        }
        if let Some(mut new_thread) = self.get_thread_mut(new_tid) {
            let off_cpu_sample_group = self
                .context_switch_handler
                .borrow_mut()
                .handle_switch_in(timestamp_raw, &mut new_thread.context_switch_data);
            if let Some(off_cpu_sample_group) = off_cpu_sample_group {
                new_thread.pending_stacks.push_back(PendingStack {
                    timestamp: timestamp_raw,
                    kernel_stack: None,
                    off_cpu_sample_group: Some(off_cpu_sample_group),
                    on_cpu_sample_cpu_delta: None,
                });
            }
        }
    }

    pub fn handle_js_method_load(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        method_name: String,
        method_start_address: u64,
        method_size: u64,
    ) {
        if !self.is_interesting_process(pid, None, None) && pid != 0 {
            return;
        }

        self.ensure_process_jit_info(pid);
        let Some(process) = self.get_process_mut(pid) else {
            return;
        };
        let mut process_jit_info = self.get_process_jit_info(pid);

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let relative_address = process_jit_info.next_relative_address;
        process_jit_info.next_relative_address += method_size as u32;

        if let Some(main_thread) = process.main_thread_handle {
            self.profile.borrow_mut().add_marker(
                main_thread,
                CategoryHandle::OTHER,
                "JitFunctionAdd",
                JitFunctionAddMarker(method_name.clone()),
                MarkerTiming::Instant(timestamp),
            );
        }

        let (category, js_frame) = self
            .js_category_manager
            .borrow_mut()
            .classify_jit_symbol(&method_name, &mut self.profile.borrow_mut());
        let info =
            LibMappingInfo::new_jit_function(process_jit_info.lib_handle, category, js_frame);
        process_jit_info.jit_mapping_ops.push(
            timestamp_raw,
            LibMappingOp::Add(LibMappingAdd {
                start_avma: method_start_address,
                end_avma: method_start_address + method_size,
                relative_address_at_start: relative_address,
                info,
            }),
        );
        process_jit_info.symbols.push(Symbol {
            address: relative_address,
            size: Some(method_size as u32),
            name: method_name,
        });
    }

    pub fn handle_coreclr_method_load(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        method_name: String,
        method_start_address: u64,
        method_size: u32,
    ) {
        self.ensure_process_jit_info(pid);
        let mut process_jit_info = self.get_process_jit_info(pid);
        let relative_address = process_jit_info.next_relative_address;
        process_jit_info.next_relative_address += method_size;

        // Not that useful for CoreCLR
        //let mh = context.add_thread_marker(s.thread_id(), timestamp, CategoryHandle::OTHER, "JitFunctionAdd", JitFunctionAddMarker(method_name.to_owned()));
        //core_clr_context.set_last_event_for_thread(thread_handle, mh);

        let category = self.get_category(KnownCategory::CoreClrJit);
        let info =
            LibMappingInfo::new_jit_function(process_jit_info.lib_handle, category.into(), None);
        process_jit_info.jit_mapping_ops.push(
            timestamp_raw,
            LibMappingOp::Add(LibMappingAdd {
                start_avma: method_start_address,
                end_avma: method_start_address + u64::from(method_size),
                relative_address_at_start: relative_address,
                info,
            }),
        );
        process_jit_info.symbols.push(Symbol {
            address: relative_address,
            size: Some(method_size),
            name: method_name,
        });
    }

    pub fn handle_freeform_marker_start(
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        name: &str,
        stringified_properties: String,
    ) {
        let Some(mut thread) = self.get_thread_mut(tid) else {
            return;
        };
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        thread.pending_markers.insert(
            name.to_owned(),
            PendingMarker {
                text: stringified_properties,
                start: timestamp,
            },
        );
    }

    pub fn handle_freeform_marker_end(
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        name: &str,
        stringified_properties: String,
        known_category: KnownCategory,
    ) {
        let Some(mut thread) = self.get_thread_mut(tid) else {
            return;
        };

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);

        // Combine the start and end markers into a single marker.
        // Alternatively, we could output the start marker with IntervalStart, however this has one big drawback:
        // The text stored in the start marker would not be available in the UI!
        // The Firefox Profiler combines IntervalStart and IntervalEnd marker into a single marker
        // whose data is taken only from the *end* marker.
        // So here we manually merge them, taking the data from the *start* marker.
        let (timing, text) = if let Some(pending) = thread.pending_markers.remove(name) {
            (
                MarkerTiming::Interval(pending.start, timestamp),
                pending.text,
            )
        } else {
            (MarkerTiming::IntervalEnd(timestamp), stringified_properties)
        };

        let category = self.get_category(known_category);
        self.profile.borrow_mut().add_marker(
            thread.handle,
            category,
            name.split_once('/').unwrap().1,
            SimpleMarker(text),
            timing,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_firefox_marker(
        &mut self,
        tid: u32,
        marker_name: &str,
        start_time_qpc: u64,
        end_time_qpc: u64,
        phase: Option<u8>,
        maybe_user_timing_name: Option<String>,
        maybe_explicit_marker_name: Option<String>,
        text: String,
    ) {
        let Some(thread) = self.get_thread(tid) else {
            return;
        };

        assert!(self.event_timestamps_are_qpc, "Inconsistent timestamp formats! ETW traces with Firefox events should be captured with QPC timestamps (-ClockType PerfCounter) so that ETW sample timestamps are compatible with the QPC timestamps in Firefox ETW trace events, so that the markers appear in the right place.");
        let (phase, instant_time_qpc): (u8, u64) = match phase {
            Some(phase) => (phase, start_time_qpc),
            None => {
                // Before the landing of https://bugzilla.mozilla.org/show_bug.cgi?id=1882640 ,
                // Firefox ETW trace events didn't have phase information, so we need to
                // guess a phase based on the timestamps.
                if start_time_qpc != 0 && end_time_qpc != 0 {
                    (PHASE_INTERVAL, 0)
                } else if start_time_qpc != 0 {
                    (PHASE_INSTANT, start_time_qpc)
                } else {
                    (PHASE_INSTANT, end_time_qpc)
                }
            }
        };
        let timing = match phase {
            PHASE_INSTANT => {
                MarkerTiming::Instant(self.timestamp_converter.convert_time(instant_time_qpc))
            }
            PHASE_INTERVAL => MarkerTiming::Interval(
                self.timestamp_converter.convert_time(start_time_qpc),
                self.timestamp_converter.convert_time(end_time_qpc),
            ),
            PHASE_INTERVAL_START => {
                MarkerTiming::IntervalStart(self.timestamp_converter.convert_time(start_time_qpc))
            }
            PHASE_INTERVAL_END => {
                MarkerTiming::IntervalEnd(self.timestamp_converter.convert_time(end_time_qpc))
            }
            _ => panic!("Unexpected marker phase {phase}"),
        };

        if marker_name == "UserTiming" {
            let name = maybe_user_timing_name.unwrap();
            self.profile.borrow_mut().add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                "UserTiming",
                UserTimingMarker(name),
                timing,
            );
        } else if marker_name == "SimpleMarker" || marker_name == "Text" || marker_name == "tracing"
        {
            let marker_name = maybe_explicit_marker_name.unwrap();
            self.profile.borrow_mut().add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                &marker_name,
                SimpleMarker(text.clone()),
                timing,
            );
        } else {
            self.profile.borrow_mut().add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                marker_name,
                SimpleMarker(text.clone()),
                timing,
            );
        }
    }

    pub fn handle_chrome_marker(
        &mut self,
        tid: u32,
        marker_name: &str,
        timestamp_raw: u64,
        phase: &str,
        keyword_bitfield: u64,
        text: String,
    ) {
        let Some(thread) = self.get_thread(tid) else {
            return;
        };

        let timestamp = self.timestamp_converter.convert_us(timestamp_raw);

        let timing = match phase {
            "Begin" => MarkerTiming::IntervalStart(timestamp),
            "End" => MarkerTiming::IntervalEnd(timestamp),
            _ => MarkerTiming::Instant(timestamp),
        };
        let keyword = KeywordNames::from_bits(keyword_bitfield).unwrap();
        if keyword == KeywordNames::blink_user_timing {
            self.profile.borrow_mut().add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                "UserTiming",
                UserTimingMarker(marker_name.to_owned()),
                timing,
            );
        } else {
            self.profile.borrow_mut().add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                marker_name,
                SimpleMarker(text.clone()),
                timing,
            );
        }
    }
    pub fn handle_unknown_event(
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        task_and_op: &str,
        stringified_properties: String,
    ) {
        let Some(thread) = self.get_thread(tid) else {
            return;
        };

        let timestamp = self.timestamp_converter.convert_us(timestamp_raw);
        let timing = MarkerTiming::Instant(timestamp);
        // this used to create a new category based on provider_name, just lump them together for now
        let category = self.get_category(KnownCategory::Unknown);
        self.profile.borrow_mut().add_marker(
            thread.handle,
            category,
            task_and_op,
            SimpleMarker(stringified_properties),
            timing,
        );
        //println!("unhandled {}", s.name())
    }

    pub fn finish(mut self) -> Profile {
        // Push queued samples into the profile.
        // We queue them so that we can get symbolicated JIT function names. To get symbolicated JIT function names,
        // we have to call profile.add_sample after we call profile.set_lib_symbol_table, and we don't have the
        // complete JIT symbol table before we've seen all JIT symbols.
        // (This is a rather weak justification. The better justification is that this is consistent with what
        // samply does on Linux and macOS, where the queued samples also want to respect JIT function names from
        // a /tmp/perf-1234.map file, and this file may not exist until the profiled process finishes.)
        let mut stack_frame_scratch_buf = Vec::new();
        for (process_id, process) in self.processes.iter() {
            let process = process.borrow_mut();
            let jitdump_lib_mapping_op_queues = match self.process_jit_infos.remove(process_id) {
                Some(jit_info) => {
                    let jit_info = jit_info.into_inner();
                    self.profile.borrow_mut().set_lib_symbol_table(
                        jit_info.lib_handle,
                        Arc::new(SymbolTable::new(jit_info.symbols)),
                    );
                    vec![jit_info.jit_mapping_ops]
                }
                None => Vec::new(),
            };

            let process_sample_data = ProcessSampleData::new(
                process.unresolved_samples.clone(),
                process.regular_lib_mapping_ops.clone(),
                jitdump_lib_mapping_op_queues,
                None,
                Vec::new(),
            );
            //main_thread_handle.unwrap_or_else(|| panic!("process no main thread {:?}", process_id)));
            let user_category = self.get_category(KnownCategory::User).into();
            let kernel_category = self.get_category(KnownCategory::Kernel).into();
            process_sample_data.flush_samples_to_profile(
                &mut self.profile.borrow_mut(),
                user_category,
                kernel_category,
                &mut stack_frame_scratch_buf,
                &self.unresolved_stacks.borrow(),
            )
        }

        /*if merge_threads {
            profile.add_thread(global_thread);
        } else {
            for (_, thread) in threads.drain() { profile.add_thread(thread.builder); }
        }*/

        log::info!(
            "{} events, {} samples, {} stack-samples",
            self.event_count,
            self.sample_count,
            self.stack_sample_count
        );

        self.profile.into_inner()
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
