use byteorder::LittleEndian;
use debugid::{CodeId, DebugId};

use framehop::{FrameAddress, Module, ModuleSvmaInfo, ModuleUnwindData, TextByteData, Unwinder};
use fxprof_processed_profile::{
    CpuDelta, LibraryInfo, Profile, ReferenceTimestamp, SamplingInterval, ThreadHandle, Timestamp,
};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{DsoInfo, DsoKey, Endianness};
use linux_perf_event_reader::constants::PERF_CONTEXT_MAX;
use linux_perf_event_reader::{
    CommOrExecRecord, CommonData, ContextSwitchRecord, ForkOrExitRecord, Mmap2FileId, Mmap2Record,
    MmapRecord, RawDataU64, SampleRecord,
};
use memmap2::Mmap;
use object::pe::{ImageNtHeaders32, ImageNtHeaders64};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile};
use object::{
    CompressedFileRange, CompressionFormat, FileKind, Object, ObjectSection, ObjectSegment,
};
use rangemap::RangeSet;
use samply_symbols::{debug_id_for_object, DebugIdExt};
use wholesym::samply_symbols;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::{ops::Range, path::Path};

use super::context_switch::{ContextSwitchHandler, OffCpuSampleGroup};
use super::convert_regs::ConvertRegs;
use super::event_interpretation::EventInterpretation;
use super::injected_jit_object::{correct_bad_perf_jit_so_file, jit_function_name};
use super::kernel_symbols::{kernel_module_build_id, KernelSymbols};
use super::mmap_range_or_vec::MmapRangeOrVec;
use super::processes::Processes;
use super::rss_stat::{RssStat, MM_ANONPAGES, MM_FILEPAGES, MM_SHMEMPAGES, MM_SWAPENTS};
use super::svma_file_range::compute_vma_bias;

use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::marker_file::{get_markers, MarkerSpan};
use crate::shared::process_sample_data::RssStatMember;
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{
    UnresolvedSamples, UnresolvedStackHandle, UnresolvedStacks,
};
use crate::shared::utils::open_file_with_fallback;

pub type BoxedProductNameGenerator = Box<dyn FnOnce(&str) -> String>;

/// See [`Converter::check_for_pe_mapping`].
#[derive(Debug, Clone)]
struct SuspectedPeMapping {
    path: Vec<u8>,
    start: u64,
    size: u64,
}

pub struct Converter<U>
where
    U: Unwinder<Module = Module<MmapRangeOrVec>> + Default,
{
    cache: U::Cache,
    profile: Profile,
    processes: Processes<U>,
    timestamp_converter: TimestampConverter,
    current_sample_time: u64,
    build_ids: HashMap<DsoKey, DsoInfo>,
    endian: Endianness,
    delayed_product_name_generator: Option<BoxedProductNameGenerator>,
    linux_version: Option<String>,
    extra_binary_artifact_dir: Option<PathBuf>,
    context_switch_handler: ContextSwitchHandler,
    unresolved_stacks: UnresolvedStacks,
    off_cpu_weight_per_sample: i32,
    have_context_switches: bool,
    event_names: Vec<String>,
    kernel_symbols: Option<KernelSymbols>,

    /// Mapping of start address to potential mapped PE binaries.
    /// The key is equal to the start field of the value.
    suspected_pe_mappings: BTreeMap<u64, SuspectedPeMapping>,

    jit_category_manager: JitCategoryManager,

    /// Whether repeated frames at the base of the stack should be folded
    /// into one frame.
    fold_recursive_prefix: bool,

    marker_spans: Vec<MarkerSpan>,

    /// If Some(), only include samples which occurred during these ranges.
    sample_ranges: Option<RangeSet<Timestamp>>,
}

const DEFAULT_OFF_CPU_SAMPLING_INTERVAL_NS: u64 = 1_000_000; // 1ms

