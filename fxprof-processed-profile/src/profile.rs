use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use indexmap::set::MutableValues;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::json;

use crate::category::{
    Category, CategoryHandle, InternalCategory, IntoSubcategoryHandle, SubcategoryHandle,
};
use crate::category_color::CategoryColor;
use crate::counters::{Counter, CounterHandle};
use crate::cpu_delta::CpuDelta;
use crate::fast_hash_map::{FastHashMap, FastHashSet, FastIndexSet};
use crate::frame::FrameAddress;
use crate::frame_table::{
    InternalFrame, InternalFrameAddress, InternalFrameVariant, NativeFrameData,
};
use crate::global_lib_table::{GlobalLibIndex, GlobalLibTable, LibraryHandle};
use crate::lib_mappings::LibMappings;
use crate::library_info::{LibraryInfo, SymbolTable};
use crate::markers::{
    GraphColor, InternalMarkerSchema, Marker, MarkerHandle, MarkerTiming, MarkerTypeHandle,
    RuntimeSchemaMarkerSchema, StaticSchemaMarker,
};
use crate::native_symbols::NativeSymbolHandle;
use crate::process::{Process, ThreadHandle};
use crate::profile_symbol_info::{LibSymbolInfo, ProfileSymbolInfo};
use crate::reference_timestamp::ReferenceTimestamp;
use crate::sample_table::WeightType;
use crate::string_table::{ProfileStringTable, StringHandle};
use crate::symbolication::StringTableAdapter;
use crate::thread::{ProcessHandle, Thread};
use crate::timestamp::Timestamp;
use crate::{FrameFlags, PlatformSpecificReferenceTimestamp, Symbol};

/// The sampling interval used during profile recording.
///
/// This doesn't have to match the actual delta between sample timestamps.
/// It just describes the intended interval.
///
/// For profiles without sampling data, this can be set to a meaningless
/// dummy value.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SamplingInterval {
    nanos: u64,
}

impl SamplingInterval {
    /// Create a sampling interval from a sampling frequency in Hz.
    ///
    /// Panics on zero or negative values.
    pub fn from_hz(samples_per_second: f32) -> Self {
        assert!(samples_per_second > 0.0);
        let nanos = (1_000_000_000.0 / samples_per_second) as u64;
        Self::from_nanos(nanos)
    }

    /// Create a sampling interval from a value in milliseconds.
    pub fn from_millis(millis: u64) -> Self {
        Self::from_nanos(millis * 1_000_000)
    }

    /// Create a sampling interval from a value in nanoseconds
    pub fn from_nanos(nanos: u64) -> Self {
        Self { nanos }
    }

    /// Convert the interval to nanoseconds.
    pub fn nanos(&self) -> u64 {
        self.nanos
    }

    /// Convert the interval to float seconds.
    pub fn as_secs_f64(&self) -> f64 {
        self.nanos as f64 / 1_000_000_000.0
    }
}

impl From<Duration> for SamplingInterval {
    fn from(duration: Duration) -> Self {
        Self::from_nanos(duration.as_nanos() as u64)
    }
}

/// A handle to a frame, specific to a thread. Can be created with
/// [`Profile::handle_for_frame_with_label`], [`Profile::handle_for_frame_with_address`],
/// and so on.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FrameHandle(ThreadHandle, usize);

/// A handle to a stack, specific to a thread. Can be created with [`Profile::handle_for_stack`](crate::Profile::handle_for_stack).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StackHandle(ThreadHandle, usize);

/// Symbol information about a frame address.
///
/// At a minimum, this contains the [`NativeSymbolHandle`] for the function
/// whose native code contains the frame address. The rest of the information
/// is optional.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct FrameSymbolInfo {
    /// The function name. If set to `None`, the name of the native symbol will be used.
    pub name: Option<StringHandle>,
    /// The native symbol for the function whose native code contains the frame address.
    pub native_symbol: NativeSymbolHandle,
    /// The source location. All fields can be set to `None` if unknown.
    pub source_location: SourceLocation,
}

/// Source code information (file path + line number + column number) for a frame.
#[derive(Debug, Default, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    /// The [`StringHandle`] for the file path of the source file. Optional.
    pub file_path: Option<StringHandle>,
    /// The line number in the source file, optional. The first line has number 1.
    ///
    /// This should be the line number of the line that's "being executed" for
    /// this frame, i.e. a line within a function.
    pub line: Option<u32>,
    /// The column number in the source file, measured in bytes from line start. Optional.
    ///
    /// The first byte in each line has column number zero.
    pub col: Option<u32>,
}

/// The unit that should be used for the timeline at the top of the profiler UI.
///
/// Used in [`Profile::set_timeline_unit`].
#[derive(Debug, Default, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum TimelineUnit {
    #[default]
    Milliseconds,
    Bytes,
}

/// Stores the profile data and can be serialized as JSON, via [`serde::Serialize`].
///
/// The profile data is organized into a list of processes with threads.
/// Each thread has its own samples and markers.
///
/// ```
/// use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, FrameAddress, FrameFlags, SamplingInterval, Timestamp};
/// use std::time::SystemTime;
///
/// # fn write_profile(output_file: std::fs::File) -> Result<(), Box<dyn std::error::Error>> {
/// let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
/// let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
/// let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
/// profile.set_thread_name(thread, "Main thread");
///
/// let root_node_string = profile.handle_for_string("Root node");
/// let root_frame = profile.handle_for_frame_with_label(thread, root_node_string, CategoryHandle::OTHER, FrameFlags::empty());
/// let first_callee_string = profile.handle_for_string("First callee");
/// let first_callee_frame = profile.handle_for_frame_with_label(thread, first_callee_string, CategoryHandle::OTHER, FrameFlags::empty());
///
/// let root_stack_node = profile.handle_for_stack(thread, root_frame, None);
/// let first_callee_node = profile.handle_for_stack(thread, first_callee_frame, Some(root_stack_node));
/// profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), Some(first_callee_node), CpuDelta::ZERO, 1);
///
/// let writer = std::io::BufWriter::new(output_file);
/// serde_json::to_writer(writer, &profile)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Profile {
    pub(crate) product: String,
    pub(crate) os_name: Option<String>,
    pub(crate) interval: SamplingInterval,
    pub(crate) timeline_unit: TimelineUnit,
    pub(crate) global_libs: GlobalLibTable,
    pub(crate) kernel_libs: LibMappings<LibraryHandle>,
    pub(crate) categories: FastIndexSet<InternalCategory>, // append-only for stable CategoryHandles
    pub(crate) processes: Vec<Process>,                    // append-only for stable ProcessHandles
    pub(crate) counters: Vec<Counter>,
    pub(crate) threads: Vec<Thread>, // append-only for stable ThreadHandles
    pub(crate) initial_visible_threads: Vec<ThreadHandle>,
    pub(crate) initial_selected_threads: Vec<ThreadHandle>,
    pub(crate) reference_timestamp: ReferenceTimestamp,
    pub(crate) platform_specific_reference_timestamp: Option<PlatformSpecificReferenceTimestamp>,
    pub(crate) string_table: ProfileStringTable,
    pub(crate) marker_schemas: Vec<InternalMarkerSchema>,
    static_schema_marker_types: FastHashMap<&'static str, MarkerTypeHandle>,
    pub(crate) symbolicated: bool,
    used_pids: FastHashMap<u32, u32>,
    used_tids: FastHashMap<u32, u32>,
}

