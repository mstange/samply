use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::json;

use crate::category::{Category, CategoryHandle, CategoryPairHandle};
use crate::category_color::CategoryColor;
use crate::counters::{Counter, CounterHandle};
use crate::cpu_delta::CpuDelta;
use crate::fast_hash_map::FastHashMap;
use crate::frame::{Frame, FrameInfo};
use crate::frame_table::{InternalFrame, InternalFrameLocation};
use crate::global_lib_table::{GlobalLibTable, LibraryHandle, UsedLibraryAddressesIterator};
use crate::lib_mappings::LibMappings;
use crate::library_info::{LibraryInfo, SymbolTable};
use crate::markers::{
    InternalMarkerSchema, Marker, MarkerHandle, MarkerSchema, MarkerTiming, MarkerTypeHandle,
    StaticSchemaMarker,
};
use crate::process::{Process, ThreadHandle};
use crate::reference_timestamp::ReferenceTimestamp;
use crate::string_table::{GlobalStringIndex, GlobalStringTable};
use crate::thread::{ProcessHandle, Thread};
use crate::timestamp::Timestamp;

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

/// A handle for an interned string, returned from [`Profile::intern_string`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct StringHandle(pub(crate) GlobalStringIndex);

/// Stores the profile data and can be serialized as JSON, via [`serde::Serialize`].
///
/// The profile data is organized into a list of processes with threads.
/// Each thread has its own samples and markers.
///
/// ```
/// use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, Frame, FrameInfo, FrameFlags, SamplingInterval, Timestamp};
/// use std::time::SystemTime;
///
/// # fn write_profile(output_file: std::fs::File) -> Result<(), Box<dyn std::error::Error>> {
/// let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
/// let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
/// let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
/// profile.set_thread_name(thread, "Main thread");
/// let stack = vec![
///     FrameInfo { frame: Frame::Label(profile.intern_string("Root node")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() },
///     FrameInfo { frame: Frame::Label(profile.intern_string("First callee")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() }
/// ];
/// profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), stack.into_iter(), CpuDelta::ZERO, 1);
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
    pub(crate) global_libs: GlobalLibTable,
    pub(crate) kernel_libs: LibMappings<LibraryHandle>,
    pub(crate) categories: Vec<Category>, // append-only for stable CategoryHandles
    pub(crate) processes: Vec<Process>,   // append-only for stable ProcessHandles
    pub(crate) counters: Vec<Counter>,
    pub(crate) threads: Vec<Thread>, // append-only for stable ThreadHandles
    pub(crate) reference_timestamp: ReferenceTimestamp,
    pub(crate) string_table: GlobalStringTable,
    pub(crate) marker_schemas: Vec<InternalMarkerSchema>,
    static_schema_marker_types: FastHashMap<&'static str, MarkerTypeHandle>,
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
        Profile {
            interval,
            product: product.to_string(),
            os_name: None,
            threads: Vec::new(),
            global_libs: GlobalLibTable::new(),
            kernel_libs: LibMappings::new(),
            reference_timestamp,
            processes: Vec::new(),
            string_table: GlobalStringTable::new(),
            marker_schemas: Vec::new(),
            categories: vec![Category {
                name: "Other".to_string(),
                color: CategoryColor::Gray,
                subcategories: Vec::new(),
            }],
            static_schema_marker_types: FastHashMap::default(),
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

    /// Change the product name.
    pub fn set_product(&mut self, product: &str) {
        self.product = product.to_string();
    }

    /// Set the name of the operating system.
    pub fn set_os_name(&mut self, os_name: &str) {
        self.os_name = Some(os_name.to_string());
    }

    /// Add a category and return its handle.
    ///
    /// Categories are used for stack frames and markers, as part of a "category pair".
    pub fn add_category(&mut self, name: &str, color: CategoryColor) -> CategoryHandle {
        let handle = CategoryHandle(self.categories.len() as u16);
        self.categories.push(Category {
            name: name.to_string(),
            color,
            subcategories: Vec::new(),
        });
        handle
    }

    /// Add a subcategory for a category, and return the "category pair" handle.
    pub fn add_subcategory(&mut self, category: CategoryHandle, name: &str) -> CategoryPairHandle {
        let subcategory = self.categories[category.0 as usize].add_subcategory(name.into());
        CategoryPairHandle(category, Some(subcategory))
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
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let suffix = *entry.get();
                *entry.get_mut() += 1;
                format!("{id}.{suffix}")
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
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
    /// use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, Frame, SamplingInterval, Timestamp};
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

    /// Get the [`LibraryHandle`] for a library. This handle is used in [`Profile::add_lib_mapping`]
    /// and in the pre-resolved [`Frame`] variants.
    ///
    /// Knowing the library information allows symbolication of native stacks once the
    /// profile is opened in the Firefox Profiler.
    pub fn add_lib(&mut self, library: LibraryInfo) -> LibraryHandle {
        self.global_libs.handle_for_lib(library)
    }

    /// Set the symbol table for a library.
    ///
    /// This symbol table can also be specified in the [`LibraryInfo`] which is given to
    /// [`Profile::add_lib`]. However, sometimes you may want to have the [`LibraryHandle`]
    /// for a library before you know about all its symbols. In those cases, you can call
    /// [`Profile::add_lib`] with `symbol_table` set to `None`, and then supply the symbol
    /// table afterwards.
    ///
    /// Symbol tables are optional.
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
    /// The new mapping will be respected by future [`Profile::add_sample`] calls, when resolving
    /// absolute frame addresses to library-relative addresses.
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
    /// unloaded, so that future calls to [`Profile::add_sample`] know about the removal.
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
    /// global across all processes. Future calls to [`Profile::add_sample`] with native
    /// frames resolve the frame's code address with respect to the currently loaded kernel
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
    /// unloaded, so that future calls to [`Profile::add_sample`] know about the unloading.
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
        self.processes[process.0].add_thread(handle);
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

    /// Turn the string into in a [`StringHandle`], for use in [`Frame::Label`].
    pub fn intern_string(&mut self, s: &str) -> StringHandle {
        StringHandle(self.string_table.index_for_string(s))
    }

    /// Get the string for a string handle. This is sometimes useful when writing tests.
    ///
    /// Panics if the handle wasn't found, which can happen if you pass a handle
    /// from a different Profile instance.
    pub fn get_string(&self, handle: StringHandle) -> &str {
        self.string_table.get_string(handle.0).unwrap()
    }

    /// Add a sample to the given thread.
    ///
    /// The sample has a timestamp, a stack, a CPU delta, and a weight.
    ///
    /// The stack frames are supplied as an iterator. Every frame has an associated
    /// category pair. The stack frames are ordered from root to leaf, or caller-most
    /// to callee-most.
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
        frames: impl Iterator<Item = FrameInfo>,
        cpu_delta: CpuDelta,
        weight: i32,
    ) {
        let stack_index = self.stack_index_for_frames(thread, frames);
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

    /// Add an allocation or deallocation sample to the given thread. This is used
    /// to collect stacks showing where allocations and deallocations happened.
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
    /// The stack frames are supplied as an iterator. Every frame has an associated
    /// category pair.
    pub fn add_allocation_sample(
        &mut self,
        thread: ThreadHandle,
        timestamp: Timestamp,
        frames: impl Iterator<Item = FrameInfo>,
        allocation_address: u64,
        allocation_size: i64,
    ) {
        // The profile format strictly separates sample data from different threads.
        // For allocation samples, this separation is a bit unfortunate, especially
        // when it comes to the "Retained Memory" panel which shows allocation stacks
        // for just objects that haven't been deallocated yet. This panel is per-thread,
        // and it needs to know about deallocations even if they happened on a different
        // thread from the allocation.
        // To resolve this conundrum, for now, we will put all allocation and deallocation
        // samples on a single thread per process, regardless of what thread they actually
        // happened on.
        // The Gecko profiler puts all allocation samples on the main thread, for example.
        // Here in fxprof-processed-profile, we just deem the first thread of each process
        // as the processes "allocation thread".
        let process_handle = self.threads[thread.0].process();
        let process = &self.processes[process_handle.0];
        let allocation_thread_handle = process.thread_handle_for_allocations().unwrap();
        let stack_index = self.stack_index_for_frames(allocation_thread_handle, frames);
        self.threads[allocation_thread_handle.0].add_allocation_sample(
            timestamp,
            stack_index,
            allocation_address,
            allocation_size,
        );
    }

    /// Registers a marker type, given the type's [`MarkerSchema`]. Usually you only need to call this for
    /// marker types whose schema is dynamically created at runtime.
    ///
    /// After you register the marker type, you'll save its [`MarkerTypeHandle`] somewhere, and then
    /// store it in every marker you create of this type. The marker then needs to return the
    /// handle from its implementation of [`Marker::marker_type`].
    ///
    /// For marker types whose schema is known at compile time, you'll want to implement
    /// [`StaticSchemaMarker`] instead.
    pub fn register_marker_type(&mut self, schema: MarkerSchema) -> MarkerTypeHandle {
        let handle = MarkerTypeHandle(self.marker_schemas.len());
        self.marker_schemas.push(schema.into());
        handle
    }

    /// Returns the marker type handle for a type that implements [`StaticSchemaMarker`].
    ///
    /// You usually don't need to call this, ever. It is called by the blanket impl
    /// of [`Marker::marker_type`] for all types which implement [`StaticSchemaMarker`].
    pub fn static_schema_marker_type<T: StaticSchemaMarker>(&mut self) -> MarkerTypeHandle {
        match self
            .static_schema_marker_types
            .entry(T::UNIQUE_MARKER_TYPE_NAME)
        {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let handle = MarkerTypeHandle(self.marker_schemas.len());
                self.marker_schemas.push(T::schema().into());
                entry.insert(handle);
                handle
            }
        }
    }

    /// Add a marker to the given thread.
    ///
    /// The marker handle that's returned by this method can be used in [`Profile::set_marker_stack`].
    ///
    /// ```
    /// use fxprof_processed_profile::{
    ///     Profile, Marker, MarkerTiming, MarkerLocation, MarkerFieldFormat, MarkerSchema,
    ///     MarkerFieldSchema, StaticSchemaMarker, CategoryHandle, StringHandle, ThreadHandle,
    ///     Timestamp,
    /// };
    ///
    /// # fn fun() {
    /// # let profile: Profile = panic!();
    /// # let thread: ThreadHandle = panic!();
    /// # let start_time: Timestamp = panic!();
    /// # let end_time: Timestamp = panic!();
    /// let name = profile.intern_string("Marker name");
    /// let text = profile.intern_string("Marker text");
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
    ///     const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
    ///
    ///     fn schema() -> MarkerSchema {
    ///         MarkerSchema {
    ///             type_name: Self::UNIQUE_MARKER_TYPE_NAME.into(),
    ///             locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
    ///             chart_label: Some("{marker.data.text}".into()),
    ///             tooltip_label: None,
    ///             table_label: Some("{marker.name} - {marker.data.text}".into()),
    ///             fields: vec![MarkerFieldSchema {
    ///                 key: "text".into(),
    ///                 label: "Contents".into(),
    ///                 format: MarkerFieldFormat::String,
    ///                 searchable: true,
    ///             }],
    ///             static_fields: vec![],
    ///         }
    ///     }
    ///
    ///     fn name(&self, _profile: &mut Profile) -> StringHandle {
    ///         self.name
    ///     }
    ///
    ///     fn category(&self, _profile: &mut Profile) -> CategoryHandle {
    ///         CategoryHandle::OTHER
    ///     }
    ///
    ///     fn string_field_value(&self, _field_index: u32) -> StringHandle {
    ///         self.text
    ///     }
    ///
    ///     fn number_field_value(&self, _field_index: u32) -> f64 {
    ///         unreachable!()
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
        let category = marker.category(self);
        let thread = &mut self.threads[thread.0];
        let name_thread_string_index = thread.convert_string_index(&self.string_table, name.0);
        let schema = &self.marker_schemas[marker_type.0];
        thread.add_marker(
            name_thread_string_index,
            marker_type,
            schema,
            marker,
            timing,
            category,
            &mut self.string_table,
        )
    }

    /// Sets a marker's stack. Every marker can have an optional stack, regardless
    /// of its marker type.
    ///
    /// The stack frames are supplied as an iterator. Just like in [`Profile::add_sample`],
    /// the stack frames are ordered from root to leaf, or caller-most to callee-most.
    ///
    /// A marker's stack is shown in its tooltip, and in the sidebar in the marker table
    /// panel if a marker with a stack is selected.
    pub fn set_marker_stack(
        &mut self,
        thread: ThreadHandle,
        marker: MarkerHandle,
        stack_frames: impl Iterator<Item = FrameInfo>,
    ) {
        let stack_index = self.stack_index_for_frames(thread, stack_frames);
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

    // frames is ordered from caller to callee, i.e. root function first, pc last
    fn stack_index_for_frames(
        &mut self,
        thread: ThreadHandle,
        frames: impl Iterator<Item = FrameInfo>,
    ) -> Option<usize> {
        let thread = &mut self.threads[thread.0];
        let process = &mut self.processes[thread.process().0];
        let mut prefix = None;
        for frame_info in frames {
            let location = match frame_info.frame {
                Frame::InstructionPointer(ip) => {
                    process.convert_address(&mut self.global_libs, &mut self.kernel_libs, ip)
                }
                Frame::ReturnAddress(ra) => process.convert_address(
                    &mut self.global_libs,
                    &mut self.kernel_libs,
                    ra.saturating_sub(1),
                ),
                Frame::AdjustedReturnAddress(ara) => {
                    process.convert_address(&mut self.global_libs, &mut self.kernel_libs, ara)
                }
                Frame::RelativeAddressFromInstructionPointer(lib_handle, relative_address) => {
                    let global_lib_index = self.global_libs.index_for_used_lib(lib_handle);
                    InternalFrameLocation::AddressInLib(relative_address, global_lib_index)
                }
                Frame::RelativeAddressFromReturnAddress(lib_handle, relative_address) => {
                    let global_lib_index = self.global_libs.index_for_used_lib(lib_handle);
                    let adjusted_relative_address = relative_address.saturating_sub(1);
                    InternalFrameLocation::AddressInLib(adjusted_relative_address, global_lib_index)
                }
                Frame::RelativeAddressFromAdjustedReturnAddress(
                    lib_handle,
                    adjusted_relative_address,
                ) => {
                    let global_lib_index = self.global_libs.index_for_used_lib(lib_handle);
                    InternalFrameLocation::AddressInLib(adjusted_relative_address, global_lib_index)
                }
                Frame::Label(string_index) => {
                    let thread_string_index =
                        thread.convert_string_index(&self.string_table, string_index.0);
                    InternalFrameLocation::Label(thread_string_index)
                }
            };
            let internal_frame = InternalFrame {
                location,
                flags: frame_info.flags,
                category_pair: frame_info.category_pair,
            };
            let frame_index = thread.frame_index_for_frame(internal_frame, &mut self.global_libs);
            prefix =
                Some(thread.stack_index_for_stack(prefix, frame_index, frame_info.category_pair));
        }
        prefix
    }

    /// Returns a flattened list of `ThreadHandle`s in the right order.
    ///
    // The processed profile format has all threads from all processes in a flattened threads list.
    // Each thread duplicates some information about its process, which allows the Firefox Profiler
    // UI to group threads from the same process.
    fn sorted_threads(&self) -> (Vec<ThreadHandle>, Vec<usize>) {
        let mut sorted_threads = Vec::with_capacity(self.threads.len());
        let mut first_thread_index_per_process = vec![0; self.processes.len()];

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
        }

        (sorted_threads, first_thread_index_per_process)
    }

    fn serializable_threads<'a>(
        &'a self,
        sorted_threads: &'a [ThreadHandle],
    ) -> SerializableProfileThreadsProperty<'a> {
        SerializableProfileThreadsProperty {
            threads: &self.threads,
            processes: &self.processes,
            categories: &self.categories,
            sorted_threads,
            marker_schemas: &self.marker_schemas,
            global_string_table: &self.string_table,
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

    fn contains_js_function(&self) -> bool {
        self.threads.iter().any(|t| t.contains_js_function())
    }

    pub fn lib_used_rva_iter(&self) -> UsedLibraryAddressesIterator {
        self.global_libs.lib_used_rva_iter()
    }
}