impl<U> Converter<U>
where
    U: Unwinder<Module = Module<MmapRangeOrVec>> + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        product: &str,
        delayed_product_name_generator: Option<BoxedProductNameGenerator>,
        build_ids: HashMap<DsoKey, DsoInfo>,
        linux_version: Option<&str>,
        first_sample_time: u64,
        endian: Endianness,
        cache: U::Cache,
        extra_binary_artifact_dir: Option<&Path>,
        interpretation: EventInterpretation,
        merge_threads: bool,
        fold_recursive_prefix: bool,
        marker_file: Option<&str>,
        marker_name_prefix_for_filtering: Option<&str>,
    ) -> Self {
        let interval = match interpretation.sampling_is_time_based {
            Some(nanos) => SamplingInterval::from_nanos(nanos),
            None => SamplingInterval::from_millis(1),
        };
        let profile = Profile::new(
            product,
            ReferenceTimestamp::from_system_time(SystemTime::now()),
            interval,
        );
        let (off_cpu_sampling_interval_ns, off_cpu_weight_per_sample) =
            match &interpretation.sampling_is_time_based {
                Some(interval_ns) => (*interval_ns, 1),
                None => (DEFAULT_OFF_CPU_SAMPLING_INTERVAL_NS, 0),
            };
        let kernel_symbols = match KernelSymbols::new_for_running_kernel() {
            Ok(kernel_symbols) => Some(kernel_symbols),
            Err(err) => {
                eprintln!("Could not obtain kernel symbols: {err}");
                None
            }
        };
        let timestamp_converter = TimestampConverter {
            reference_raw: first_sample_time,
            raw_to_ns_factor: 1,
        };
        let (marker_spans, sample_ranges) = match marker_file {
            Some(marker_file) => get_markers(
                marker_file,
                marker_name_prefix_for_filtering,
                timestamp_converter,
            )
            .expect("Could not get markers"),
            None => (Vec::new(), None),
        };

        Self {
            profile,
            cache,
            processes: Processes::new(merge_threads),
            timestamp_converter,
            current_sample_time: first_sample_time,
            build_ids,
            endian,
            delayed_product_name_generator,
            linux_version: linux_version.map(ToOwned::to_owned),
            extra_binary_artifact_dir: extra_binary_artifact_dir.map(ToOwned::to_owned),
            off_cpu_weight_per_sample,
            context_switch_handler: ContextSwitchHandler::new(off_cpu_sampling_interval_ns),
            unresolved_stacks: UnresolvedStacks::default(),
            have_context_switches: interpretation.have_context_switches,
            event_names: interpretation.event_names,
            kernel_symbols,
            suspected_pe_mappings: BTreeMap::new(),
            jit_category_manager: JitCategoryManager::new(),
            fold_recursive_prefix,
            marker_spans,
            sample_ranges,
        }
    }

    pub fn finish(mut self) -> Profile {
        let mut profile = self.profile;
        self.processes.finish(
            &mut profile,
            &self.unresolved_stacks,
            &self.event_names,
            &mut self.jit_category_manager,
            &self.timestamp_converter,
            &self.marker_spans,
            self.sample_ranges.as_ref(),
        );
        profile
    }

    pub fn handle_main_event_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: &SampleRecord,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        let tid = e.tid.expect("Can't handle samples without tids");
        let timestamp = e
            .timestamp
            .expect("Can't handle samples without timestamps");
        self.current_sample_time = timestamp;

        let profile_timestamp = self.timestamp_converter.convert_time(timestamp);

        let process = self.processes.get_by_pid(pid, &mut self.profile);
        process.check_jitdump(
            &mut self.jit_category_manager,
            &mut self.profile,
            &self.timestamp_converter,
        );

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(
            e,
            &process.unwinder,
            &mut self.cache,
            &mut stack,
            self.fold_recursive_prefix,
        );

        let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);

        if thread.last_sample_timestamp == Some(timestamp) {
            // Duplicate sample. Ignore.
            return;
        }

        thread.last_sample_timestamp = Some(timestamp);
        let thread_handle = thread.profile_thread;

        // Consume off-cpu time and clear any saved off-CPU stack.
        let off_cpu_sample = self
            .context_switch_handler
            .handle_on_cpu_sample(timestamp, &mut thread.context_switch_data);
        if let (Some(off_cpu_sample), Some(off_cpu_stack)) =
            (off_cpu_sample, thread.off_cpu_stack.take())
        {
            let cpu_delta_ns = self
                .context_switch_handler
                .consume_cpu_delta(&mut thread.context_switch_data);
            process_off_cpu_sample_group(
                off_cpu_sample,
                thread_handle,
                cpu_delta_ns,
                &self.timestamp_converter,
                self.off_cpu_weight_per_sample,
                off_cpu_stack,
                &mut process.unresolved_samples,
            );
        }

        let cpu_delta = if self.have_context_switches {
            CpuDelta::from_nanos(
                self.context_switch_handler
                    .consume_cpu_delta(&mut thread.context_switch_data),
            )
        } else if let Some(period) = e.period {
            // If the observed perf event is one of the clock time events, or cycles, then we should convert it to a CpuDelta.
            // TODO: Detect event type
            CpuDelta::from_nanos(period)
        } else {
            CpuDelta::from_nanos(0)
        };

        let stack_index = self.unresolved_stacks.convert(stack.iter().rev().cloned());
        process.unresolved_samples.add_sample(
            thread_handle,
            profile_timestamp,
            timestamp,
            stack_index,
            cpu_delta,
            1,
            None,
        );
    }

    pub fn handle_sched_switch_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: &SampleRecord,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        let tid = e.tid.expect("Can't handle samples without tids");
        let process = self.processes.get_by_pid(pid, &mut self.profile);
        process.check_jitdump(
            &mut self.jit_category_manager,
            &mut self.profile,
            &self.timestamp_converter,
        );

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(
            e,
            &process.unwinder,
            &mut self.cache,
            &mut stack,
            self.fold_recursive_prefix,
        );

        let stack_index = self
            .unresolved_stacks
            .convert_no_kernel(stack.iter().rev().cloned());
        let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);
        thread.off_cpu_stack = Some(stack_index);
    }

    pub fn handle_rss_stat_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: &SampleRecord,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        // let tid = e.tid.expect("Can't handle samples without tids");
        let process = self.processes.get_by_pid(pid, &mut self.profile);

        let Some(raw) = e.raw else { return };
        let Ok(rss_stat) = RssStat::parse(
            raw,
            self.endian,

        ) else { return };

        let Some(timestamp_mono) = e.timestamp else {
            eprintln!("rss_stat record doesn't have a timestamp");
            return;
        };
        let timestamp = self.timestamp_converter.convert_time(timestamp_mono);

        let (prev_size_of_this_member, member) = match rss_stat.member {
            MM_FILEPAGES => (
                &mut process.prev_mm_filepages_size,
                RssStatMember::ResidentFileMappingPages,
            ),
            MM_ANONPAGES => (
                &mut process.prev_mm_anonpages_size,
                RssStatMember::ResidentAnonymousPages,
            ),
            MM_SHMEMPAGES => (
                &mut process.prev_mm_shmempages_size,
                RssStatMember::ResidentSharedMemoryPages,
            ),
            MM_SWAPENTS => (
                &mut process.prev_mm_swapents_size,
                RssStatMember::AnonymousSwapEntries,
            ),
            _ => return,
        };

        let delta = rss_stat.size - *prev_size_of_this_member;
        *prev_size_of_this_member = rss_stat.size;

        if rss_stat.member == MM_ANONPAGES {
            let counter = process.get_or_make_mem_counter(&mut self.profile);
            self.profile
                .add_counter_sample(counter, timestamp, delta as f64, 1);
        }

        process.check_jitdump(
            &mut self.jit_category_manager,
            &mut self.profile,
            &self.timestamp_converter,
        );

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(
            e,
            &process.unwinder,
            &mut self.cache,
            &mut stack,
            self.fold_recursive_prefix,
        );
        let unresolved_stack = self.unresolved_stacks.convert(stack.into_iter().rev());
        let thread_handle = process.threads.main_thread.profile_thread;
        process.unresolved_samples.add_rss_stat_marker(
            thread_handle,
            timestamp,
            timestamp_mono,
            unresolved_stack,
            member,
            rss_stat.size,
            delta,
        );
    }

    pub fn handle_other_event_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: &SampleRecord,
        attr_index: usize,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        let timestamp_mono = e
            .timestamp
            .expect("Can't handle samples without timestamps");
        let timestamp = self.timestamp_converter.convert_time(timestamp_mono);
        // let tid = e.tid.expect("Can't handle samples without tids");
        let process = self.processes.get_by_pid(pid, &mut self.profile);
        process.check_jitdump(
            &mut self.jit_category_manager,
            &mut self.profile,
            &self.timestamp_converter,
        );

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(
            e,
            &process.unwinder,
            &mut self.cache,
            &mut stack,
            self.fold_recursive_prefix,
        );

        let thread_handle = match e.tid {
            Some(tid) => {
                process
                    .threads
                    .get_thread_by_tid(tid, &mut self.profile)
                    .profile_thread
            }
            None => process.threads.main_thread.profile_thread,
        };

        let unresolved_stack = self.unresolved_stacks.convert(stack.into_iter().rev());
        process.unresolved_samples.add_other_event_marker(
            thread_handle,
            timestamp,
            timestamp_mono,
            unresolved_stack,
            attr_index,
        );
    }

    /// Get the stack contained in this sample, and put it into `stack`.
    ///
    /// We can have both the kernel stack and the user stack, or just one of
    /// them, or neither. The stack is appended onto the `stack` outparameter,
    /// ordered from callee-most ("innermost") to caller-most. The kernel
    /// stack comes before the user stack.
    ///
    /// If the `SampleRecord` has a kernel stack, it's always in `e.callchain`.
    ///
    /// If this sample has a user stack, its source depends on the method of
    /// stackwalking that was requested during recording:
    ///
    ///  - With frame pointer unwinding (the default on x86, `perf record -g`,
    ///    or more explicitly `perf record --call-graph fp`), the user stack
    ///    is walked during sampling by the kernel and appended to e.callchain.
    ///  - With DWARF unwinding (`perf record --call-graph dwarf`), the raw
    ///    bytes on the stack are just copied into the perf.data file, and we
    ///    need to do the unwinding now, based on the register values in
    ///    `e.user_regs` and the raw stack bytes in `e.user_stack`.
    fn get_sample_stack<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        e: &SampleRecord,
        unwinder: &U,
        cache: &mut U::Cache,
        stack: &mut Vec<StackFrame>,
        fold_recursive_prefix: bool,
    ) {
        stack.truncate(0);

        // CpuMode::from_misc(e.raw.misc)

        // Get the first fragment of the stack from e.callchain.
        if let Some(callchain) = e.callchain {
            let mut is_first_frame = true;
            let mut mode = StackMode::from(e.cpu_mode);
            for i in 0..callchain.len() {
                let address = callchain.get(i).unwrap();
                if address >= PERF_CONTEXT_MAX {
                    if let Some(new_mode) = StackMode::from_context_frame(address) {
                        mode = new_mode;
                    }
                    continue;
                }

                let stack_frame = match is_first_frame {
                    true => StackFrame::InstructionPointer(address, mode),
                    false => StackFrame::ReturnAddress(address, mode),
                };
                stack.push(stack_frame);

                is_first_frame = false;
            }
        }

        // Append the user stack with the help of DWARF unwinding.
        if let (Some(regs), Some((user_stack, _))) = (&e.user_regs, e.user_stack) {
            let ustack_bytes = RawDataU64::from_raw_data::<LittleEndian>(user_stack);
            let (pc, sp, regs) = C::convert_regs(regs);
            let mut read_stack = |addr: u64| {
                // ustack_bytes has the stack bytes starting from the current stack pointer.
                let offset = addr.checked_sub(sp).ok_or(())?;
                let index = usize::try_from(offset / 8).map_err(|_| ())?;
                ustack_bytes.get(index).ok_or(())
            };

            // Unwind.
            let mut frames = unwinder.iter_frames(pc, regs, cache, &mut read_stack);
            loop {
                let frame = match frames.next() {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(_) => {
                        stack.push(StackFrame::TruncatedStackMarker);
                        break;
                    }
                };
                let stack_frame = match frame {
                    FrameAddress::InstructionPointer(addr) => {
                        StackFrame::InstructionPointer(addr, StackMode::User)
                    }
                    FrameAddress::ReturnAddress(addr) => {
                        StackFrame::ReturnAddress(addr.into(), StackMode::User)
                    }
                };
                stack.push(stack_frame);
            }
        }

        if stack.is_empty() {
            if let Some(ip) = e.ip {
                stack.push(StackFrame::InstructionPointer(ip, e.cpu_mode.into()));
            }
        } else if fold_recursive_prefix {
            let last_frame = *stack.last().unwrap();
            while stack.len() >= 2 && stack[stack.len() - 2] == last_frame {
                stack.pop();
            }
        }
    }

    /// This is a terrible hack to get binary correlation working with apps on Wine.
    ///
    /// Unlike ELF, PE has the notion of "file alignment" that is different from page alignment.
    /// Hence, even if the virtual address is page aligned, its on-disk offset may not be. This
    /// leads to obvious trouble with using mmap, since mmap requires the file offset to be page
    /// aligned. Wine's workaround is straightforward: for misaligned sections, Wine will simply
    /// copy the image from disk instead of mmapping them. For example, `/proc/<pid>/maps` can look
    /// like this:
    ///
    /// ```plain
    /// <PE header> 140000000-140001000 r--p 00000000 00:25 272185   game.exe
    /// <.text>     140001000-143be8000 r-xp 00000000 00:00 0
    ///             143be8000-144c0c000 r--p 00000000 00:00 0
    /// ```
    ///
    /// When this misalignment happens, most of the sections in the memory will not be a file
    /// mapping. However, the PE header is always mapped, and it resides at the beginning of the
    /// file, which means it's also always *aligned*. Finally, it's always mapped first, because
    /// the information from the header is required to determine the load address of the other
    /// sections. Hence, if we find a mapping that seems to pointing to a PE file, and has a file
    /// offset of 0, we'll add it to the list of "suspected PE images". When we see a later mapping
    /// that belongs to one of the suspected PE ranges, we'll match the mapping with the file,
    /// which allows binary correlation and unwinding to work.
    fn check_for_pe_mapping(&mut self, path_slice: &[u8], mapping_start_avma: u64) {
        // Do a quick extension check first, to avoid end up trying to parse every mmapped file.
        let filename_is_pe = path_slice.ends_with(b".exe")
            || path_slice.ends_with(b".dll")
            || path_slice.ends_with(b".EXE")
            || path_slice.ends_with(b".DLL");
        if !filename_is_pe {
            return;
        }

        // There are a few assumptions here:
        // - The SizeOfImage field in the PE header is defined to be a multiple of SectionAlignment.
        //   SectionAlignment is usually the page size. When it's not the page size, additional
        //   layout restrictions apply and Wine will always map the file in its entirety, which
        //   means we're safe without the workaround. So we can safely assume it to be page aligned
        //   here.
        // - VirtualAddress of the sections are defined to be adjacent after page-alignment. This
        //   means that we can treat the image as a contiguous region.
        if let Some(size) = get_pe_mapping_size(path_slice) {
            let mapping = SuspectedPeMapping {
                path: path_slice.to_owned(),
                start: mapping_start_avma,
                size,
            };
            self.suspected_pe_mappings.insert(mapping.start, mapping);
        }
    }

    pub fn handle_mmap(&mut self, e: MmapRecord, timestamp: u64) {
        let mut path = e.path.as_slice();
        if let Some(jitdump_path) = get_path_if_jitdump(&path) {
            let process = self.processes.get_by_pid(e.pid, &mut self.profile);
            process
                .jitdump_manager
                .add_jitdump_path(jitdump_path, self.extra_binary_artifact_dir.clone());
            return;
        }

        if e.page_offset == 0 {
            self.check_for_pe_mapping(&e.path.as_slice(), e.address);
        }

        if !e.is_executable {
            return;
        }

        let dso_key = match DsoKey::detect(&path, e.cpu_mode) {
            Some(dso_key) => dso_key,
            None => return,
        };
        let mut build_id = None;
        if let Some(dso_info) = self.build_ids.get(&dso_key) {
            build_id = Some(dso_info.build_id.to_owned());
            // Overwrite the path from the mmap record with the path from the build ID info.
            // These paths are usually the same, but in some cases the path from the build
            // ID info can be "better". For example, the synthesized mmap event for the
            // kernel vmlinux image usually has "[kernel.kallsyms]_text" whereas the build
            // ID info might have the full path to a kernel debug file, e.g.
            // "/usr/lib/debug/boot/vmlinux-4.16.0-1-amd64".
            path = dso_info.path.to_owned().into();
        }

        if e.pid == -1 {
            self.add_kernel_module(e.address, e.length, dso_key, build_id.as_deref(), &path);
        } else {
            self.add_module_to_process(
                e.pid,
                &path,
                e.page_offset,
                e.address,
                e.length,
                build_id.as_deref(),
                timestamp,
            );
        }
    }

    pub fn handle_mmap2(&mut self, e: Mmap2Record, timestamp: u64) {
        let path = e.path.as_slice();
        if let Some(jitdump_path) = get_path_if_jitdump(&path) {
            let process = self.processes.get_by_pid(e.pid, &mut self.profile);
            process
                .jitdump_manager
                .add_jitdump_path(jitdump_path, self.extra_binary_artifact_dir.clone());
            return;
        }

        if e.page_offset == 0 {
            self.check_for_pe_mapping(&e.path.as_slice(), e.address);
        }

        const PROT_EXEC: u32 = 0b100;
        if e.protection & PROT_EXEC == 0 {
            // Ignore non-executable mappings.
            return;
        }

        let build_id = match &e.file_id {
            Mmap2FileId::BuildId(build_id) => Some(build_id.to_owned()),
            Mmap2FileId::InodeAndVersion(_) => {
                let dso_key = match DsoKey::detect(&path, e.cpu_mode) {
                    Some(dso_key) => dso_key,
                    None => return,
                };
                self.build_ids
                    .get(&dso_key)
                    .map(|db| db.build_id.to_owned())
            }
        };

        self.add_module_to_process(
            e.pid,
            &path,
            e.page_offset,
            e.address,
            e.length,
            build_id.as_deref(),
            timestamp,
        );
    }

    pub fn handle_context_switch(&mut self, e: ContextSwitchRecord, common: CommonData) {
        let pid = common.pid.expect("Can't handle samples without pids");
        let tid = common.tid.expect("Can't handle samples without tids");
        let timestamp = common
            .timestamp
            .expect("Can't handle context switch without time");
        let process = self.processes.get_by_pid(pid, &mut self.profile);
        let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);

        match e {
            ContextSwitchRecord::In { .. } => {
                // Consume off-cpu time and clear the saved off-CPU stack.
                let off_cpu_sample = self
                    .context_switch_handler
                    .handle_switch_in(timestamp, &mut thread.context_switch_data);
                if let (Some(off_cpu_sample), Some(off_cpu_stack)) =
                    (off_cpu_sample, thread.off_cpu_stack.take())
                {
                    let cpu_delta_ns = self
                        .context_switch_handler
                        .consume_cpu_delta(&mut thread.context_switch_data);
                    process_off_cpu_sample_group(
                        off_cpu_sample,
                        thread.profile_thread,
                        cpu_delta_ns,
                        &self.timestamp_converter,
                        self.off_cpu_weight_per_sample,
                        off_cpu_stack,
                        &mut process.unresolved_samples,
                    );
                }
            }
            ContextSwitchRecord::Out { .. } => {
                self.context_switch_handler
                    .handle_switch_out(timestamp, &mut thread.context_switch_data);
            }
        }
    }

    /// Called for a FORK record.
    ///
    /// FORK records are emitted if a new thread is started or if a new
    /// process is created. The name is inherited from the forking thread.
    pub fn handle_fork(&mut self, e: ForkOrExitRecord) {
        let start_time = self.timestamp_converter.convert_time(e.timestamp);

        let is_main = e.pid == e.tid;
        let parent_process = self.processes.get_by_pid(e.ppid, &mut self.profile);
        if e.pid != e.ppid {
            // We've created a new process.
            if !is_main {
                eprintln!("Unexpected data in FORK record: If we fork into a different process, the forked child thread should be the main thread of the new process");
            }
            let parent_process_name = parent_process.name.clone();
            self.processes.recycle_or_get_new(
                e.pid,
                parent_process_name,
                start_time,
                &mut self.profile,
            );
        } else {
            let parent_thread = parent_process
                .threads
                .get_thread_by_tid(e.ptid, &mut self.profile);
            let parent_thread_name = parent_thread.name.clone();
            parent_process.recycle_or_get_new_thread(
                e.tid,
                parent_thread_name,
                start_time,
                &mut self.profile,
            );
        }
    }

    /// Called for an EXIT record.
    pub fn handle_exit(&mut self, e: ForkOrExitRecord) {
        let is_main = e.pid == e.tid;
        let end_time = self.timestamp_converter.convert_time(e.timestamp);
        if is_main {
            self.processes.remove(
                e.pid,
                end_time,
                &mut self.profile,
                &mut self.jit_category_manager,
                &self.timestamp_converter,
            );
        } else {
            let process = self.processes.get_by_pid(e.pid, &mut self.profile);
            process
                .threads
                .remove_non_main_thread(e.tid, end_time, &mut self.profile);
        }
    }

    pub fn handle_comm(&mut self, e: CommOrExecRecord, timestamp: Option<u64>) {
        let is_main = e.pid == e.tid;
        let name = e.name.as_slice();
        let name = String::from_utf8_lossy(&name);

        // If the COMM record doesn't have a timestamp, take the last seen
        // timestamp from the previous sample.
        let timestamp_mono = match timestamp {
            Some(0) | None => self.current_sample_time,
            Some(ts) => ts,
        };
        let timestamp = self.timestamp_converter.convert_time(timestamp_mono);

        if is_main && self.delayed_product_name_generator.is_some() && name != "perf-exec" {
            let generator = self.delayed_product_name_generator.take().unwrap();
            let product = generator(&name);
            self.profile.set_product(&product);
        }

        if e.is_execve {
            // Mark the old thread / process as ended.
            if is_main {
                self.processes.remove(
                    e.pid,
                    timestamp,
                    &mut self.profile,
                    &mut self.jit_category_manager,
                    &self.timestamp_converter,
                );
                self.processes.recycle_or_get_new(
                    e.pid,
                    Some(name.to_string()),
                    timestamp,
                    &mut self.profile,
                );
            } else {
                eprintln!(
                    "Unexpected is_execve on non-main thread! pid: {}, tid: {}",
                    e.pid, e.tid
                );
                let process = self.processes.get_by_pid(e.pid, &mut self.profile);
                process
                    .threads
                    .remove_non_main_thread(e.tid, timestamp, &mut self.profile);
                process.recycle_or_get_new_thread(
                    e.tid,
                    Some(name.to_string()),
                    timestamp,
                    &mut self.profile,
                );
            }
        } else if is_main {
            self.processes
                .rename_process(e.pid, timestamp, name.to_string(), &mut self.profile);
        } else {
            let process = self.processes.get_by_pid(e.pid, &mut self.profile);
            process.threads.rename_non_main_thread(
                e.tid,
                timestamp,
                name.to_string(),
                &mut self.profile,
            );
        }
    }

    #[allow(unused)]
    pub fn register_existing_thread(&mut self, pid: i32, tid: i32, name: &str) {
        let is_main = pid == tid;

        let process = self.processes.get_by_pid(pid, &mut self.profile);
        let process_handle = process.profile_process;

        let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);
        let thread_handle = thread.profile_thread;

        self.profile.set_thread_name(thread_handle, name);
        thread.name = Some(name.to_owned());
        if is_main {
            self.profile.set_process_name(process_handle, name);
            process.name = Some(name.to_owned());
        }

        // Mark this as the start time of the new thread / process.
        let time = self
            .timestamp_converter
            .convert_time(self.current_sample_time);
        self.profile.set_thread_start_time(thread_handle, time);
        if is_main {
            self.profile.set_process_start_time(process_handle, time);
        }

        if self.delayed_product_name_generator.is_some() && name != "perf-exec" {
            let generator = self.delayed_product_name_generator.take().unwrap();
            let product = generator(name);
            self.profile.set_product(&product);
        }
    }

    fn add_kernel_module(
        &mut self,
        base_address: u64,
        len: u64,
        dso_key: DsoKey,
        build_id: Option<&[u8]>,
        path: &[u8],
    ) {
        let path = std::str::from_utf8(path).unwrap().to_string();
        let build_id: Option<Vec<u8>> = match (build_id, self.kernel_symbols.as_ref()) {
            (None, Some(kernel_symbols)) if kernel_symbols.base_avma == base_address => {
                Some(kernel_symbols.build_id.clone())
            }
            (None, _) => {
                kernel_module_build_id(Path::new(&path), self.extra_binary_artifact_dir.as_deref())
            }
            (Some(build_id), _) => Some(build_id.to_owned()),
        };
        let debug_id = build_id
            .as_deref()
            .map(|id| DebugId::from_identifier(id, self.endian == Endianness::LittleEndian));

        let debug_path = match self.linux_version.as_deref() {
            Some(linux_version) if path.starts_with("[kernel.kallsyms]") => {
                // Take a guess at the vmlinux debug file path.
                format!("/usr/lib/debug/boot/vmlinux-{linux_version}")
            }
            _ => path.clone(),
        };
        let symbol_table = match (&dso_key, &build_id, self.kernel_symbols.as_ref()) {
            (DsoKey::Kernel, Some(build_id), Some(kernel_symbols))
                if build_id == &kernel_symbols.build_id && kernel_symbols.base_avma != 0 =>
            {
                // Run `echo '0' | sudo tee /proc/sys/kernel/kptr_restrict` to get here without root.
                Some(kernel_symbols.symbol_table.clone())
            }
            _ => None,
        };

        let lib_handle = self.profile.add_lib(LibraryInfo {
            debug_id: debug_id.unwrap_or_default(),
            path,
            debug_path,
            code_id: build_id.map(|build_id| CodeId::from_binary(&build_id).to_string()),
            name: dso_key.name().to_string(),
            debug_name: dso_key.name().to_string(),
            arch: None,
            symbol_table,
        });
        self.profile
            .add_kernel_lib_mapping(lib_handle, base_address, base_address + len, 0);
    }

    /// Tell the unwinder about this module, and alsos create a ProfileModule
    /// and add it to the profile.
    ///
    /// The unwinder needs to know about it in case we need to do DWARF stack
    /// unwinding - it needs to get the unwinding information from the binary.
    /// The profile needs to know about this module so that it can assign
    /// addresses in the stack to the right module and so that symbolication
    /// knows where to get symbols for this module.
    #[allow(clippy::too_many_arguments)]
    fn add_module_to_process(
        &mut self,
        process_pid: i32,
        path_slice: &[u8],
        mapping_start_file_offset: u64,
        mapping_start_avma: u64,
        mapping_size: u64,
        build_id: Option<&[u8]>,
        timestamp: u64,
    ) {
        let process = self.processes.get_by_pid(process_pid, &mut self.profile);

        let path = std::str::from_utf8(path_slice).unwrap();
        let (mut file, mut path): (Option<_>, String) = match open_file_with_fallback(
            Path::new(path),
            self.extra_binary_artifact_dir.as_deref(),
        ) {
            Ok((file, path)) => (Some(file), path.to_string_lossy().to_string()),
            _ => (None, path.to_owned()),
        };

        let mut suspected_pe_mapping = None;
        if file.is_none() {
            suspected_pe_mapping = self
                .suspected_pe_mappings
                .range(..=mapping_start_avma)
                .next_back()
                .map(|(_, m)| m)
                .filter(|m| {
                    mapping_start_avma >= m.start
                        && mapping_start_avma + mapping_size <= m.start + m.size
                });
            if let Some(mapping) = suspected_pe_mapping {
                if let Ok((pe_file, pe_path)) = open_file_with_fallback(
                    Path::new(std::str::from_utf8(&mapping.path).unwrap()),
                    self.extra_binary_artifact_dir.as_deref(),
                ) {
                    file = Some(pe_file);
                    path = pe_path.to_string_lossy().to_string();
                }
            }
        }

        if file.is_none() && !path.starts_with('[') {
            // eprintln!("Could not open file {:?}", objpath);
        }

        // Fix up bad files from `perf inject --jit`.
        if let Some(file_inner) = &file {
            if let Some((fixed_file, fixed_path)) = correct_bad_perf_jit_so_file(file_inner, &path)
            {
                file = Some(fixed_file);
                path = fixed_path;
            }
        }

        let mapping_end_avma = mapping_start_avma + mapping_size;
        let avma_range = mapping_start_avma..mapping_end_avma;

        let name = Path::new(&path)
            .file_name()
            .map_or("<unknown>".into(), |f| f.to_string_lossy().to_string());

        if let Some(file) = file {
            let mmap = match unsafe { memmap2::MmapOptions::new().map(&file) } {
                Ok(mmap) => Arc::new(mmap),
                Err(err) => {
                    eprintln!("Could not mmap file {path}: {err:?}");
                    return;
                }
            };

            fn section_data<'a>(
                section: &impl ObjectSection<'a>,
                mmap: Arc<Mmap>,
            ) -> Option<MmapRangeOrVec> {
                let CompressedFileRange {
                    format,
                    offset,
                    compressed_size: _,
                    uncompressed_size,
                } = section.compressed_file_range().ok()?;
                match format {
                    CompressionFormat::None => {
                        MmapRangeOrVec::new_mmap_range(mmap, offset, uncompressed_size)
                    }
                    _ => Some(MmapRangeOrVec::Vec(Arc::new(
                        section.uncompressed_data().ok()?.to_vec(),
                    ))),
                }
            }

            let file = match object::File::parse(&mmap[..]) {
                Ok(file) => file,
                Err(_) => {
                    eprintln!("File {path} has unrecognized format");
                    return;
                }
            };

            // Verify build ID.
            if let Some(build_id) = build_id {
                match file.build_id().ok().flatten() {
                    Some(file_build_id) if build_id == file_build_id => {
                        // Build IDs match. Good.
                    }
                    Some(file_build_id) => {
                        let file_build_id = CodeId::from_binary(file_build_id);
                        let expected_build_id = CodeId::from_binary(build_id);
                        eprintln!(
                            "File {path} has non-matching build ID {file_build_id} (expected {expected_build_id})"
                        );
                        return;
                    }
                    None => {
                        eprintln!(
                            "File {path} does not contain a build ID, but we expected it to have one"
                        );
                        return;
                    }
                }
            }

            let base_svma = samply_symbols::relative_address_base(&file);
            let base_avma = if let Some(mapping) = suspected_pe_mapping {
                // For the PE correlation hack, we can't use the mapping offsets as they correspond to
                // an anonymous mapping. Instead, the base address is pre-determined from the PE header
                // mapping.
                mapping.start
            } else if let Some(bias) = compute_vma_bias(
                &file,
                mapping_start_file_offset,
                mapping_start_avma,
                mapping_size,
            ) {
                base_svma.wrapping_add(bias)
            } else {
                return;
            };

            let text = file.section_by_name(".text");
            let text_env = file.section_by_name("text_env");
            let eh_frame = file.section_by_name(".eh_frame");
            let got = file.section_by_name(".got");
            let eh_frame_hdr = file.section_by_name(".eh_frame_hdr");

            let unwind_data = match (
                eh_frame
                    .as_ref()
                    .and_then(|s| section_data(s, mmap.clone())),
                eh_frame_hdr
                    .as_ref()
                    .and_then(|s| section_data(s, mmap.clone())),
            ) {
                (Some(eh_frame), Some(eh_frame_hdr)) => {
                    ModuleUnwindData::EhFrameHdrAndEhFrame(eh_frame_hdr, eh_frame)
                }
                (Some(eh_frame), None) => ModuleUnwindData::EhFrame(eh_frame),
                (None, _) => ModuleUnwindData::None,
            };

            let text_data = if let Some(text_segment) = file
                .segments()
                .find(|segment| segment.name_bytes() == Ok(Some(b"__TEXT")))
            {
                let (start, size) = text_segment.file_range();
                let address_range = base_avma + start..base_avma + start + size;
                MmapRangeOrVec::new_mmap_range(mmap.clone(), start, size)
                    .map(|data| TextByteData::new(data, address_range))
            } else if let Some(text_section) = &text {
                if let Some((start, size)) = text_section.file_range() {
                    let address_range = base_avma + start..base_avma + start + size;
                    MmapRangeOrVec::new_mmap_range(mmap.clone(), start, size)
                        .map(|data| TextByteData::new(data, address_range))
                } else {
                    None
                }
            } else {
                None
            };

            fn svma_range<'a>(section: &impl ObjectSection<'a>) -> Range<u64> {
                section.address()..section.address() + section.size()
            }

            let module_svma_info = ModuleSvmaInfo {
                base_svma,
                text: text.as_ref().map(svma_range),
                text_env: text_env.as_ref().map(svma_range),
                stubs: None,
                stub_helper: None,
                eh_frame: eh_frame.as_ref().map(svma_range),
                eh_frame_hdr: eh_frame_hdr.as_ref().map(svma_range),
                got: got.as_ref().map(svma_range),
            };

            let module = Module::new(
                path.to_string(),
                avma_range.clone(),
                base_avma,
                module_svma_info,
                unwind_data,
                text_data,
            );
            process.unwinder.add_module(module);

            let debug_id = if let Some(debug_id) = debug_id_for_object(&file) {
                debug_id
            } else {
                return;
            };
            let code_id = file
                .build_id()
                .ok()
                .flatten()
                .map(|build_id| CodeId::from_binary(build_id).to_string());
            let lib_handle = self.profile.add_lib(LibraryInfo {
                debug_id,
                code_id,
                path: path.clone(),
                debug_path: path,
                debug_name: name.clone(),
                name: name.clone(),
                arch: None,
                symbol_table: None,
            });

            let relative_address_at_start = (avma_range.start - base_avma) as u32;

            if name.starts_with("jitted-") && name.ends_with(".so") {
                let symbol_name = jit_function_name(&file);
                process.add_lib_mapping_for_injected_jit_lib(
                    timestamp,
                    self.timestamp_converter.convert_time(timestamp),
                    symbol_name,
                    mapping_start_avma,
                    mapping_end_avma,
                    relative_address_at_start,
                    lib_handle,
                    &mut self.jit_category_manager,
                    &mut self.profile,
                );
            } else {
                process.add_regular_lib_mapping(
                    timestamp,
                    mapping_start_avma,
                    mapping_end_avma,
                    relative_address_at_start,
                    lib_handle,
                );
            }
        } else {
            // Without access to the binary file, make some guesses. We can't really
            // know what the right base address is because we don't have the section
            // information which lets us map between addresses and file offsets, but
            // often svmas and file offsets are the same, so this is a reasonable guess.
            let base_avma = mapping_start_avma - mapping_start_file_offset;
            let relative_address_at_start = (mapping_start_avma - base_avma) as u32;

            // If we have a build ID, convert it to a debug_id and a code_id.
            let debug_id = build_id
                .map(|id| DebugId::from_identifier(id, true)) // TODO: endian
                .unwrap_or_default();
            let code_id = build_id.map(|build_id| CodeId::from_binary(build_id).to_string());

            let lib_handle = self.profile.add_lib(LibraryInfo {
                debug_id,
                code_id,
                path: path.clone(),
                debug_path: path,
                debug_name: name.clone(),
                name,
                arch: None,
                symbol_table: None,
            });
            process.add_regular_lib_mapping(
                timestamp,
                mapping_start_avma,
                mapping_end_avma,
                relative_address_at_start,
                lib_handle,
            );
        }
    }
}