impl Profile {
    /// Create a new profile.
    ///
    /// The `product` is the name of the main application which was profiled.
    /// The `reference_timestamp` is some arbitrary absolute timestamp which all
    /// other timestamps in the profile data are relative to. The `interval` is the intended
    /// time delta between samples.
    pub fn new(
        product: &str,
        reference_timestamp: ReferenceTimestamp,
        interval: SamplingInterval,
    ) -> Self {
        let mut categories = FastIndexSet::default();
        categories.insert(InternalCategory::new("Other", CategoryColor::Gray));
        Profile {
            interval,
            product: product.to_string(),
            os_name: None,
            timeline_unit: TimelineUnit::Milliseconds,
            threads: Vec::new(),
            initial_visible_threads: Vec::new(),
            initial_selected_threads: Vec::new(),
            global_libs: GlobalLibTable::new(),
            kernel_libs: LibMappings::new(),
            reference_timestamp,
            platform_specific_reference_timestamp: None,
            processes: Vec::new(),
            string_table: ProfileStringTable::new(),
            marker_schemas: Vec::new(),
            categories,
            static_schema_marker_types: FastHashMap::default(),
            symbolicated: false,
            used_pids: FastHashMap::default(),
            used_tids: FastHashMap::default(),
            counters: Vec::new(),
        }
    }

    /// Change the declared sampling interval.
    pub fn set_interval(&mut self, interval: SamplingInterval) {
        self.interval = interval;
    }

    /// Change the reference timestamp.
    pub fn set_reference_timestamp(&mut self, reference_timestamp: ReferenceTimestamp) {
        self.reference_timestamp = reference_timestamp;
    }

    /// Set an additional reference timestamp with a platform-specific unit.
    pub fn set_platform_specific_reference_timestamp(
        &mut self,
        platform_specific_reference_timestamp: PlatformSpecificReferenceTimestamp,
    ) {
        self.platform_specific_reference_timestamp = Some(platform_specific_reference_timestamp);
    }

    /// Change the product name.
    pub fn set_product(&mut self, product: &str) {
        self.product = product.to_string();
    }

    /// Set the name of the operating system.
    pub fn set_os_name(&mut self, os_name: &str) {
        self.os_name = Some(os_name.to_string());
    }

    /// Set the unit that the timeline should display. Default is [`TimelineUnit::Milliseconds`].
    ///
    /// If this is set to [`TimelineUnit::Bytes`], then the sample [`Timestamp`]s are interpreted
    /// as byte offsets, with one milliseconds equaling one byte.
    /// This can be used for file size profiles, where each sample describes
    /// a certain byte within the file at an offset.
    pub fn set_timeline_unit(&mut self, timeline_unit: TimelineUnit) {
        self.timeline_unit = timeline_unit;
    }

    /// Get or create a handle for a category.
    ///
    /// Categories are used for stack frames and markers.
    pub fn handle_for_category(&mut self, category: Category) -> CategoryHandle {
        let index = self.categories.get_index_of(&category).unwrap_or_else(|| {
            let Category(name, color) = category;
            self.categories
                .insert_full(InternalCategory::new(name, color))
                .0
        });
        CategoryHandle(index as u16)
    }

    /// Get or create a handle for a subcategory.
    ///
    /// Every category has a default subcategory; you can convert a `CategoryHandle` into
    /// its corresponding `SubcategoryHandle` for the default category using `category.into()`.
    pub fn handle_for_subcategory(
        &mut self,
        category_handle: CategoryHandle,
        subcategory_name: &str,
    ) -> SubcategoryHandle {
        let category_index = category_handle.0 as usize;
        let category = self.categories.get_index_mut2(category_index).unwrap();
        let subcategory_index = category.index_for_subcategory(subcategory_name);
        SubcategoryHandle(category_handle, subcategory_index)
    }

    /// Add an empty process. The name, pid and start time can be changed afterwards,
    /// but they are required here because they have to be present in the profile JSON.
    pub fn add_process(&mut self, name: &str, pid: u32, start_time: Timestamp) -> ProcessHandle {
        let pid = self.make_unique_pid(pid);
        let handle = ProcessHandle(self.processes.len());
        self.processes.push(Process::new(name, pid, start_time));
        handle
    }

    fn make_unique_pid(&mut self, pid: u32) -> String {
        Self::make_unique_pid_or_tid(&mut self.used_pids, pid)
    }

    fn make_unique_tid(&mut self, tid: u32) -> String {
        Self::make_unique_pid_or_tid(&mut self.used_tids, tid)
    }

    /// Appends ".1" / ".2" etc. to the pid or tid if needed.
    ///
    /// The map contains the next suffix for each pid/tid, or no entry if the pid/tid
    /// hasn't been used before and needs no suffix.
    fn make_unique_pid_or_tid(map: &mut FastHashMap<u32, u32>, id: u32) -> String {
        match map.entry(id) {
            Entry::Occupied(mut entry) => {
                let suffix = *entry.get();
                *entry.get_mut() += 1;
                format!("{id}.{suffix}")
            }
            Entry::Vacant(entry) => {
                entry.insert(1);
                format!("{id}")
            }
        }
    }

    /// Create a counter. Counters let you make graphs with a time axis and a Y axis. One example of a
    /// counter is memory usage.
    ///
    /// # Example
    ///
    /// ```
    /// use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, SamplingInterval, Timestamp};
    /// use std::time::SystemTime;
    ///
    /// let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
    /// let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
    /// let memory_counter = profile.add_counter(process, "malloc", "Memory", "Amount of allocated memory");
    /// profile.add_counter_sample(memory_counter, Timestamp::from_millis_since_reference(0.0), 0.0, 0);
    /// profile.add_counter_sample(memory_counter, Timestamp::from_millis_since_reference(1.0), 1000.0, 2);
    /// profile.add_counter_sample(memory_counter, Timestamp::from_millis_since_reference(2.0), 800.0, 1);
    /// ```
    pub fn add_counter(
        &mut self,
        process: ProcessHandle,
        name: &str,
        category: &str,
        description: &str,
    ) -> CounterHandle {
        let handle = CounterHandle(self.counters.len());
        self.counters.push(Counter::new(
            name,
            category,
            description,
            process,
            self.processes[process.0].pid(),
        ));
        handle
    }

    /// Set the color to use when rendering the counter.
    pub fn set_counter_color(&mut self, counter: CounterHandle, color: GraphColor) {
        self.counters[counter.0].set_color(color);
    }

    /// Change the start time of a process.
    pub fn set_process_start_time(&mut self, process: ProcessHandle, start_time: Timestamp) {
        self.processes[process.0].set_start_time(start_time);
    }

    /// Set the end time of a process.
    pub fn set_process_end_time(&mut self, process: ProcessHandle, end_time: Timestamp) {
        self.processes[process.0].set_end_time(end_time);
    }

    /// Change the name of a process.
    pub fn set_process_name(&mut self, process: ProcessHandle, name: &str) {
        self.processes[process.0].set_name(name);
    }

    /// Get the [`LibraryHandle`] for a library. This handle is used in [`FrameAddress`]
    /// and in [`Profile::add_lib_mapping`].
    ///
    /// Knowing the library information allows symbolication of native stacks once the
    /// profile is opened in the Firefox Profiler.
    pub fn add_lib(&mut self, library: LibraryInfo) -> LibraryHandle {
        self.global_libs.handle_for_lib(library)
    }