impl Serialize for Profile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (sorted_threads, first_thread_index_per_process) = self.sorted_threads();
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("meta", &SerializableProfileMeta(self))?;
        map.serialize_entry("libs", &self.global_libs)?;
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

struct SerializableProfileMeta<'a>(&'a Profile);

impl<'a> Serialize for SerializableProfileMeta<'a> {
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
        map.serialize_entry("preprocessedProfileVersion", &49)?;
        map.serialize_entry("processType", &0)?;
        map.serialize_entry("product", &self.0.product)?;
        if let Some(os_name) = &self.0.os_name {
            map.serialize_entry("oscpu", os_name)?;
        }
        map.serialize_entry(
            "sampleUnits",
            &json!({
                "time": "ms",
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
            }),
        )?;
        map.serialize_entry("startTime", &self.0.reference_timestamp)?;
        map.serialize_entry("symbolicated", &false)?;
        map.serialize_entry("pausedRanges", &[] as &[()])?;
        map.serialize_entry("version", &24)?;
        map.serialize_entry("usesOnlyOneStackType", &(!self.0.contains_js_function()))?;
        map.serialize_entry("doesNotUseFrameImplementation", &true)?;
        map.serialize_entry("sourceCodeIsNotOnSearchfox", &true)?;

        let mut marker_schemas: Vec<InternalMarkerSchema> = self.0.marker_schemas.clone();
        marker_schemas.sort_by(|a, b| a.type_name().cmp(b.type_name()));
        map.serialize_entry("markerSchema", &marker_schemas)?;