// #[test]
// fn test_my_jit() {
//     let data = std::fs::read("/Users/mstange/Downloads/jitted-123175-0-fixed.so").unwrap();
//     let file = object::File::parse(&data[..]).unwrap();
//     dbg!(jit_function_name(&file));
// }

fn process_off_cpu_sample_group(
    off_cpu_sample: OffCpuSampleGroup,
    thread_handle: ThreadHandle,
    cpu_delta_ns: u64,
    timestamp_converter: &TimestampConverter,
    off_cpu_weight_per_sample: i32,
    off_cpu_stack: UnresolvedStackHandle,
    samples: &mut UnresolvedSamples,
) {
    let OffCpuSampleGroup {
        begin_timestamp,
        end_timestamp,
        sample_count,
    } = off_cpu_sample;

    // Add a sample at the beginning of the paused range.
    // This "first sample" will carry any leftover accumulated running time ("cpu delta").
    let cpu_delta = CpuDelta::from_nanos(cpu_delta_ns);
    let weight = off_cpu_weight_per_sample;
    let stack = off_cpu_stack;
    let profile_timestamp = timestamp_converter.convert_time(begin_timestamp);
    samples.add_sample(
        thread_handle,
        profile_timestamp,
        begin_timestamp,
        stack,
        cpu_delta,
        weight,
        None,
    );

    if sample_count > 1 {
        // Emit a "rest sample" with a CPU delta of zero covering the rest of the paused range.
        let cpu_delta = CpuDelta::from_nanos(0);
        let weight = i32::try_from(sample_count - 1).unwrap_or(0) * off_cpu_weight_per_sample;
        let profile_timestamp = timestamp_converter.convert_time(end_timestamp);
        samples.add_sample(
            thread_handle,
            profile_timestamp,
            begin_timestamp,
            stack,
            cpu_delta,
            weight,
            None,
        );
    }
}

fn get_pe_mapping_size(path_slice: &[u8]) -> Option<u64> {
    fn inner<T: ImageNtHeaders>(data: &[u8]) -> Option<u64> {
        let file = PeFile::<T>::parse(data).ok()?;
        let size = file.nt_headers().optional_header().size_of_image();
        Some(size as u64)
    }

    let path = Path::new(std::str::from_utf8(path_slice).ok()?);
    let file = std::fs::File::open(path).ok()?;
    let mmap = unsafe { Mmap::map(&file).ok()? };

    match FileKind::parse(&mmap[..]).ok()? {
        FileKind::Pe32 => inner::<ImageNtHeaders32>(&mmap),
        FileKind::Pe64 => inner::<ImageNtHeaders64>(&mmap),
        _ => None,
    }
}

fn get_path_if_jitdump(path: &[u8]) -> Option<&Path> {
    let path = Path::new(std::str::from_utf8(path).ok()?);
    let filename = path.file_name()?.to_str()?;
    if filename.starts_with("jit-") && filename.ends_with(".dump") {
        Some(path)
    } else {
        None
    }
}