    /// Look up the [`LibraryInfo`] for a [`LibraryHandle`].
    pub fn get_library_info(&self, handle: LibraryHandle) -> &LibraryInfo {
        self.global_libs.get_lib(handle)
    }

    /// Set an optional symbol table for a library, for "pre-symbolicating" stack frames.
    ///
    /// Usually, symbolication is something that should happen asynchronously,
    /// because it can be very slow, so the regular way to use the profiler is to
    /// store only frame addresses and no symbols in the profile JSON, and perform
    /// symbolication only once the profile is loaded in the Firefox Profiler UI.
    ///
    /// However, sometimes symbols are only available during recording and are not
    /// easily accessible afterwards. One such example the symbol table of the
    /// Linux kernel: Users with root privileges can access the symbol table of the
    /// currently-running kernel via `/proc/kallsyms`, but we don't want to have
    /// to run the local symbol server with root privileges. So it's easier to
    /// resolve kernel symbols when generating the profile JSON.
    ///
    /// This form of symbolicating does not support file names, line numbers, or
    /// inline frames. It is intended for relatively "small" symbol tables for which
    /// an address lookup is fast.
    pub fn set_lib_symbol_table(&mut self, library: LibraryHandle, symbol_table: Arc<SymbolTable>) {
        self.global_libs.set_lib_symbol_table(library, symbol_table);
    }

    /// For a given process, define where in the virtual memory of this process the given library
    /// is mapped.
    ///
    /// Existing mappings which overlap with the range `start_avma..end_avma` will be removed.
    ///
    /// A single library can have multiple mappings in the same process.
    ///
    /// The new mapping will be respected by future [`Profile::handle_for_frame_with_address`]
    /// calls, when resolving absolute frame addresses to library-relative addresses.
    pub fn add_lib_mapping(
        &mut self,
        process: ProcessHandle,
        lib: LibraryHandle,
        start_avma: u64,
        end_avma: u64,
        relative_address_at_start: u32,
    ) {
        self.processes[process.0].add_lib_mapping(
            lib,
            start_avma,
            end_avma,
            relative_address_at_start,
        );
    }

    /// Mark the library mapping at the specified start address in the specified process as
    /// unloaded, so that future calls to [`Profile::handle_for_frame_with_address`] know about the removal.
    pub fn remove_lib_mapping(&mut self, process: ProcessHandle, start_avma: u64) {
        self.processes[process.0].remove_lib_mapping(start_avma);
    }

    /// Clear all library mappings in the specified process.
    pub fn clear_process_lib_mappings(&mut self, process: ProcessHandle) {
        self.processes[process.0].remove_all_lib_mappings();
    }

    /// Add a kernel library mapping. This allows symbolication of kernel stacks once the profile is
    /// opened in the Firefox Profiler. Kernel libraries are global and not tied to a process.
    ///
    /// Each kernel library covers an address range in the kernel address space, which is
    /// global across all processes. Future calls to [`Profile::handle_for_frame_with_address`]
    /// with native frames resolve the frame's code address with respect to the currently loaded kernel
    /// and process libraries.
    pub fn add_kernel_lib_mapping(
        &mut self,
        lib: LibraryHandle,
        start_avma: u64,
        end_avma: u64,
        relative_address_at_start: u32,
    ) {
        self.kernel_libs
            .add_mapping(start_avma, end_avma, relative_address_at_start, lib);
    }

    /// Mark the kernel library at the specified start address as
    /// unloaded, so that future calls to [`Profile::handle_for_frame_with_address`] know about the unloading.
    pub fn remove_kernel_lib_mapping(&mut self, start_avma: u64) {
        self.kernel_libs.remove_mapping(start_avma);
    }

    /// Add an empty thread to the specified process.
    pub fn add_thread(
        &mut self,
        process: ProcessHandle,
        tid: u32,
        start_time: Timestamp,
        is_main: bool,
    ) -> ThreadHandle {
        let tid = self.make_unique_tid(tid);
        let handle = ThreadHandle(self.threads.len());
        self.threads
            .push(Thread::new(process, tid, start_time, is_main));
        self.processes[process.0].add_thread(handle, is_main);
        handle
    }

    /// Change the name of a thread.
    pub fn set_thread_name(&mut self, thread: ThreadHandle, name: &str) {
        self.threads[thread.0].set_name(name);
    }

    /// Change the start time of a thread.
    pub fn set_thread_start_time(&mut self, thread: ThreadHandle, start_time: Timestamp) {
        self.threads[thread.0].set_start_time(start_time);
    }

    /// Set the end time of a thread.
    pub fn set_thread_end_time(&mut self, thread: ThreadHandle, end_time: Timestamp) {
        self.threads[thread.0].set_end_time(end_time);
    }

    /// Set the tid (thread ID) of a thread.
    pub fn set_thread_tid(&mut self, thread: ThreadHandle, tid: u32) {
        let tid = self.make_unique_tid(tid);
        self.threads[thread.0].set_tid(tid);
    }

    /// Set whether to show a timeline which displays [`MarkerLocations::TIMELINE_OVERVIEW`](crate::MarkerLocations::TIMELINE_OVERVIEW)
    /// markers for this thread.
    ///
    /// Main threads always have such a timeline view and always display such markers,
    /// but non-main threads only do so when specified using this method.
    pub fn set_thread_show_markers_in_timeline(&mut self, thread: ThreadHandle, v: bool) {
        self.threads[thread.0].set_show_markers_in_timeline(v);
    }

    /// Set the weighting type of samples of a thread.
    ///
    /// Default is [`WeightType::Samples`].
    pub fn set_thread_samples_weight_type(&mut self, thread: ThreadHandle, t: WeightType) {
        self.threads[thread.0].set_samples_weight_type(t);
    }

    /// Add a thread as initially visible in the UI.
    ///
    /// If not called, the UI uses its own ranking heuristic to choose which
    /// threads are visible.
    pub fn add_initial_visible_thread(&mut self, thread: ThreadHandle) {
        self.initial_visible_threads.push(thread);
    }

    /// Clear the list of threads marked as initially visible in the UI.
    pub fn clear_initial_visible_threads(&mut self) {
        self.initial_visible_threads.clear();
    }

    /// Add a thread as initially selected in the UI.
    ///
    /// If not called, the UI uses its own heuristic to choose which threads
    /// are initially selected.
    pub fn add_initial_selected_thread(&mut self, thread: ThreadHandle) {
        self.initial_selected_threads.push(thread);
    }

    /// Clear the list of threads marked as initially selected in the UI.
    pub fn clear_initial_selected_threads(&mut self) {
        self.initial_selected_threads.clear();
    }

    /// Get or create the [`StringHandle`] for a string.
    pub fn handle_for_string(&mut self, s: &str) -> StringHandle {
        self.string_table.index_for_string(s)
    }

    /// Look up the string for a string handle. This is sometimes useful when writing tests.
    ///
    /// Panics if the handle wasn't found, which can happen if you pass a handle
    /// from a different Profile instance.
    pub fn get_string(&self, handle: StringHandle) -> &str {
        self.string_table.get_string(handle)
    }

    /// Get the [`FrameHandle`] for a label frame.
    ///
    /// The returned handle can only be used with this thread.
    pub fn handle_for_frame_with_label<SC: IntoSubcategoryHandle>(
        &mut self,
        thread: ThreadHandle,
        label: StringHandle,
        subcategory: SC,
        flags: FrameFlags,
    ) -> FrameHandle {
        let subcategory = subcategory.into_subcategory_handle(self);
        self.handle_for_frame_with_label_internal(thread, label, None, subcategory, flags)
    }