        map.end()
    }
}

struct SerializableProfileThreadsProperty<'a> {
    threads: &'a [Thread],
    processes: &'a [Process],
    categories: &'a [Category],
    sorted_threads: &'a [ThreadHandle],
    marker_schemas: &'a [InternalMarkerSchema],
    global_string_table: &'a GlobalStringTable,
}

impl<'a> Serialize for SerializableProfileThreadsProperty<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.threads.len()))?;

        for thread in self.sorted_threads {
            let categories = self.categories;
            let thread = &self.threads[thread.0];
            let process = &self.processes[thread.process().0];
            let marker_schemas = self.marker_schemas;
            let global_string_table = self.global_string_table;
            seq.serialize_element(&SerializableProfileThread(
                process,
                thread,
                categories,
                marker_schemas,
                global_string_table,
            ))?;
        }

        seq.end()
    }
}

struct SerializableProfileCountersProperty<'a> {
    counters: &'a [Counter],
    first_thread_index_per_process: &'a [usize],
}

impl<'a> Serialize for SerializableProfileCountersProperty<'a> {
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
    &'a [Category],
    &'a [InternalMarkerSchema],
    &'a GlobalStringTable,
);

impl<'a> Serialize for SerializableProfileThread<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SerializableProfileThread(
            process,
            thread,
            categories,
            marker_schemas,
            global_string_table,
        ) = self;
        let process_start_time = process.start_time();
        let process_end_time = process.end_time();
        let process_name = process.name();
        let pid = process.pid();
        thread.serialize_with(
            serializer,
            categories,
            process_start_time,
            process_end_time,
            process_name,
            pid,
            marker_schemas,
            global_string_table,
        )
    }
}
