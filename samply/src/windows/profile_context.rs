use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use debugid::DebugId;
use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CounterHandle, CpuDelta, Frame, FrameFlags, FrameInfo,
    LibraryHandle, LibraryInfo, MarkerDynamicField, MarkerFieldFormat, MarkerHandle,
    MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerTiming, ProcessHandle, Profile,
    ProfilerMarker, SamplingInterval, Symbol, SymbolTable, ThreadHandle, Timestamp,
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
use crate::shared::jit_function_recycler::JitFunctionRecycler;
use crate::shared::lib_mappings::{LibMappingAdd, LibMappingInfo, LibMappingOp, LibMappingOpQueue};
use crate::shared::process_sample_data::{ProcessSampleData, SimpleMarker, UserTimingMarker};
use crate::shared::recording_props::ProfileCreationProps;
use crate::shared::recycling::{ProcessRecycler, ProcessRecyclingData, ThreadRecycler};
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
    pub name: Option<String>,
    pub is_main_thread: bool,
    pub handle: ThreadHandle,
    pub label_frame: FrameInfo,
    pub pending_stacks: VecDeque<PendingStack>,
    pub context_switch_data: ThreadContextSwitchData,
    pub thread_id: u32,
    pub process_id: u32,
    pub pending_markers: HashMap<String, PendingMarker>,
}

impl ThreadState {
    fn new(
        name: Option<String>,
        is_main_thread: bool,
        handle: ThreadHandle,
        label_frame: FrameInfo,
        pid: u32,
        tid: u32,
    ) -> Self {
        ThreadState {
            name,
            is_main_thread,
            handle,
            label_frame,
            pending_stacks: VecDeque::new(),
            context_switch_data: Default::default(),
            pending_markers: HashMap::new(),
            thread_id: tid,
            process_id: pid,
        }
    }
}

pub struct ProcessState {
    pub name: String,
    pub handle: ProcessHandle,
    pub seen_main_thread_start: bool,
    pub unresolved_samples: UnresolvedSamples,
    pub regular_lib_mapping_ops: LibMappingOpQueue,
    pub main_thread_handle: ThreadHandle,
    pub main_thread_label_frame: FrameInfo,
    pub pending_libraries: HashMap<u64, LibraryInfo>,
    pub memory_usage: Option<MemoryUsage>,
    pub process_id: u32,
    pub parent_id: u32,
    pub jit_info: Option<ProcessJitInfo>,
    pub thread_recycler: Option<ThreadRecycler>,
    pub jit_function_recycler: Option<JitFunctionRecycler>,
}

impl ProcessState {
    pub fn get_jit_info(&mut self, profile: &mut Profile) -> &mut ProcessJitInfo {
        self.jit_info.get_or_insert_with(|| {
            let jitname = format!("JIT-{}", self.process_id);
            let jitlib = profile.add_lib(LibraryInfo {
                name: jitname.clone(),
                debug_name: jitname.clone(),
                path: jitname.clone(),
                debug_path: jitname.clone(),
                debug_id: DebugId::nil(),
                code_id: None,
                arch: None,
                symbol_table: None,
            });
            ProcessJitInfo {
                lib_handle: jitlib,
                jit_mapping_ops: LibMappingOpQueue::default(),
                next_relative_address: 0,
                symbols: Vec::new(),
            }
        })
    }