    /// Get the [`FrameHandle`] for an address-based stack frame.
    ///
    /// The returned handle can only be used with this thread.
    pub fn handle_for_frame_with_address<SC: IntoSubcategoryHandle>(
        &mut self,
        thread: ThreadHandle,
        frame_address: FrameAddress,
        subcategory: SC,
        flags: FrameFlags,
    ) -> FrameHandle {
        let subcategory = subcategory.into_subcategory_handle(self);
        self.handle_for_frame_with_address_internal(thread, frame_address, subcategory, flags)
    }

    fn handle_for_frame_with_address_internal(
        &mut self,
        thread: ThreadHandle,
        frame_address: FrameAddress,
        subcategory: SubcategoryHandle,
        flags: FrameFlags,
    ) -> FrameHandle {
        let thread_handle = thread;
        let thread = &mut self.threads[thread_handle.0];
        let process = &mut self.processes[thread.process().0];
        let address = Self::resolve_frame_address(
            process,
            frame_address,
            &mut self.global_libs,
            &mut self.kernel_libs,
            &mut self.string_table,
        );
        let (variant, name) = match address {
            InternalFrameAddress::Unknown(address) => {
                let name = self.string_table.index_for_hex_address_string(address);
                (InternalFrameVariant::Label, name)
            }
            InternalFrameAddress::InLib(address, lib_index) => {
                let lib_symbol_table = self.global_libs.get_lib_symbol_table(lib_index);
                let symbol = lib_symbol_table.and_then(|symbol_table| symbol_table.lookup(address));
                let (native_symbol, name) = match symbol {
                    Some(symbol) => {
                        let (native_symbol, name) = thread
                            .native_symbol_index_and_string_index_for_symbol(
                                lib_index,
                                symbol,
                                &mut self.string_table,
                            );
                        (Some(native_symbol), name)
                    }
                    None => {
                        let name = self
                            .string_table
                            .index_for_hex_address_string(address.into());
                        (None, name)
                    }
                };
                let variant = InternalFrameVariant::Native(NativeFrameData {
                    lib: lib_index,
                    relative_address: address,
                    inline_depth: 0,
                    native_symbol,
                });
                (variant, name)
            }
        };
        let internal_frame = InternalFrame {
            variant,
            subcategory,
            flags,
            name,
            source_location: Default::default(),
        };
        let frame_index = thread.frame_index_for_frame(internal_frame);
        FrameHandle(thread_handle, frame_index)
    }

    /// Get the [`FrameHandle`] for a label frame with source location information.
    ///
    /// The returned handle can only be used with this thread.
    pub fn handle_for_frame_with_label_and_source_location<SC: IntoSubcategoryHandle>(
        &mut self,
        thread: ThreadHandle,
        label: StringHandle,
        source_location: SourceLocation,
        subcategory: SC,
        flags: FrameFlags,
    ) -> FrameHandle {
        let subcategory = subcategory.into_subcategory_handle(self);
        self.handle_for_frame_with_label_internal(
            thread,
            label,
            Some(source_location),
            subcategory,
            flags,
        )
    }

    fn handle_for_frame_with_label_internal(
        &mut self,
        thread: ThreadHandle,
        label: StringHandle,
        source_location: Option<SourceLocation>,
        subcategory: SubcategoryHandle,
        flags: FrameFlags,
    ) -> FrameHandle {
        let thread_handle = thread;
        let thread = &mut self.threads[thread_handle.0];
        let name = label;
        let source_location = source_location.unwrap_or_default();
        let internal_frame = InternalFrame {
            name,
            variant: InternalFrameVariant::Label,
            subcategory,
            source_location,
            flags,
        };
        let frame_index = thread.frame_index_for_frame(internal_frame);
        FrameHandle(thread_handle, frame_index)
    }

    /// Get the [`FrameHandle`] for an address-based stack frame with symbol information.
    ///
    /// The returned handle can only be used with this thread.
    pub fn handle_for_frame_with_address_and_symbol<SC: IntoSubcategoryHandle>(
        &mut self,
        thread: ThreadHandle,
        frame_address: FrameAddress,
        frame_symbol_info: FrameSymbolInfo,
        inline_depth: u16,
        subcategory: SC,
        flags: FrameFlags,
    ) -> FrameHandle {
        let subcategory = subcategory.into_subcategory_handle(self);
        self.handle_for_frame_with_address_and_symbol_internal(
            thread,
            frame_address,
            frame_symbol_info,
            inline_depth,
            subcategory,
            flags,
        )
    }

    fn handle_for_frame_with_address_and_symbol_internal(
        &mut self,
        thread: ThreadHandle,
        frame_address: FrameAddress,
        frame_symbol_info: FrameSymbolInfo,
        inline_depth: u16,
        subcategory: SubcategoryHandle,
        flags: FrameFlags,
    ) -> FrameHandle {
        let thread_handle = thread;
        let FrameSymbolInfo {
            name,
            source_location,
            native_symbol,
        } = frame_symbol_info;
        assert_eq!(native_symbol.0, thread_handle, "NativeSymbolHandle from wrong thread passed to Profile::handle_for_frame_with_address_and_symbol");
        let native_symbol_index = native_symbol.1;
        let thread = &mut self.threads[thread_handle.0];
        let process = &mut self.processes[thread.process().0];
        let address = Self::resolve_frame_address(
            process,
            frame_address,
            &mut self.global_libs,
            &mut self.kernel_libs,
            &mut self.string_table,
        );
        let (variant, name) = match address {
            InternalFrameAddress::Unknown(addr) => {
                let name =
                    name.unwrap_or_else(|| self.string_table.index_for_hex_address_string(addr));
                (InternalFrameVariant::Label, name)
            }
            InternalFrameAddress::InLib(relative_address, lib) => {
                let name =
                    name.unwrap_or_else(|| thread.get_native_symbol_name(native_symbol_index));
                (
                    InternalFrameVariant::Native(NativeFrameData {
                        lib,
                        native_symbol: Some(native_symbol.1),
                        relative_address,
                        inline_depth,
                    }),
                    name,
                )
            }
        };
        let internal_frame = InternalFrame {
            name,
            subcategory,
            variant,
            source_location,
            flags,
        };
        let frame_index = thread.frame_index_for_frame(internal_frame);
        FrameHandle(thread_handle, frame_index)
    }

    /// Get the [`NativeSymbolHandle`] for a native symbol, for use in [`FrameSymbolInfo`].
    ///
    /// The returned handle can only be used with this thread.
    pub fn handle_for_native_symbol(
        &mut self,
        thread: ThreadHandle,
        lib: LibraryHandle,
        symbol: &Symbol,
    ) -> NativeSymbolHandle {
        let thread_handle = thread;
        let thread = &mut self.threads[thread_handle.0];
        let global_lib_index = self
            .global_libs
            .index_for_used_lib(lib, &mut self.string_table);
        let native_symbol_index = thread.native_symbol_index_for_native_symbol(
            global_lib_index,
            symbol,
            &mut self.string_table,
        );
        NativeSymbolHandle(thread_handle, native_symbol_index)
    }

    /// Get the [`StackHandle`] for a stack with the given `frame` and `parent`,
    /// for the given thread.
    ///
    /// The returned stack handle can be used with [`Profile::add_sample`] and
    /// [`Profile::set_marker_stack`], but only for samples / markers of the same
    /// thread.
    ///
    /// If `parent` is `None`, this creates a root stack node. Otherwise, `parent`
    /// is the caller of the returned stack node.
    pub fn handle_for_stack(
        &mut self,
        thread: ThreadHandle,
        frame: FrameHandle,
        parent: Option<StackHandle>,
    ) -> StackHandle {
        let thread_handle = thread;
        let prefix = match parent {
            Some(StackHandle(parent_thread_handle, prefix_stack_index)) => {
                assert_eq!(
                    parent_thread_handle, thread_handle,
                    "StackHandle from different thread passed to Profile::handle_for_stack"
                );
                Some(prefix_stack_index)
            }
            None => None,
        };
        let FrameHandle(frame_thread_handle, frame_index) = frame;
        assert_eq!(
            frame_thread_handle, thread_handle,
            "FrameHandle from different thread passed to Profile::handle_for_stack"
        );
        let thread = &mut self.threads[thread.0];
        let stack_index = thread.stack_index_for_stack(prefix, frame_index);
        StackHandle(thread_handle, stack_index)
    }

    /// Get the [`StackHandle`] for a stack whose frames are given by an iterator function.
    ///
    /// The stack frames yielded by the function need to be ordered from caller-most
    /// to callee-most. The function will be called until it returns `None`. We pass a `&mut`
    /// reference to this profile object so that the callback can create new frames.
    ///
    /// Returns `None` if the stack has zero frames.
    pub fn handle_for_stack_frames<F>(
        &mut self,
        thread: ThreadHandle,
        mut frames_iter: F,
    ) -> Option<StackHandle>
    where
        F: FnMut(&mut Profile) -> Option<FrameHandle>,
    {
        let thread_handle = thread;
        let mut prefix = None;
        while let Some(frame_handle) = frames_iter(self) {
            assert_eq!(
                frame_handle.0, thread_handle,
                "FrameHandle from different thread passed to Profile::handle_for_stack_frames"
            );
            let thread = &mut self.threads[thread_handle.0];
            prefix = Some(thread.stack_index_for_stack(prefix, frame_handle.1));
        }
        let stack_index = prefix?;
        Some(StackHandle(thread_handle, stack_index))
    }

    /// Add a sample to the given thread.
    ///
    /// The sample has a timestamp, a stack, a CPU delta, and a weight.
    ///
    /// To get the stack handle, you can use [`Profile::handle_for_stack`] or
    /// [`Profile::handle_for_stack_frames`].
    ///
    /// The CPU delta is the amount of CPU time that the CPU was busy with work for this
    /// thread since the previous sample. It should always be less than or equal the
    /// time delta between the sample timestamps.
    ///
    /// The weight affects the sample's stack's score in the call tree. You usually set
    /// this to 1. You can use weights greater than one if you want to combine multiple
    /// adjacent samples with the same stack into one sample, to save space. However,
    /// this discards any CPU deltas between the adjacent samples, so it's only really
    /// useful if no CPU time has occurred between the samples, and for that use case the
    /// [`Profile::add_sample_same_stack_zero_cpu`] method should be preferred.
    ///
    /// You can can also set the weight to something negative, such as -1, to create a
    /// "diff profile". For example, if you have partitioned your samples into "before"
    /// and "after" groups, you can use -1 for all "before" samples and 1 for all "after"
    /// samples, and the call tree will show you which stacks occur more frequently in
    /// the "after" part of the profile, by sorting those stacks to the top.
    pub fn add_sample(
        &mut self,
        thread: ThreadHandle,
        timestamp: Timestamp,
        stack: Option<StackHandle>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        let stack_index = match stack {
            Some(StackHandle(stack_thread_handle, stack_index)) => {
                assert_eq!(
                    stack_thread_handle, thread,
                    "StackHandle from different thread passed to Profile::add_sample"
                );
                Some(stack_index)
            }
            None => None,
        };
        self.threads[thread.0].add_sample(timestamp, stack_index, cpu_delta, weight);
    }

    /// Add a sample with a CPU delta of zero. Internally, multiple consecutive
    /// samples with a delta of zero will be combined into one sample with an accumulated
    /// weight.
    pub fn add_sample_same_stack_zero_cpu(
        &mut self,
        thread: ThreadHandle,
        timestamp: Timestamp,
        weight: i32,
    ) {
        self.threads[thread.0].add_sample_same_stack_zero_cpu(timestamp, weight);
    }

    /// Add an allocation or deallocation sample to the *main* thread of the given
    /// process. This is used to collect stacks showing where allocations and
    /// deallocations happened.
    ///
    /// Can only be called once the main thread for `process` has been created.
    /// `stack` must be a stack handle which is valid for that main thread.
    ///
    /// # Details
    ///
    /// When loading profiles with allocation samples in the Firefox Profiler, the
    /// UI will display a dropdown above the call tree to switch between regular
    /// samples and allocation samples.
    ///
    /// An allocation sample has a timestamp, a stack, a memory address, and an allocation size.
    ///
    /// The size should be in bytes, with positive values for allocations and negative
    /// values for deallocations.
    ///
    /// The memory address allows correlating the allocation and deallocation stacks of the
    /// same object. This lets the UI display just the stacks for objects which haven't
    /// been deallocated yet ("Retained memory").
    ///
    /// To avoid having to capture stacks for every single allocation, you can sample just
    /// a subset of allocations. The sampling should be done based on the allocation size
    /// ("probability per byte"). The decision whether to sample should be done at
    /// allocation time and remembered for the lifetime of the allocation, so that for
    /// each allocated object you either sample both its allocation and deallocation, or
    /// neither.
    ///
    /// To get the stack handle, you can use [`Profile::handle_for_stack`] or
    /// [`Profile::handle_for_stack_frames`].
    ///
    /// # Panics
    ///
    /// Panics if the `stack` handle is not valid for the main thread of `process`.
    ///
    /// # Main thread requirement
    ///
    /// Allocations are per-process, because you can allocate something one one thread
    /// and then free it on a different thread, and you'll still want those two operations
    /// to be matched up in a view that shows the retained memory.
    ///
    /// Unfortunately, `StackHandle` is currently per-thread. This method will become more
    /// ergonomic once the profile format has changed so that `StackHandle`s can be used
    /// across all threads of a profile. In the meantime, unfortunately you must manually
    /// ensure that you create the stack handle for the main thread of the given process.
    pub fn add_allocation_sample(
        &mut self,
        process: ProcessHandle,
        timestamp: Timestamp,
        stack: Option<StackHandle>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        let process = &self.processes[process.0];
        let Some(allocation_thread) = process.thread_handle_for_allocations() else {
            panic!("Profile::add_allocation_sample called for a thread whose process does not have a main thread");
        };
        let stack_index = match stack {
            Some(StackHandle(stack_thread, stack_index)) => {
                assert_eq!(
                    stack_thread, allocation_thread,
                    "StackHandle from different thread passed to Profile::add_allocation_sample"
                );
                Some(stack_index)
            }
            None => None,
        };
        self.threads[allocation_thread.0].add_allocation_sample(
            timestamp,
            stack_index,
            allocation_address,
            allocation_size,
        );
    }