    pub fn take_recycling_data(&mut self) -> Option<ProcessRecyclingData> {
        Some(ProcessRecyclingData {
            process_handle: self.handle,
            main_thread_recycling_data: (
                self.main_thread_handle,
                self.main_thread_label_frame.clone(),
            ),
            thread_recycler: self.thread_recycler.take()?,
            jit_function_recycler: self.jit_function_recycler.take()?,
        })
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

struct KnownCategories(HashMap<KnownCategory, CategoryHandle>);

impl KnownCategories {
    pub fn new() -> Self {
        Self(HashMap::new())
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

    pub fn get(&mut self, category: KnownCategory, profile: &mut Profile) -> CategoryHandle {
        let category = if category == KnownCategory::Default {
            KnownCategory::User
        } else {
            category
        };

        *self.0.entry(category).or_insert_with(|| {
            let (category_name, color) = Self::CATEGORIES
                .iter()
                .find(|(c, _, _)| *c == category)
                .map(|(_, name, color)| (*name, *color))
                .unwrap();
            profile.add_category(category_name, color)
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct AddressClassifier {
    kernel_min: u64,
}

impl AddressClassifier {
    pub fn get_stack_mode(&self, address: u64) -> StackMode {
        if address >= self.kernel_min {
            StackMode::Kernel
        } else {
            StackMode::User
        }
    }
}

pub struct ProfileContext {
    profile: Profile,

    profile_creation_props: ProfileCreationProps,

    // state -- keep track of the processes etc we've seen as we're processing,
    // and their associated handles in the json profile
    processes: HashMap<u32, ProcessState>,
    dead_processes_with_reused_pids: Vec<ProcessState>,
    threads: HashMap<u32, ThreadState>,
    dead_threads_with_reused_tids: Vec<ThreadState>,

    unresolved_stacks: UnresolvedStacks,

    /// Some() if a thread should be merged into a previously exited
    /// thread of the same name.
    process_recycler: Option<ProcessRecycler>,

    // some special threads
    gpu_thread_handle: Option<ThreadHandle>,

    libs_with_pending_debugid: HashMap<(u32, u64), (String, u32, u32)>,
    kernel_pending_libraries: HashMap<u64, LibraryInfo>,

    // These are the processes + their descendants that we want to write into
    // the profile.json. If it's None, include everything.
    included_processes: Option<IncludedProcesses>,

    categories: KnownCategories,

    js_category_manager: JitCategoryManager,
    context_switch_handler: ContextSwitchHandler,

    // cache of device mappings
    device_mappings: HashMap<String, String>, // map of \Device\HarddiskVolume4 -> C:\

    // the minimum address for kernel drivers, so that we can assign kernel_category to the frame
    kernel_min: u64,

    address_classifier: AddressClassifier,

    // architecture to record in the trace. will be the system architecture for now.
    // TODO no idea how to handle "I'm on aarch64 windows but I'm recording a win64 process".
    // I have no idea how stack traces work in that case anyway, so this is probably moot.
    arch: String,

    sample_count: usize,
    stack_sample_count: usize,
    event_count: usize,

    timestamp_converter: TimestampConverter,
    event_timestamps_are_qpc: bool,

    /// Only include main threads.
    main_thread_only: bool,
}

impl ProfileContext {
    pub fn new(
        profile: Profile,
        arch: &str,
        included_processes: Option<IncludedProcesses>,
        profile_creation_props: ProfileCreationProps,
    ) -> Self {
        // On 64-bit systems, the kernel address space always has 0xF in the first 16 bits.
        // The actual kernel address space is much higher, but we just need this to disambiguate kernel and user
        // stacks. Use add_kernel_drivers to get accurate mappings.
        let kernel_min: u64 = if arch == "x86" {
            0x8000_0000
        } else {
            0xF000_0000_0000_0000
        };
        let address_classifier = AddressClassifier { kernel_min };
        let process_recycler = if profile_creation_props.reuse_threads {
            Some(ProcessRecycler::new())
        } else {
            None
        };
        let main_thread_only = profile_creation_props.main_thread_only;

        Self {
            profile,
            profile_creation_props,
            processes: HashMap::new(),
            dead_processes_with_reused_pids: Vec::new(),
            threads: HashMap::new(),
            dead_threads_with_reused_tids: Vec::new(),
            unresolved_stacks: UnresolvedStacks::default(),
            process_recycler,
            gpu_thread_handle: None,
            libs_with_pending_debugid: HashMap::new(),
            kernel_pending_libraries: HashMap::new(),
            included_processes,
            categories: KnownCategories::new(),
            js_category_manager: JitCategoryManager::new(),
            context_switch_handler: ContextSwitchHandler::new(122100), // hardcoded, but replaced once TraceStart is received
            device_mappings: winutils::get_dos_device_mappings(),
            kernel_min,
            address_classifier,
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
            main_thread_only,
        }
    }

    pub fn creation_props(&self) -> ProfileCreationProps {
        self.profile_creation_props.clone()
    }

    pub fn is_arm64(&self) -> bool {
        self.arch == "arm64"
    }

    pub fn has_thread(&self, tid: u32) -> bool {
        self.threads.contains_key(&tid)
    }

    pub fn get_or_create_memory_usage_counter(&mut self, pid: u32) -> Option<CounterHandle> {
        let process = self.processes.get_mut(&pid)?;
        let process_handle = process.handle;
        let memory_usage = process.memory_usage.get_or_insert_with(|| {
            let counter = self.profile.add_counter(
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
            let lib_handle = self.profile.add_lib(lib_info);
            self.profile
                .add_kernel_lib_mapping(lib_handle, start_avma, end_avma, 0);
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
        &mut self,
        timestamp_raw: u64,
        tid: u32,
        known_category: KnownCategory,
        name: &str,
        marker: impl ProfilerMarker,
    ) -> MarkerHandle {
        let category = self.categories.get(known_category, &mut self.profile);
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let timing = MarkerTiming::Instant(timestamp);
        let thread = self.threads.get_mut(&tid).unwrap();
        self.profile
            .add_marker(thread.handle, category, name, marker, timing)
    }

    pub fn add_thread_interval_marker(
        &mut self,
        start_timestamp_raw: u64,
        end_timestamp_raw: u64,
        tid: u32,
        known_category: KnownCategory,
        name: &str,
        marker: impl ProfilerMarker,
    ) -> MarkerHandle {
        let category = self.categories.get(known_category, &mut self.profile);
        let start_timestamp = self.timestamp_converter.convert_time(start_timestamp_raw);
        let end_timestamp = self.timestamp_converter.convert_time(end_timestamp_raw);
        let timing = MarkerTiming::Interval(start_timestamp, end_timestamp);
        let thread = self.threads.get(&tid).unwrap();
        self.profile
            .add_marker(thread.handle, category, name, marker, timing)
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
        self.profile.set_interval(interval);
        self.context_switch_handler = ContextSwitchHandler::new(interval_raw as u64);
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
        let name = self.map_device_path(&image_file_name);
        let process_handle = self.profile.add_process(&name, pid, timestamp);
        let main_thread_handle = self
            .profile
            .add_thread(process_handle, pid, timestamp, true);
        let main_thread_label_frame =
            make_thread_label_frame(&mut self.profile, Some(&name), pid, pid);
        let (thread_recycler, jit_function_recycler) = if self.process_recycler.is_some() {
            (
                Some(ThreadRecycler::new()),
                Some(JitFunctionRecycler::default()),
            )
        } else {
            (None, None)
        };
        let process = ProcessState {
            name,
            seen_main_thread_start: false,
            handle: process_handle,
            unresolved_samples: UnresolvedSamples::default(),
            regular_lib_mapping_ops: LibMappingOpQueue::default(),
            main_thread_handle,
            main_thread_label_frame,
            pending_libraries: HashMap::new(),
            memory_usage: None,
            process_id: pid,
            parent_id: parent_pid,
            jit_info: None,
            thread_recycler,
            jit_function_recycler,
        };
        self.processes.insert(pid, process);
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

        if let Some(dead_process_with_reused_pid) = self.processes.remove(&pid) {
            self.profile
                .set_process_end_time(dead_process_with_reused_pid.handle, timestamp);
            self.dead_processes_with_reused_pids
                .push(dead_process_with_reused_pid);
        }

        let name = self.map_device_path(&image_file_name);
        if let Some(process_recycler) = self.process_recycler.as_mut() {
            if let Some(ProcessRecyclingData {
                process_handle,
                main_thread_recycling_data,
                thread_recycler,
                jit_function_recycler,
            }) = process_recycler.recycle_by_name(&name)
            {
                log::info!("Found old process for pid {} and name {}", pid, name);
                let (main_thread_handle, main_thread_label_frame) = main_thread_recycling_data;
                let process = ProcessState {
                    name,
                    seen_main_thread_start: false,
                    handle: process_handle,
                    unresolved_samples: UnresolvedSamples::default(),
                    regular_lib_mapping_ops: LibMappingOpQueue::default(),
                    main_thread_handle,
                    main_thread_label_frame,
                    pending_libraries: HashMap::new(),
                    memory_usage: None,
                    process_id: pid,
                    parent_id: parent_pid,
                    jit_info: None,
                    thread_recycler: Some(thread_recycler),
                    jit_function_recycler: Some(jit_function_recycler),
                };
                self.processes.insert(pid, process);
                return;
            }
        }
        let process_handle = self.profile.add_process(&name, pid, timestamp);
        let main_thread_handle = self
            .profile
            .add_thread(process_handle, pid, timestamp, true);
        let main_thread_label_frame =
            make_thread_label_frame(&mut self.profile, Some(&name), pid, pid);
        let (thread_recycler, jit_function_recycler) = if self.process_recycler.is_some() {
            (
                Some(ThreadRecycler::new()),
                Some(JitFunctionRecycler::default()),
            )
        } else {
            (None, None)
        };
        let process = ProcessState {
            name,
            seen_main_thread_start: false,
            handle: process_handle,
            unresolved_samples: UnresolvedSamples::default(),
            regular_lib_mapping_ops: LibMappingOpQueue::default(),
            main_thread_handle,
            main_thread_label_frame,
            pending_libraries: HashMap::new(),
            memory_usage: None,
            process_id: pid,
            parent_id: parent_pid,
            jit_info: None,
            thread_recycler,
            jit_function_recycler,
        };
        self.processes.insert(pid, process);
    }

    pub fn handle_process_end(&mut self, timestamp_raw: u64, pid: u32) {
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.profile.set_process_end_time(process.handle, timestamp);

        if let Some(process_recycler) = self.process_recycler.as_mut() {
            if let Some(process_recycling_data) = process.take_recycling_data() {
                process_recycler.add_to_pool(&process.name, process_recycling_data);
                log::info!(
                    "Adding process with pid {} and name {} to pool",
                    process.process_id,
                    process.name
                );
            } else {
                log::info!("Could not get process recycling data");
            }
        }
    }

    pub fn handle_process_dcend(&mut self, _timestamp_raw: u64, _pid: u32) {
        // Nothing to do - the process is still alive at the end of profiling.
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
        if !self.processes.contains_key(&pid) {
            log::warn!("Adding thread {tid} for unknown pid {pid}");
            return;
        }

        let process = self.processes.get_mut(&pid).unwrap();
        if !process.seen_main_thread_start {
            process.seen_main_thread_start = true;
            let thread_handle = process.main_thread_handle;
            let thread_label_frame =
                make_thread_label_frame(&mut self.profile, name.as_deref(), pid, tid);
            process.main_thread_label_frame = thread_label_frame.clone();
            // self.profile.set_thread_tid(thread_handle, tid);
            let thread = ThreadState::new(name, true, thread_handle, thread_label_frame, pid, tid);
            self.threads.insert(tid, thread);
            return;
        }

        if self.main_thread_only {
            // Ignore this thread.
            return;
        }

        let thread_handle = self
            .profile
            .add_thread(process.handle, tid, timestamp, false);
        let thread_label_frame =
            make_thread_label_frame(&mut self.profile, name.as_deref(), pid, tid);
        if let Some(name) = name.as_deref() {
            if !name.is_empty() {
                self.profile.set_thread_name(thread_handle, name);
            }
        }

        let thread = ThreadState::new(name, false, thread_handle, thread_label_frame, pid, tid);
        self.threads.insert(tid, thread);
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

        if let Some(dead_thread_with_reused_tid) = self.threads.remove(&tid) {
            self.profile
                .set_thread_end_time(dead_thread_with_reused_tid.handle, timestamp);
            self.dead_threads_with_reused_tids
                .push(dead_thread_with_reused_tid);
        }

        if !self.processes.contains_key(&pid) {
            log::warn!("Adding thread {tid} for unknown pid {pid}");
            return;
        }

        let process = self.processes.get_mut(&pid).unwrap();
        if !process.seen_main_thread_start {
            process.seen_main_thread_start = true;
            let thread_handle = process.main_thread_handle;
            let thread_label_frame =
                make_thread_label_frame(&mut self.profile, name.as_deref(), pid, tid);
            process.main_thread_label_frame = thread_label_frame.clone();
            // self.profile.set_thread_tid(thread_handle, tid);
            let thread = ThreadState::new(name, true, thread_handle, thread_label_frame, pid, tid);
            self.threads.insert(tid, thread);
            return;
        }

        if self.main_thread_only {
            // Ignore this thread.
            return;
        }

        if let (Some(thread_name), Some(thread_recycler)) =
            (&name, process.thread_recycler.as_mut())
        {
            if let Some((thread_handle, thread_label_frame)) =
                thread_recycler.recycle_by_name(thread_name)
            {
                let thread =
                    ThreadState::new(name, false, thread_handle, thread_label_frame, pid, tid);
                self.threads.insert(tid, thread);
                return;
            }
        }

        let thread_handle = self
            .profile
            .add_thread(process.handle, tid, timestamp, false);
        let thread_label_frame =
            make_thread_label_frame(&mut self.profile, name.as_deref(), pid, tid);
        if let Some(name) = name.as_deref() {
            if !name.is_empty() {
                self.profile.set_thread_name(thread_handle, name);
            }
        }

        let thread = ThreadState::new(name, false, thread_handle, thread_label_frame, pid, tid);
        self.threads.insert(tid, thread);
    }

    pub fn handle_thread_set_name(
        &mut self,
        _timestamp_raw: u64,
        pid: u32,
        tid: u32,
        name: String,
    ) {
        if name.is_empty() {
            return;
        }
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };

        if let Some(thread_recycler) = process.thread_recycler.as_mut() {
            if let Some(old_name) = thread.name.as_deref() {
                let thread_recycling_data = (thread.handle, thread.label_frame.clone());
                thread_recycler.add_to_pool(old_name, thread_recycling_data);
            }
            if let Some((thread_handle, thread_label_frame)) =
                thread_recycler.recycle_by_name(&name)
            {
                thread.handle = thread_handle;
                thread.label_frame = thread_label_frame;
            }
        }
        self.profile.set_thread_name(thread.handle, &name);
        thread.name = Some(name);
    }

    pub fn handle_thread_end(&mut self, timestamp_raw: u64, pid: u32, tid: u32) {
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.profile.set_thread_end_time(thread.handle, timestamp);

        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        if let (Some(name), Some(thread_recycler)) =
            (thread.name.as_deref(), process.thread_recycler.as_mut())
        {
            let thread_recycling_data = (thread.handle, thread.label_frame.clone());
            thread_recycler.add_to_pool(name, thread_recycling_data);
        }
    }

    pub fn handle_thread_dcend(&mut self, _timestamp_raw: u64, _tid: u32) {
        // Nothing to do. The thread is still alive at the end of profiling.
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
        let Some(thread) = self.threads.get(&tid) else {
            return;
        };
        let stack: Vec<StackFrame> = to_stack_frames(stack_address_iter, self.address_classifier);

        let stack_index = self.unresolved_stacks.convert(stack.into_iter().rev());
        //eprintln!("event: StackWalk stack: {:?}", stack);

        // Note: we don't add these as actual samples, and instead just attach them to the marker.
        // If we added them as samples, it would throw off the profile counting, because they arrive
        // in between regular interval samples. In the future, maybe we can support fractional samples
        // somehow (fractional weight), but for now, we just attach them to the marker.

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.processes
            .get_mut(&pid)
            .unwrap()
            .unresolved_samples
            .attach_stack_to_marker(
                thread.handle,
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
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };

        // On ARM64, this seems to be simpler -- stacks come in with full kernel and user frames.
        // At least, I've never seen a kernel stack come in separately.
        // TODO -- is this because I can't use PROFILE events in the VM?

        let stack: Vec<StackFrame> = to_stack_frames(stack_address_iter, self.address_classifier);

        let cpu_delta_raw = self
            .context_switch_handler
            .consume_cpu_delta(&mut thread.context_switch_data);
        let cpu_delta =
            CpuDelta::from_nanos(cpu_delta_raw * self.timestamp_converter.raw_to_ns_factor);
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let stack_index = self.unresolved_stacks.convert(stack.into_iter().rev());
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        process.unresolved_samples.add_sample(
            thread.handle,
            timestamp,
            timestamp_raw,
            stack_index,
            cpu_delta,
            1,
            None,
        );
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
        let first_frame_stack_mode = self.address_classifier.get_stack_mode(first_frame_address);
        stack.push(StackFrame::InstructionPointer(
            first_frame_address,
            first_frame_stack_mode,
        ));
        stack.extend(address_iter.map(|addr| {
            let stack_mode = self.address_classifier.get_stack_mode(addr);
            StackFrame::ReturnAddress(addr, stack_mode)
        }));

        match first_frame_stack_mode {
            StackMode::Kernel => self.handle_kernel_stack(timestamp_raw, pid, tid, stack),
            StackMode::User => self.handle_user_stack(timestamp_raw, pid, tid, stack),
        }
    }

    fn handle_kernel_stack(
        &mut self,
        timestamp_raw: u64,
        _pid: u32,
        tid: u32,
        stack: Vec<StackFrame>,
    ) {
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };
        if let Some(pending_stack) = thread
            .pending_stacks
            .iter_mut()
            .rev()
            .find(|s| s.timestamp == timestamp_raw)
        {
            if let Some(kernel_stack) = pending_stack.kernel_stack.as_mut() {
                log::warn!("Multiple kernel stacks for timestamp {timestamp_raw} on thread {tid}");
                kernel_stack.extend(&stack);
            } else {
                pending_stack.kernel_stack = Some(stack);
            }
        }
    }

    fn handle_user_stack(
        &mut self,
        timestamp_raw: u64,
        pid: u32,
        tid: u32,
        stack: Vec<StackFrame>,
    ) {
        // We now know that we have a user stack. User stacks always come last. Consume
        // the pending stack with matching timestamp.

        let user_stack = stack;
        let user_stack_index = self
            .unresolved_stacks
            .convert(user_stack.iter().cloned().rev());

        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };
        // the number of pending stacks at or before our timestamp
        let num_pending_stacks = thread
            .pending_stacks
            .iter()
            .take_while(|s| s.timestamp <= timestamp_raw)
            .count();

        let pending_stacks: VecDeque<_> =
            thread.pending_stacks.drain(..num_pending_stacks).collect();

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
                    self.context_switch_handler
                        .consume_cpu_delta(&mut thread.context_switch_data)
                };
                let cpu_delta =
                    CpuDelta::from_nanos(cpu_delta_raw * self.timestamp_converter.raw_to_ns_factor);

                // Add a sample at the beginning of the paused range.
                // This "first sample" will carry any leftover accumulated running time ("cpu delta").
                let begin_timestamp = self.timestamp_converter.convert_time(begin_timestamp_raw);
                let Some(process) = self.processes.get_mut(&pid) else {
                    return;
                };
                process.unresolved_samples.add_sample(
                    thread.handle,
                    begin_timestamp,
                    begin_timestamp_raw,
                    user_stack_index,
                    cpu_delta,
                    1,
                    None,
                );

                if sample_count > 1 {
                    // Emit a "rest sample" with a CPU delta of zero covering the rest of the paused range.
                    let weight = i32::try_from(sample_count - 1).unwrap_or(0);
                    let end_timestamp = self.timestamp_converter.convert_time(end_timestamp_raw);
                    process.unresolved_samples.add_sample(
                        thread.handle,
                        end_timestamp,
                        end_timestamp_raw,
                        user_stack_index,
                        CpuDelta::ZERO,
                        weight,
                        None,
                    );
                }
            }

            if let Some(cpu_delta) = on_cpu_sample_cpu_delta {
                if let Some(mut combined_stack) = kernel_stack {
                    combined_stack.extend_from_slice(&user_stack[..]);
                    let combined_stack_index = self
                        .unresolved_stacks
                        .convert(combined_stack.into_iter().rev());
                    let Some(process) = self.processes.get_mut(&pid) else {
                        return;
                    };
                    process.unresolved_samples.add_sample(
                        thread.handle,
                        timestamp,
                        timestamp_raw,
                        combined_stack_index,
                        cpu_delta,
                        1,
                        None,
                    );
                } else {
                    let Some(process) = self.processes.get_mut(&pid) else {
                        return;
                    };
                    process.unresolved_samples.add_sample(
                        thread.handle,
                        timestamp,
                        timestamp_raw,
                        user_stack_index,
                        cpu_delta,
                        1,
                        None,
                    );
                }
                self.stack_sample_count += 1;
            }
        }
    }

    pub fn handle_sample(&mut self, timestamp_raw: u64, tid: u32) {
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };

        let off_cpu_sample_group = self
            .context_switch_handler
            .handle_on_cpu_sample(timestamp_raw, &mut thread.context_switch_data);
        let delta = self
            .context_switch_handler
            .consume_cpu_delta(&mut thread.context_switch_data);
        let cpu_delta = CpuDelta::from_nanos(delta * self.timestamp_converter.raw_to_ns_factor);
        thread.pending_stacks.push_back(PendingStack {
            timestamp: timestamp_raw,
            kernel_stack: None,
            off_cpu_sample_group,
            on_cpu_sample_cpu_delta: Some(cpu_delta),
        });

        self.sample_count += 1;
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

        let Some(memory_usage_counter) = self.get_or_create_memory_usage_counter(pid) else {
            return;
        };
        self.profile
            .add_counter_sample(memory_usage_counter, timestamp, delta_size, 1);
        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };
        self.profile.add_marker(
            thread.handle,
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
        } else if let Some(process) = self.processes.get_mut(&pid) {
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
        } else if let Some(process) = self.processes.get_mut(&pid) {
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

        let lib_handle = self.profile.add_lib(info);
        if pid == 0 || image_base >= self.kernel_min {
            self.profile
                .add_kernel_lib_mapping(lib_handle, image_base, image_base + image_size, 0);
            return;
        }

        let info = if known_category != KnownCategory::Unknown {
            let category = self.categories.get(known_category, &mut self.profile);
            LibMappingInfo::new_lib_with_category(lib_handle, category.into())
        } else {
            LibMappingInfo::new_lib(lib_handle)
        };

        self.processes
            .get_mut(&pid)
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
            let gpu = self.profile.add_process("GPU", 1, start_timestamp);
            self.profile.add_thread(gpu, 1, start_timestamp, false)
        });
        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        self.profile.add_marker(
            *gpu_thread,
            CategoryHandle::OTHER,
            "Vsync",
            VSyncMarker {},
            MarkerTiming::Instant(timestamp),
        );
    }

    pub fn handle_cswitch(&mut self, timestamp_raw: u64, old_tid: u32, new_tid: u32) {
        // println!("CSwitch {} -> {} @ {} on {}", old_tid, old_tid, e.EventHeader.TimeStamp, unsafe { e.BufferContext.Anonymous.ProcessorIndex });

        if let Some(old_thread) = self.threads.get_mut(&old_tid) {
            self.context_switch_handler
                .handle_switch_out(timestamp_raw, &mut old_thread.context_switch_data);
        }
        if let Some(new_thread) = self.threads.get_mut(&new_tid) {
            let off_cpu_sample_group = self
                .context_switch_handler
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

        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        let main_thread_handle = process.main_thread_handle;
        let process_jit_info = process.get_jit_info(&mut self.profile);

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let relative_address = process_jit_info.next_relative_address;
        process_jit_info.next_relative_address += method_size as u32;

        self.profile.add_marker(
            main_thread_handle,
            CategoryHandle::OTHER,
            "JitFunctionAdd",
            JitFunctionAddMarker(method_name.clone()),
            MarkerTiming::Instant(timestamp),
        );

        let (category, js_frame) = self
            .js_category_manager
            .classify_jit_symbol(&method_name, &mut self.profile);
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
        let Some(process) = self.processes.get_mut(&pid) else {
            return;
        };
        let process_jit_info = process.get_jit_info(&mut self.profile);
        let relative_address = process_jit_info.next_relative_address;
        process_jit_info.next_relative_address += method_size;

        // Not that useful for CoreCLR
        //let mh = context.add_thread_marker(s.thread_id(), timestamp, CategoryHandle::OTHER, "JitFunctionAdd", JitFunctionAddMarker(method_name.to_owned()));
        //core_clr_context.set_last_event_for_thread(thread_handle, mh);

        let category = self
            .categories
            .get(KnownCategory::CoreClrJit, &mut self.profile);
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
        let Some(thread) = self.threads.get_mut(&tid) else {
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
        let Some(thread) = self.threads.get_mut(&tid) else {
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

        let category = self.categories.get(known_category, &mut self.profile);
        self.profile.add_marker(
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
        let Some(thread) = self.threads.get_mut(&tid) else {
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
            self.profile.add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                "UserTiming",
                UserTimingMarker(name),
                timing,
            );
        } else if marker_name == "SimpleMarker" || marker_name == "Text" || marker_name == "tracing"
        {
            let marker_name = maybe_explicit_marker_name.unwrap();
            self.profile.add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                &marker_name,
                SimpleMarker(text.clone()),
                timing,
            );
        } else {
            self.profile.add_marker(
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
        let Some(thread) = self.threads.get_mut(&tid) else {
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
            self.profile.add_marker(
                thread.handle,
                CategoryHandle::OTHER,
                "UserTiming",
                UserTimingMarker(marker_name.to_owned()),
                timing,
            );
        } else {
            self.profile.add_marker(
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
        if !self.profile_creation_props.unknown_event_markers {
            return;
        }

        let Some(thread) = self.threads.get_mut(&tid) else {
            return;
        };

        let timestamp = self.timestamp_converter.convert_time(timestamp_raw);
        let timing = MarkerTiming::Instant(timestamp);
        // this used to create a new category based on provider_name, just lump them together for now
        let category = self
            .categories
            .get(KnownCategory::Unknown, &mut self.profile);
        self.profile.add_marker(
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
        for mut process in self
            .dead_processes_with_reused_pids
            .into_iter()
            .chain(self.processes.into_values())
        {
            let jitdump_lib_mapping_op_queues = match process.jit_info.take() {
                Some(jit_info) => {
                    self.profile.set_lib_symbol_table(
                        jit_info.lib_handle,
                        Arc::new(SymbolTable::new(jit_info.symbols)),
                    );
                    vec![jit_info.jit_mapping_ops]
                }
                None => Vec::new(),
            };

            let process_sample_data = ProcessSampleData::new(
                process.unresolved_samples,
                process.regular_lib_mapping_ops,
                jitdump_lib_mapping_op_queues,
                None,
                Vec::new(),
            );
            //main_thread_handle.unwrap_or_else(|| panic!("process no main thread {:?}", process_id)));
            let user_category = self
                .categories
                .get(KnownCategory::User, &mut self.profile)
                .into();
            let kernel_category = self
                .categories
                .get(KnownCategory::Kernel, &mut self.profile)
                .into();
            process_sample_data.flush_samples_to_profile(
                &mut self.profile,
                user_category,
                kernel_category,
                &mut stack_frame_scratch_buf,
                &self.unresolved_stacks,
            )
        }

        log::info!(
            "{} events, {} samples, {} stack-samples",
            self.event_count,
            self.sample_count,
            self.stack_sample_count
        );

        self.profile
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

fn to_stack_frames(
    mut address_iter: impl Iterator<Item = u64>,
    address_classifier: AddressClassifier,
) -> Vec<StackFrame> {
    let Some(first_addr) = address_iter.next() else {
        return Vec::new();
    };
    let first_stack_mode = address_classifier.get_stack_mode(first_addr);
    let mut frames = vec![StackFrame::InstructionPointer(first_addr, first_stack_mode)];

    frames.extend(address_iter.map(|addr| {
        let stack_mode = address_classifier.get_stack_mode(addr);
        StackFrame::ReturnAddress(addr, stack_mode)
    }));
    frames
}

pub fn make_thread_label_frame(
    profile: &mut Profile,
    name: Option<&str>,
    pid: u32,
    tid: u32,
) -> FrameInfo {
    let s = match name {
        Some(name) => format!("{name} (pid: {pid}, tid: {tid})"),
        None => format!("Thread {tid} (pid: {pid}, tid: {tid})"),
    };
    let thread_label = profile.intern_string(&s);
    FrameInfo {
        frame: Frame::Label(thread_label),
        category_pair: CategoryHandle::OTHER.into(),
        flags: FrameFlags::empty(),
    }
}