    /// Registers a marker type for a [`RuntimeSchemaMarkerSchema`]. You only need to call this for
    /// marker types whose schema is dynamically created at runtime.
    ///
    /// After you register the marker type, you'll save its [`MarkerTypeHandle`] somewhere, and then
    /// store it in every marker you create of this type. The marker then needs to return the
    /// handle from its implementation of [`Marker::marker_type`].
    ///
    /// For marker types whose schema is known at compile time, you'll want to implement
    /// [`StaticSchemaMarker`] instead, and you don't need to call this method.
    pub fn register_marker_type(&mut self, schema: RuntimeSchemaMarkerSchema) -> MarkerTypeHandle {
        let handle = MarkerTypeHandle(self.marker_schemas.len());
        self.marker_schemas.push(schema.into());
        handle
    }

    /// Returns the marker type handle for a type that implements [`StaticSchemaMarker`].
    ///
    /// You usually don't need to call this, ever. It is called by the blanket impl
    /// of [`Marker::marker_type`] for all types which implement [`StaticSchemaMarker`].
    pub fn static_schema_marker_type<T: StaticSchemaMarker>(&mut self) -> MarkerTypeHandle {
        if let Some(handle) = self
            .static_schema_marker_types
            .get(T::UNIQUE_MARKER_TYPE_NAME)
        {
            return *handle;
        }

        let handle = MarkerTypeHandle(self.marker_schemas.len());
        let schema = InternalMarkerSchema::from_static_schema::<T>(self);
        self.marker_schemas.push(schema);
        self.static_schema_marker_types
            .insert(T::UNIQUE_MARKER_TYPE_NAME, handle);
        handle
    }

    /// Add a marker to the given thread.
    ///
    /// The marker handle that's returned by this method can be used in [`Profile::set_marker_stack`].
    ///
    /// ```
    /// use fxprof_processed_profile::{
    ///     Profile, Category, CategoryColor, Marker, MarkerStringFieldFormat, MarkerTiming,
    ///     StaticSchema, StaticSchemaMarker, StaticSchemaMarkerField, StringHandle,
    ///     ThreadHandle, Timestamp,
    /// };
    ///
    /// # fn fun() {
    /// # let profile: Profile = panic!();
    /// # let thread: ThreadHandle = panic!();
    /// # let start_time: Timestamp = panic!();
    /// # let end_time: Timestamp = panic!();
    /// let name = profile.handle_for_string("Marker name");
    /// let text = profile.handle_for_string("Marker text");
    /// let my_marker = TextMarker { name, text };
    /// profile.add_marker(thread, MarkerTiming::Interval(start_time, end_time), my_marker);
    /// # }
    ///
    /// #[derive(Debug, Clone)]
    /// pub struct TextMarker {
    ///   pub name: StringHandle,
    ///   pub text: StringHandle,
    /// }
    ///
    /// impl StaticSchemaMarker for TextMarker {
    ///     type FieldsType = StringHandle;
    ///
    ///     const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
    ///
    ///     const CHART_LABEL: Option<&'static str> = Some("{marker.data.text}");
    ///     const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.text}");
    ///
    ///     const FIELDS: StaticSchema<Self::FieldsType> = StaticSchema(StaticSchemaMarkerField::new(
    ///         "text",
    ///         "Contents",
    ///         MarkerStringFieldFormat::String,
    ///     ));
    ///
    ///     fn name(&self, _profile: &mut Profile) -> StringHandle {
    ///         self.name
    ///     }
    ///
    ///     fn field_values(&self) -> StringHandle {
    ///         self.text
    ///     }
    /// }
    /// ```
    pub fn add_marker<T: Marker>(
        &mut self,
        thread: ThreadHandle,
        timing: MarkerTiming,
        marker: T,
    ) -> MarkerHandle {
        let marker_type = marker.marker_type(self);
        let name = marker.name(self);
        let schema = &self.marker_schemas[marker_type.0];
        let thread_obj = &mut self.threads[thread.0];
        thread_obj.add_marker(
            &mut self.string_table,
            name,
            marker_type,
            schema,
            marker,
            timing,
        )
    }

    /// Sets a marker's stack. Every marker can have an optional stack, regardless
    /// of its marker type.
    ///
    /// A marker's stack is shown in its tooltip, and in the sidebar in the marker table
    /// panel if a marker with a stack is selected.
    ///
    /// To get the stack handle, you can use [`Profile::handle_for_stack`] or
    /// [`Profile::handle_for_stack_frames`].
    pub fn set_marker_stack(
        &mut self,
        thread: ThreadHandle,
        marker: MarkerHandle,
        stack: Option<StackHandle>,
    ) {
        let stack_index = match stack {
            Some(StackHandle(stack_thread_handle, stack_index)) => {
                assert_eq!(
                    stack_thread_handle, thread,
                    "StackHandle from different thread passed to Profile::add_sample"
                );
                Some(stack_index)
            }
            None => None,
        };
        self.threads[thread.0].set_marker_stack(marker, stack_index);
    }

    /// Add a data point to a counter. For a memory counter, `value_delta` is the number
    /// of bytes that have been allocated / deallocated since the previous counter sample, and
    /// `number_of_operations` is the number of `malloc` / `free` calls since the previous
    /// counter sample. Both numbers are deltas.
    ///
    /// The graph in the profiler UI will connect subsequent data points with diagonal lines.
    /// Counters are intended for values that are measured at a time-based sample rate; for example,
    /// you could add a counter sample once every millisecond with the current memory usage.
    ///
    /// Alternatively, you can emit a new data point only whenever the value changes.
    /// In that case you probably want to emit two values per change: one right before (with
    /// the old value) and one right at the timestamp of change (with the new value). This way
    /// you'll get more horizontal lines, and the diagonal line will be very short.
    pub fn add_counter_sample(
        &mut self,
        counter: CounterHandle,
        timestamp: Timestamp,
        value_delta: f64,
        number_of_operations_delta: u32,
    ) {
        self.counters[counter.0].add_sample(timestamp, value_delta, number_of_operations_delta)
    }

    /// Returns a Vec listing the libraries and addresses used by any native frames
    /// in this profile, i.e. by frames created with
    /// [`handle_for_frame_with_address`](Profile::handle_for_frame_with_address) or
    /// [`handle_for_frame_with_address_and_symbol`](Profile::handle_for_frame_with_address_and_symbol).
    ///
    /// You can create a symbolicated profile after-the-fact by looking up symbol information
    /// for all listed libraries and addresses and calling [`Profile::make_symbolicated_profile`]
    /// with it.
    pub fn native_frame_addresses_per_library(&self) -> Vec<(LibraryHandle, BTreeSet<u32>)> {
        let mut collector = self.global_libs.address_collector();
        for thread in &self.threads {
            thread.gather_used_rvas(&mut collector);
        }
        collector.finish(&self.global_libs)
    }

    /// Create a symbolicated profile by combining this profile with the
    /// supplied `symbol_info`.
    ///
    /// The old profile is consumed when this function is called.
    ///
    /// The following handles remain valid in the new profile: [`ProcessHandle`],
    /// [`LibraryHandle`], [`ThreadHandle`], [`CategoryHandle`], [`SubcategoryHandle`],
    /// [`CounterHandle`], [`MarkerTypeHandle`], [`MarkerHandle`]
    ///
    /// The following handles from the original profile **are not valid** in the
    /// new profile: [`NativeSymbolHandle`], [`FrameHandle`], [`StackHandle`], [`StringHandle`]
    ///
    /// # Details
    ///
    /// This method re-creates any native frames (i.e. frames with addresses) whose library is
    /// present in `symbol_info`. All other frames, specifically any native frames from different
    /// libraries, as well as non-native frames, are retained as-is - but their [`FrameHandle`]s may
    /// change.
    ///
    /// If any of the recreated frames already had symbol info, the existing symbol info is
    /// discarded.
    ///
    /// Unlike [`Profile::handle_for_frame_with_address`], this way of symbolicating does not
    /// attempt to get symbol information any existing library symbol tables that have been
    /// supplied using [`Profile::set_lib_symbol_table`].
    ///
    /// Note: This method does not set the new profile's `symbolicated` property to true.
    /// You may want to do so yourself using [`Profile::set_symbolicated`]. This will ensure
    /// that the Firefox Profiler front-end won't attempt to symbolicate the profile another
    /// time.
    ///
    /// # Usage
    ///
    /// 1. Create an unsymbolicated profile first, using [`Profile::handle_for_frame_with_address`]
    ///    for any native frames.
    /// 2. Get a list of libraries and addresses to symbolicate from [`Profile::native_frame_addresses_per_library`].
    /// 3. Look up symbols for those libraries and addresses, for example using the `wholesym` crate.
    ///    You can consult `presymbolicate.rs` in the `samply` source code for inspiration.
    /// 4. Assemble a [`ProfileSymbolInfo`] object with all the obtained symbol info.
    /// 5. Call this method, and then call `profile.set_symbolicated(true)` on the result.
    /// 6. Save the new profile.
    pub fn make_symbolicated_profile(self, symbol_info: &ProfileSymbolInfo) -> Profile {
        let ProfileSymbolInfo {
            string_table: symbol_string_table,
            lib_symbols,
        } = symbol_info;
        let lib_symbols: BTreeMap<GlobalLibIndex, &LibSymbolInfo> = lib_symbols
            .iter()
            .map(|lib_symbols| {
                let lib_index = self
                    .global_libs
                    .used_lib_index(lib_symbols.lib_handle)
                    .unwrap();
                (lib_index, lib_symbols)
            })
            .collect();
        let libs: FastHashSet<GlobalLibIndex> = lib_symbols.keys().cloned().collect();

        let Profile {
            product,
            os_name,
            interval,
            timeline_unit,
            global_libs,
            kernel_libs,
            categories,
            processes,
            counters,
            threads,
            initial_visible_threads,
            initial_selected_threads,
            reference_timestamp,
            platform_specific_reference_timestamp,
            mut string_table,
            marker_schemas,
            static_schema_marker_types,
            symbolicated,
            used_pids,
            used_tids,
        } = self;

        let mut strings = StringTableAdapter::new(symbol_string_table, &mut string_table);
        let threads: Vec<Thread> = threads
            .into_iter()
            .map(|thread| thread.make_symbolicated_thread(&libs, &lib_symbols, &mut strings))
            .collect();

        // TODO: Remove unused hex address strings from the string table.

        Profile {
            product,
            os_name,
            interval,
            timeline_unit,
            global_libs,
            kernel_libs,
            categories,
            processes,
            counters,
            threads,
            initial_visible_threads,
            initial_selected_threads,
            reference_timestamp,
            platform_specific_reference_timestamp,
            string_table,
            marker_schemas,
            static_schema_marker_types,
            symbolicated,
            used_pids,
            used_tids,
        }
    }

    fn resolve_frame_address(
        process: &mut Process,
        frame_address: FrameAddress,
        global_libs: &mut GlobalLibTable,
        kernel_libs: &mut LibMappings<LibraryHandle>,
        string_table: &mut ProfileStringTable,
    ) -> InternalFrameAddress {
        match frame_address {
            FrameAddress::InstructionPointer(ip) => {
                process.convert_address(global_libs, kernel_libs, string_table, ip)
            }
            FrameAddress::ReturnAddress(ra) => process.convert_address(
                global_libs,
                kernel_libs,
                string_table,
                ra.saturating_sub(1),
            ),
            FrameAddress::AdjustedReturnAddress(ara) => {
                process.convert_address(global_libs, kernel_libs, string_table, ara)
            }
            FrameAddress::RelativeAddressFromInstructionPointer(lib_handle, relative_address) => {
                let global_lib_index = global_libs.index_for_used_lib(lib_handle, string_table);
                InternalFrameAddress::InLib(relative_address, global_lib_index)
            }
            FrameAddress::RelativeAddressFromReturnAddress(lib_handle, relative_address) => {
                let global_lib_index = global_libs.index_for_used_lib(lib_handle, string_table);
                let adjusted_relative_address = relative_address.saturating_sub(1);
                InternalFrameAddress::InLib(adjusted_relative_address, global_lib_index)
            }
            FrameAddress::RelativeAddressFromAdjustedReturnAddress(
                lib_handle,
                adjusted_relative_address,
            ) => {
                let global_lib_index = global_libs.index_for_used_lib(lib_handle, string_table);
                InternalFrameAddress::InLib(adjusted_relative_address, global_lib_index)
            }
        }
    }

    /// Set whether the profile is already symbolicated.
    ///
    /// Read: whether symbols are resolved.
    ///
    /// If your samples refer to labels instead of addresses, it is safe
    /// to set to true.
    ///
    /// Setting to true prevents the Firefox Profiler from attempting to
    /// resolve symbols.
    ///
    /// By default, this is set to false. This causes the Firefox Profiler
    /// to look up symbols for any address-based frame, i.e. any frame
    /// which was created via [`Profile::handle_for_frame_with_address`] or
    /// [`Profile::handle_for_frame_with_address_and_symbol`].
    ///
    /// If you use address-based frames and supply your own symbols using
    /// [`Profile::add_lib`] or [`Profile::set_lib_symbol_table`], you can
    /// choose to set this to true and avoid another symbol lookup, or you
    /// can leave it set to false if there is a way to obtain richer symbol
    /// information than the information supplied in those symbol tables.
    ///
    /// For example, when samply creates a profile which includes JIT frames,
    /// and there is a Jitdump file with symbol information about those JIT
    /// frames, samply uses [`Profile::set_lib_symbol_table`] to provide
    /// the function names for the JIT functions. But it does not call
    /// [`Profile::set_symbolicated`] with true, because the Jitdump files may
    /// include additional information that's not in the [`SymbolTable`],
    /// specifically the Jitdump file may have file name and line number information.
    /// This information is only added into the profile by the Firefox Profiler's
    /// resolution of symbols: The Firefox Profiler requests symbol information
    /// for the JIT frame addresses from samply's symbol server, at which point
    /// samply obtains the richer information from the Jitdump file and returns
    /// it via the symbol server response.
    pub fn set_symbolicated(&mut self, v: bool) {
        self.symbolicated = v;
    }

    /// Returns a flattened list of `ThreadHandle`s in the right order.
    ///
    // The processed profile format has all threads from all processes in a flattened threads list.
    // Each thread duplicates some information about its process, which allows the Firefox Profiler
    // UI to group threads from the same process.
    fn sorted_threads(&self) -> (Vec<ThreadHandle>, Vec<usize>, Vec<usize>) {
        let mut sorted_threads = Vec::with_capacity(self.threads.len());
        let mut first_thread_index_per_process = vec![0; self.processes.len()];
        let mut new_thread_indices = vec![0; self.threads.len()];

        let mut sorted_processes: Vec<_> = (0..self.processes.len()).map(ProcessHandle).collect();
        sorted_processes.sort_by(|a_handle, b_handle| {
            let a = &self.processes[a_handle.0];
            let b = &self.processes[b_handle.0];
            a.cmp_for_json_order(b)
        });

        for process in sorted_processes {
            let prev_len = sorted_threads.len();
            first_thread_index_per_process[process.0] = prev_len;
            sorted_threads.extend_from_slice(self.processes[process.0].threads());

            let sorted_threads_for_this_process = &mut sorted_threads[prev_len..];
            sorted_threads_for_this_process.sort_by(|a_handle, b_handle| {
                let a = &self.threads[a_handle.0];
                let b = &self.threads[b_handle.0];
                a.cmp_for_json_order(b)
            });

            for (i, v) in sorted_threads_for_this_process.iter().enumerate() {
                new_thread_indices[v.0] = prev_len + i;
            }
        }

        (
            sorted_threads,
            first_thread_index_per_process,
            new_thread_indices,
        )
    }

    fn serializable_threads<'a>(
        &'a self,
        sorted_threads: &'a [ThreadHandle],
    ) -> SerializableProfileThreadsProperty<'a> {
        SerializableProfileThreadsProperty {
            threads: &self.threads,
            processes: &self.processes,
            sorted_threads,
            marker_schemas: &self.marker_schemas,
            string_table: &self.string_table,
        }
    }

    fn serializable_counters<'a>(
        &'a self,
        first_thread_index_per_process: &'a [usize],
    ) -> SerializableProfileCountersProperty<'a> {
        SerializableProfileCountersProperty {
            counters: &self.counters,
            first_thread_index_per_process,
        }
    }

    fn contains_js_frame(&self) -> bool {
        self.threads.iter().any(|t| t.contains_js_frame())
    }
}

impl Serialize for Profile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (sorted_threads, first_thread_index_per_process, new_thread_indices) =
            self.sorted_threads();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("meta", &SerializableProfileMeta(self, &new_thread_indices))?;
        map.serialize_entry("libs", &self.global_libs)?;
        map.serialize_entry("shared", &SerializableProfileShared(self))?;
        map.serialize_entry("threads", &self.serializable_threads(&sorted_threads))?;
        map.serialize_entry("pages", &[] as &[()])?;
        map.serialize_entry("profilerOverhead", &[] as &[()])?;
        map.serialize_entry(
            "counters",
            &self.serializable_counters(&first_thread_index_per_process),
        )?;
        map.end()
    }
}

struct SerializableProfileMeta<'a>(&'a Profile, &'a [usize]);

impl Serialize for SerializableProfileMeta<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("categories", &self.0.categories)?;
        map.serialize_entry("debug", &false)?;
        map.serialize_entry(
            "extensions",
            &json!({
                "length": 0,
                "baseURL": [],
                "id": [],
                "name": [],
            }),
        )?;
        map.serialize_entry("interval", &(self.0.interval.as_secs_f64() * 1000.0))?;
        map.serialize_entry("preprocessedProfileVersion", &57)?;
        map.serialize_entry("processType", &0)?;
        map.serialize_entry("product", &self.0.product)?;
        if let Some(os_name) = &self.0.os_name {
            map.serialize_entry("oscpu", os_name)?;
        }
        let time_unit = match self.0.timeline_unit {
            TimelineUnit::Milliseconds => "ms",
            TimelineUnit::Bytes => "bytes",
        };
        map.serialize_entry(
            "sampleUnits",
            &json!({
                "time": &time_unit,
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
            }),
        )?;
        map.serialize_entry("startTime", &self.0.reference_timestamp)?;
        match &self.0.platform_specific_reference_timestamp {
            Some(PlatformSpecificReferenceTimestamp::ClockMonotonicNanosecondsSinceBoot(val)) => {
                map.serialize_entry("startTimeAsClockMonotonicNanosecondsSinceBoot", &val)?;
            }
            Some(PlatformSpecificReferenceTimestamp::MachAbsoluteTimeNanoseconds(val)) => {
                map.serialize_entry("startTimeAsMachAbsoluteTimeNanoseconds", &val)?;
            }
            Some(PlatformSpecificReferenceTimestamp::QueryPerformanceCounterValue(val)) => {
                map.serialize_entry("startTimeAsQueryPerformanceCounterValue", &val)?;
            }
            None => {}
        }
        map.serialize_entry("symbolicated", &self.0.symbolicated)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("version", &24)?; // this version is ignored, only "preprocessedProfileVersion" is used
        map.serialize_entry("usesOnlyOneStackType", &(!self.0.contains_js_frame()))?;
        map.serialize_entry("sourceCodeIsNotOnSearchfox", &true)?;

        let mut marker_schemas: Vec<InternalMarkerSchema> = self.0.marker_schemas.clone();
        marker_schemas.sort_by(|a, b| a.type_name().cmp(b.type_name()));
        map.serialize_entry("markerSchema", &marker_schemas)?;

        if !self.0.initial_visible_threads.is_empty() {
            map.serialize_entry(
                "initialVisibleThreads",
                &self
                    .0
                    .initial_visible_threads
                    .iter()
                    .map(|x| self.1[x.0])
                    .collect::<Vec<_>>(),
            )?;
        }

        if !self.0.initial_selected_threads.is_empty() {
            map.serialize_entry(
                "initialSelectedThreads",
                &self
                    .0
                    .initial_selected_threads
                    .iter()
                    .map(|x| self.1[x.0])
                    .collect::<Vec<_>>(),
            )?;
        };

        map.end()
    }
}

struct SerializableProfileShared<'a>(&'a Profile);

impl Serialize for SerializableProfileShared<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("stringArray", &self.0.string_table)?;
        map.end()
    }
}

struct SerializableProfileThreadsProperty<'a> {
    threads: &'a [Thread],
    processes: &'a [Process],
    sorted_threads: &'a [ThreadHandle],
    marker_schemas: &'a [InternalMarkerSchema],
    string_table: &'a ProfileStringTable,
}

impl Serialize for SerializableProfileThreadsProperty<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.threads.len()))?;

        for thread in self.sorted_threads {
            let thread = &self.threads[thread.0];
            let process = &self.processes[thread.process().0];
            let marker_schemas = self.marker_schemas;
            let string_table = self.string_table;
            seq.serialize_element(&SerializableProfileThread(
                process,
                thread,
                marker_schemas,
                string_table,
            ))?;
        }

        seq.end()
    }
}

struct SerializableProfileCountersProperty<'a> {
    counters: &'a [Counter],
    first_thread_index_per_process: &'a [usize],
}

impl Serialize for SerializableProfileCountersProperty<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.counters.len()))?;

        for counter in self.counters {
            let main_thread_index = self.first_thread_index_per_process[counter.process().0];
            seq.serialize_element(&counter.as_serializable(main_thread_index))?;
        }

        seq.end()
    }
}

struct SerializableProfileThread<'a>(
    &'a Process,
    &'a Thread,
    &'a [InternalMarkerSchema],
    &'a ProfileStringTable,
);

impl Serialize for SerializableProfileThread<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SerializableProfileThread(process, thread, marker_schemas, string_table) = self;
        let process_start_time = process.start_time();
        let process_end_time = process.end_time();
        let process_name = process.name();
        let pid = process.pid();
        thread.serialize_with(
            serializer,
            process_start_time,
            process_end_time,
            process_name,
            pid,
            marker_schemas,
            string_table,
        )
    }
}
