use byteorder::LittleEndian;
use debugid::DebugId;

use framehop::{ExplicitModuleSectionInfo, FrameAddress, Module, Unwinder};
use fxprof_processed_profile::{
    CpuDelta, LibraryHandle, LibraryInfo, Profile, ReferenceTimestamp, SamplingInterval,
    SymbolTable, ThreadHandle,
};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{
    DsoInfo, DsoKey, Endianness, SimpleperfFileRecord, SimpleperfSymbol, SimpleperfTypeSpecificInfo,
};
use linux_perf_event_reader::constants::PERF_CONTEXT_MAX;
use linux_perf_event_reader::{
    CommOrExecRecord, CommonData, ContextSwitchRecord, ForkOrExitRecord, Mmap2FileId, Mmap2Record,
    MmapRecord, RawDataU64, SampleRecord,
};
use memmap2::Mmap;
use object::{CompressedFileRange, CompressionFormat, Object, ObjectSection};
use samply_symbols::{debug_id_for_object, DebugIdExt};
use wholesym::samply_symbols::demangle_any;
use wholesym::{samply_symbols, CodeId, ElfBuildId};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::{ops::Range, path::Path};

use super::avma_range::AvmaRange;
use super::context_switch::{ContextSwitchHandler, OffCpuSampleGroup};
use super::convert_regs::ConvertRegs;
use super::event_interpretation::{EventInterpretation, OffCpuIndicator};
use super::injected_jit_object::{correct_bad_perf_jit_so_file, jit_function_name};
use super::kernel_symbols::{kernel_module_build_id, KernelSymbols};
use super::mmap_range_or_vec::MmapRangeOrVec;
use super::pe_mappings::{PeMappings, SuspectedPeMapping};
use super::per_cpu::Cpus;
use super::processes::Processes;
use super::rss_stat::{RssStat, MM_ANONPAGES, MM_FILEPAGES, MM_SHMEMPAGES, MM_SWAPENTS};
use super::svma_file_range::compute_vma_bias;
use super::vdso::VdsoObject;

use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::process_sample_data::RssStatMember;
use crate::shared::recording_props::ProfileCreationProps;
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::types::{StackFrame, StackMode};
use crate::shared::unresolved_samples::{
    UnresolvedSamples, UnresolvedStackHandle, UnresolvedStacks,
};
use crate::shared::utils::open_file_with_fallback;

pub type BoxedProductNameGenerator = Box<dyn FnOnce(&str) -> String>;

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
    off_cpu_indicator: Option<OffCpuIndicator>,
    event_names: Vec<String>,
    kernel_symbols: Option<KernelSymbols>,
    kernel_image_mapping: Option<KernelImageMapping>,
    simpleperf_symbol_tables_user: HashMap<Vec<u8>, SymbolTableFromSimpleperf>,
    simpleperf_symbol_tables_kernel: HashMap<Vec<u8>, Vec<SimpleperfSymbol>>,
    pe_mappings: PeMappings,
    jit_category_manager: JitCategoryManager,
    cpus: Option<Cpus>,

    /// Whether repeated frames at the base of the stack should be folded
    /// into one frame.
    fold_recursive_prefix: bool,
}

const DEFAULT_OFF_CPU_SAMPLING_INTERVAL_NS: u64 = 1_000_000; // 1ms

impl<U> Converter<U>
where
    U: Unwinder<Module = Module<MmapRangeOrVec>> + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile_creation_props: &ProfileCreationProps,
        delayed_product_name_generator: Option<BoxedProductNameGenerator>,
        build_ids: HashMap<DsoKey, DsoInfo>,
        linux_version: Option<&str>,
        first_sample_time: u64,
        endian: Endianness,
        cache: U::Cache,
        extra_binary_artifact_dir: Option<&Path>,
        interpretation: EventInterpretation,
        simpleperf_symbol_tables: Option<Vec<SimpleperfFileRecord>>,
    ) -> Self {
        let interval = match interpretation.sampling_is_time_based {
            Some(nanos) => SamplingInterval::from_nanos(nanos),
            None => SamplingInterval::from_millis(1),
        };
        let mut profile = Profile::new(
            &profile_creation_props.profile_name,
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
            Err(_err) => {
                // eprintln!("Could not obtain kernel symbols: {err}");
                None
            }
        };

        let mut simpleperf_symbol_tables_user = HashMap::new();
        let mut simpleperf_symbol_tables_kernel = HashMap::new();
        if let Some(simpleperf_symbol_tables) = simpleperf_symbol_tables {
            for f in simpleperf_symbol_tables {
                let path = f.path.clone().into_bytes();
                if f.path == "[kernel.kallsyms]"
                    || matches!(
                        f.type_specific_msg,
                        Some(SimpleperfTypeSpecificInfo::KernelModule(_))
                    )
                {
                    simpleperf_symbol_tables_kernel.insert(path, f.symbol);
                } else {
                    let file_offset_of_min_vaddr_in_elf_file = match f.type_specific_msg {
                        Some(SimpleperfTypeSpecificInfo::ElfFile(elf)) => {
                            Some(elf.file_offset_of_min_vaddr)
                        }
                        _ => None,
                    };
                    let symbols: Vec<_> = f
                        .symbol
                        .iter()
                        .map(|s| fxprof_processed_profile::Symbol {
                            address: s.vaddr as u32,
                            size: Some(s.len),
                            name: demangle_any(&s.name),
                        })
                        .collect();
                    let symbol_table = SymbolTable::new(symbols);
                    let symbol_table = SymbolTableFromSimpleperf {
                        file_offset_of_min_vaddr_in_elf_file,
                        min_vaddr: f.min_vaddr,
                        symbol_table: Arc::new(symbol_table),
                    };
                    simpleperf_symbol_tables_user.insert(path, symbol_table);
                }
            }
        }

        let timestamp_converter = TimestampConverter {
            reference_raw: first_sample_time,
            raw_to_ns_factor: 1,
        };

        let cpus = if profile_creation_props.create_per_cpu_threads {
            let start_timestamp = timestamp_converter.convert_time(first_sample_time);
            Some(Cpus::new(start_timestamp, &mut profile))
        } else {
            None
        };

        Self {
            profile,
            cache,
            processes: Processes::new(
                profile_creation_props.reuse_threads,
                profile_creation_props.unlink_aux_files,
            ),
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
            off_cpu_indicator: interpretation.off_cpu_indicator,
            event_names: interpretation.event_names,
            kernel_symbols,
            kernel_image_mapping: None,
            simpleperf_symbol_tables_user,
            simpleperf_symbol_tables_kernel,
            pe_mappings: PeMappings::new(),
            jit_category_manager: JitCategoryManager::new(),
            fold_recursive_prefix: profile_creation_props.fold_recursive_prefix,
            cpus,
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

        let cpu_delta = if self.off_cpu_indicator.is_some() {
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

        if let (Some(cpu_index), Some(cpus)) = (e.cpu, &mut self.cpus) {
            let cpu = cpus.get_mut(cpu_index as usize, &mut self.profile);

            let thread_handle = cpu.thread_handle;

            // Consume idle cpu time.
            let _idle_cpu_sample = self
                .context_switch_handler
                .handle_on_cpu_sample(timestamp, &mut cpu.context_switch_data);

            let cpu_delta = if self.off_cpu_indicator.is_some() {
                CpuDelta::from_nanos(
                    self.context_switch_handler
                        .consume_cpu_delta(&mut cpu.context_switch_data),
                )
            } else {
                CpuDelta::from_nanos(0)
            };

            process.unresolved_samples.add_sample(
                thread_handle,
                profile_timestamp,
                timestamp,
                stack_index,
                cpu_delta,
                1,
                Some(thread.thread_label_frame.clone()),
            );

            process.unresolved_samples.add_sample(
                cpus.combined_thread_handle(),
                profile_timestamp,
                timestamp,
                stack_index,
                CpuDelta::ZERO,
                1,
                Some(thread.thread_label_frame.clone()),
            );
        }
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

        if self.off_cpu_indicator == Some(OffCpuIndicator::SchedSwitchAndSamples) {
            // Treat this sched_switch sample as a switch-out.
            // Sometimes we have sched_switch samples but no context switch records; for
            // example when using `simpleperf record --trace-offcpu`.
            let timestamp = e
                .timestamp
                .expect("Can't handle context switch without time");
            self.context_switch_handler
                .handle_switch_out(timestamp, &mut thread.context_switch_data);
        }
    }

    pub fn handle_rss_stat_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: &SampleRecord,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        // let tid = e.tid.expect("Can't handle samples without tids");
        let process = self.processes.get_by_pid(pid, &mut self.profile);

        let Some(raw) = e.raw else { return };
        let Ok(rss_stat) = RssStat::parse(raw, self.endian) else {
            return;
        };

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

    pub fn handle_mmap(&mut self, e: MmapRecord, timestamp: u64) {
        let mut path = e.path.as_slice();
        if self.check_jitdump_or_marker_file(&path, e.pid, e.tid) {
            // Not a DSO.
            return;
        }

        if e.page_offset == 0 {
            self.pe_mappings.check_mmap(&path, e.address);
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
        if self.check_jitdump_or_marker_file(&path, e.pid, e.tid) {
            // Not a DSO.
            return;
        }

        if e.page_offset == 0 {
            self.pe_mappings.check_mmap(&path, e.address);
        }

        const PROT_EXEC: u32 = 0b100;
        if e.protection & PROT_EXEC == 0 && !self.simpleperf_symbol_tables_user.contains_key(&*path)
        {
            // Ignore non-executable mappings.
            // Don't ignore mappings that simpleperf found symbols for, even if they're
            // non-executable. TODO: Find out why .vdex and .jar mappings with symbols aren't
            // marked as executable in the simpleperf perf.data files. Is simpleperf simply
            // forgetting to set the right flag? Or are these mappings synthetic? Are we
            // actually running code from inside these mappings? Surely then the memory must
            // be executable?
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

    fn check_jitdump_or_marker_file(&mut self, path: &[u8], pid: i32, tid: i32) -> bool {
        let Ok(path) = std::str::from_utf8(path) else {
            return false;
        };

        let filename = match path.rfind('/') {
            Some(pos) => &path[pos + 1..],
            None => path,
        };

        if filename.starts_with("jit-") && filename.ends_with(".dump") {
            let jitdump_path = Path::new(path);
            let process = self.processes.get_by_pid(pid, &mut self.profile);
            let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);
            let profile_thread = thread.profile_thread;
            process.jitdump_manager.add_jitdump_path(
                profile_thread,
                jitdump_path,
                self.extra_binary_artifact_dir.clone(),
            );
            return true;
        }

        if filename.starts_with("marker-") && filename.ends_with(".txt") {
            let marker_file_path = Path::new(path);
            let process = self.processes.get_by_pid(pid, &mut self.profile);
            let thread = process.threads.get_thread_by_tid(tid, &mut self.profile);
            let profile_thread = thread.profile_thread;
            process.add_marker_file_path(
                profile_thread,
                marker_file_path,
                self.extra_binary_artifact_dir.clone(),
            );
            return true;
        }

        false
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
                if let (Some(cpus), Some(cpu_index)) = (&mut self.cpus, common.cpu) {
                    let combined_thread = cpus.combined_thread_handle();
                    let cpu = cpus.get_mut(cpu_index as usize, &mut self.profile);
                    let _idle_cpu_sample = self
                        .context_switch_handler
                        .handle_switch_in(timestamp, &mut cpu.context_switch_data);
                    cpu.notify_switch_in(
                        tid,
                        thread.thread_label(),
                        timestamp,
                        &self.timestamp_converter,
                        &[cpu.thread_handle, combined_thread],
                        &mut self.profile,
                    );
                }
            }
            ContextSwitchRecord::Out { .. } => {
                self.context_switch_handler
                    .handle_switch_out(timestamp, &mut thread.context_switch_data);
                if let (Some(cpus), Some(cpu_index)) = (&mut self.cpus, Some(common.cpu.unwrap())) {
                    let combined_thread = cpus.combined_thread_handle();
                    let cpu = cpus.get_mut(cpu_index as usize, &mut self.profile);
                    self.context_switch_handler
                        .handle_switch_out(timestamp, &mut cpu.context_switch_data);
                    cpu.notify_switch_out(
                        tid,
                        timestamp,
                        &self.timestamp_converter,
                        &[cpu.thread_handle, combined_thread],
                        &mut self.profile,
                    );
                }
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
            // New process. The forking thread becomes the main thread of the new process.
            // eprintln!("Process fork: old_pid={}, old_tid={}, new_pid={}", e.ppid, e.ptid, e.pid);
            if !is_main {
                eprintln!("Unexpected data in FORK record: If we fork into a different process, the forked child thread should be the main thread of the new process");
            }
            let parent_process_name = parent_process.name.clone();
            let fork_data = parent_process.clone_fork_data();
            let child_process = self.processes.recycle_or_get_new(
                e.pid,
                parent_process_name,
                start_time,
                &mut self.profile,
            );
            child_process.adopt_fork_data_from_parent(fork_data);
        } else {
            // New thread within the same process.
            // eprintln!("New thread: pid={}, old_tid={}, new_tid={}", e.pid, e.ptid, e.tid);
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
            // eprintln!("Process execve: pid={}, tid={}, new name: {}", e.pid, e.tid, name);

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
            // eprintln!("Process rename: pid={}, new name: {}", e.pid, name);
            self.processes
                .rename_process(e.pid, timestamp, name.to_string(), &mut self.profile);
        } else {
            // eprintln!("Thread rename: pid={}, tid={}, new name: {}", e.pid, e.tid, name);
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
        path_slice: &[u8],
    ) {
        let original_path = path_slice;
        let path = std::str::from_utf8(path_slice).unwrap().to_string();
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
        let symbol_table = if let Some(symbols) =
            self.simpleperf_symbol_tables_kernel.remove(original_path)
        {
            let symbols: Vec<_> = symbols
                .into_iter()
                .map(|s| fxprof_processed_profile::Symbol {
                    address: (s.vaddr - base_address) as u32,
                    size: Some(s.len),
                    name: s.name,
                })
                .collect();
            Some(Arc::new(SymbolTable::new(symbols)))
        } else {
            match (&dso_key, &build_id, self.kernel_symbols.as_ref()) {
                (DsoKey::Kernel, Some(build_id), Some(kernel_symbols))
                    if build_id == &kernel_symbols.build_id && kernel_symbols.base_avma != 0 =>
                {
                    // Run `echo '0' | sudo tee /proc/sys/kernel/kptr_restrict` to get here without root.
                    Some(kernel_symbols.symbol_table.clone())
                }
                _ => None,
            }
        };

        let lib_handle = self.profile.add_lib(LibraryInfo {
            debug_id: debug_id.unwrap_or_default(),
            path,
            debug_path,
            code_id: build_id
                .map(|build_id| CodeId::ElfBuildId(ElfBuildId::from_bytes(&build_id)).to_string()),
            name: dso_key.name().to_string(),
            debug_name: dso_key.name().to_string(),
            arch: None,
            symbol_table,
        });
        let end_address = base_address + len;
        self.profile
            .add_kernel_lib_mapping(lib_handle, base_address, end_address, 0);

        if path_slice == b"[kernel.kallsyms]" {
            // Store information about this mapping so that we can later adjust the mapping,
            // if we find that other kernel modules overlap with it.
            self.kernel_image_mapping = Some(KernelImageMapping {
                lib_handle,
                base_address,
                end_address,
            });
        } else if let Some(kernel_image_mapping) = &self.kernel_image_mapping {
            // We added a kernel module which is not the main kernel image.
            // See if the module overlaps with it. This can happen when the main kernel
            // image is advertised with a bad address range. For example, in profiles from
            // simpleperf, [kernel.kallsyms] might have the following range:
            // ffffffdc99610000 - ffffffffffffffff
            // And then there are kernel modules at ranges that overlap, like this:
            // ffffffdc9d7d6000 - ffffffdc9d7db000
            // Whenever we encounter such overlap, we adjust the end_address of the
            // main kernel image downwards so that there is no overlap.
            if base_address > kernel_image_mapping.base_address
                && base_address < kernel_image_mapping.end_address
            {
                // This mapping overlaps with the kernel image lib mapping.
                // This means that the call to `add_kernel_lib_mapping` above caused the main kernel
                // image mapping to be evicted.
                //
                // Adjust the mapping and put it back in.
                let mut kernel_image_mapping = self.kernel_image_mapping.take().unwrap();
                kernel_image_mapping.end_address = base_address;
                self.profile.add_kernel_lib_mapping(
                    kernel_image_mapping.lib_handle,
                    kernel_image_mapping.base_address,
                    kernel_image_mapping.end_address,
                    0,
                );
                self.kernel_image_mapping = Some(kernel_image_mapping);
            }
        }
    }

    /// Tell the unwinder and the profile about this module.
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
        let avma_range = AvmaRange::with_start_size(mapping_start_avma, mapping_size);
        let expected_code_id =
            build_id.map(|build_id| CodeId::ElfBuildId(ElfBuildId::from_bytes(build_id)));

        let original_path = path_slice;
        let Some(path) = path_from_unix_bytes(path_slice) else {
            return;
        };

        let mut mapping_info = MappingInfo::new_elf(path, avma_range);
        if path_slice.is_empty() {
            if let Some(pe_mapping) = self.pe_mappings.find_mapping(&avma_range) {
                mapping_info = MappingInfo::new_pe(pe_mapping);
            }
        }

        let mut file = None;
        let mut path = mapping_info.path.to_string_lossy().to_string();

        if let Ok((f, p)) = open_file_with_fallback(
            &mapping_info.path,
            self.extra_binary_artifact_dir.as_deref(),
        ) {
            // Fix up bad files from `perf inject --jit`.
            if let Some((fixed_file, fixed_path)) = correct_bad_perf_jit_so_file(&f, &path) {
                file = Some(fixed_file);
                path = fixed_path;
            } else {
                file = Some(f);
                path = p.to_string_lossy().to_string();
            }
        }

        let name = match path.rfind('/') {
            Some(pos) => path[pos + 1..].to_owned(),
            None => path.clone(),
        };

        let process = self.processes.get_by_pid(process_pid, &mut self.profile);

        // Case 1: There are symbols in the file, if we are importing a perf.data file
        // that was recorded with simpleperf.
        if let Some(symbol_table) = self.simpleperf_symbol_tables_user.get(original_path) {
            let relative_address_at_start = if let Some(file_offset_of_min_vaddr) =
                &symbol_table.file_offset_of_min_vaddr_in_elf_file
            {
                // Example:
                //  - start_avma: 0x721e13c000
                //  - mapping_start_file_offset: 0x4535000
                //  - file_offset_of_min_vaddr: 0x45357e0
                //  - min_vaddr: 0x45367e0,
                let min_vaddr_offset_from_mapping_start =
                    file_offset_of_min_vaddr - mapping_start_file_offset;
                let vaddr_at_start = symbol_table.min_vaddr - min_vaddr_offset_from_mapping_start;

                // Assume vaddr = SVMA == relative address
                vaddr_at_start as u32
            } else {
                // If it's not an ELF file, then this is probably a DEX file.
                // In a DEX file, SVMA == file offset == relative address
                mapping_start_file_offset as u32
            };

            // If we have a build ID, convert it to a debug_id and a code_id.
            let debug_id = build_id
                .map(|id| DebugId::from_identifier(id, true)) // TODO: endian
                .unwrap_or_default();
            let code_id = build_id
                .map(|build_id| CodeId::ElfBuildId(ElfBuildId::from_bytes(build_id)).to_string());

            let lib_handle = self.profile.add_lib(LibraryInfo {
                debug_id,
                code_id,
                path: path.clone(),
                debug_path: path,
                debug_name: name.clone(),
                name,
                arch: None,
                symbol_table: Some(symbol_table.symbol_table.clone()),
            });
            process.add_regular_lib_mapping(
                timestamp,
                avma_range.start(),
                avma_range.end(),
                relative_address_at_start,
                lib_handle,
            );
            return;
        }

        // Case 2: We have access to the file that was loaded into the process.
        if let Some(file) = file {
            let mmap = match unsafe { memmap2::MmapOptions::new().map(&file) } {
                Ok(mmap) => Arc::new(mmap),
                Err(err) => {
                    eprintln!("Could not mmap file {path}: {err:?}");
                    return;
                }
            };

            let file = match object::File::parse(&mmap[..]) {
                Ok(file) => file,
                Err(_) => {
                    eprintln!("File {path} has unrecognized format");
                    return;
                }
            };

            let file_code_id = mapping_info.code_id.clone().or_else(|| {
                Some(CodeId::ElfBuildId(ElfBuildId::from_bytes(
                    file.build_id().ok()??,
                )))
            });
            if expected_code_id.as_ref().is_some_and(|expected_code_id| {
                !Self::code_id_matches(file_code_id.as_ref(), expected_code_id, &path)
            }) {
                return;
            }

            let module_section_info =
                Self::module_section_info_with_object(Some(mmap.clone()), &file);
            let Some(library_info) =
                Self::library_info_with_object(&name, &path, &file, file_code_id)
            else {
                return;
            };

            let Some(base_avma) = mapping_info.compute_base_avma(&file, mapping_start_file_offset)
            else {
                return;
            };
            let module = Module::new(
                path.to_string(),
                avma_range.start()..avma_range.end(),
                base_avma,
                module_section_info,
            );

            let relative_address_at_start = (mapping_start_avma - module.base_avma()) as u32;
            process.unwinder.add_module(module);
            let lib_handle = self.profile.add_lib(library_info);

            if name.starts_with("jitted-") && name.ends_with(".so") {
                let symbol_name = jit_function_name(&file);
                process.add_lib_mapping_for_injected_jit_lib(
                    timestamp,
                    self.timestamp_converter.convert_time(timestamp),
                    symbol_name,
                    avma_range.start(),
                    avma_range.end(),
                    relative_address_at_start,
                    lib_handle,
                    &mut self.jit_category_manager,
                    &mut self.profile,
                );
            } else {
                process.add_regular_lib_mapping(
                    timestamp,
                    avma_range.start(),
                    avma_range.end(),
                    relative_address_at_start,
                    lib_handle,
                );
            }
            return;
        }

        // Case 3: This is the VDSO mapping.
        if name == "[vdso]" {
            if let Some(vdso) = VdsoObject::shared_instance_for_this_process() {
                if expected_code_id.as_ref().is_some_and(|expected_code_id| {
                    !Self::code_id_matches(Some(vdso.code_id()), expected_code_id, &path)
                }) {
                    return;
                }

                let module_section_info =
                    Self::module_section_info_with_object(None, vdso.object());
                let code_id = vdso.code_id().clone();
                let Some(library_info) =
                    Self::library_info_with_object(&name, &path, vdso.object(), Some(code_id))
                else {
                    return;
                };

                let Some(base_avma) =
                    mapping_info.compute_base_avma(vdso.object(), mapping_start_file_offset)
                else {
                    return;
                };
                let module = Module::new(
                    path.clone(),
                    avma_range.start()..avma_range.end(),
                    base_avma,
                    module_section_info,
                );

                let relative_address_at_start = (mapping_start_avma - module.base_avma()) as u32;
                process.unwinder.add_module(module);
                let lib_handle = self.profile.add_lib(library_info);

                process.add_regular_lib_mapping(
                    timestamp,
                    avma_range.start(),
                    avma_range.end(),
                    relative_address_at_start,
                    lib_handle,
                );
                return;
            }
        }

        // Case 4: We don't have access to the file.

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
        let code_id = build_id
            .map(|build_id| CodeId::ElfBuildId(ElfBuildId::from_bytes(build_id)).to_string());

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
            avma_range.start(),
            avma_range.end(),
            relative_address_at_start,
            lib_handle,
        );
    }

    fn code_id_matches(
        file_code_id: Option<&CodeId>,
        expected_code_id: &CodeId,
        path: &str,
    ) -> bool {
        match file_code_id {
            Some(file_code_id) if expected_code_id == file_code_id => {
                // Build IDs match. Good.
                true
            }
            Some(file_code_id) => {
                eprintln!(
                        "File {path} has non-matching build ID {file_code_id} (expected {expected_code_id})"
                    );
                false
            }
            None => {
                eprintln!(
                    "File {path} does not contain a build ID, but we expected it to have one"
                );
                false
            }
        }
    }

    fn library_info_with_object<'data, R: object::ReadRef<'data>>(
        name: &str,
        path: &str,
        file: &object::File<'data, R>,
        code_id: Option<CodeId>,
    ) -> Option<LibraryInfo> {
        let debug_id = debug_id_for_object(file)?;
        Some(LibraryInfo {
            debug_id,
            code_id: code_id.map(|ci| ci.to_string()),
            path: path.to_owned(),
            debug_path: path.to_owned(),
            debug_name: name.to_owned(),
            name: name.to_owned(),
            arch: None,
            symbol_table: None,
        })
    }

    fn module_section_info_with_object<'data, R: object::ReadRef<'data>>(
        mmap_arc: Option<Arc<Mmap>>,
        file: &object::File<'data, R>,
    ) -> ExplicitModuleSectionInfo<MmapRangeOrVec> {
        let mmap = mmap_arc.as_ref();

        fn section_data<'a>(
            section: &impl ObjectSection<'a>,
            mmap: Option<&Arc<Mmap>>,
        ) -> Option<MmapRangeOrVec> {
            let CompressedFileRange {
                format,
                offset,
                compressed_size: _,
                uncompressed_size,
            } = section.compressed_file_range().ok()?;
            match (format, mmap) {
                (CompressionFormat::None, Some(mmap)) => {
                    MmapRangeOrVec::new_mmap_range(mmap.clone(), offset, uncompressed_size)
                }
                _ => Some(MmapRangeOrVec::Vec(Arc::new(
                    section.uncompressed_data().ok()?.to_vec(),
                ))),
            }
        }

        let base_svma = samply_symbols::relative_address_base(file);
        let text = file.section_by_name(".text");
        let eh_frame = file.section_by_name(".eh_frame");
        let got = file.section_by_name(".got");
        let eh_frame_hdr = file.section_by_name(".eh_frame_hdr");

        let eh_frame_data = eh_frame.as_ref().and_then(|s| section_data(s, mmap));
        let eh_frame_hdr_data = eh_frame_hdr.as_ref().and_then(|s| section_data(s, mmap));
        let text_data = text.as_ref().and_then(|s| section_data(s, mmap));
        fn svma_range<'a>(section: &impl ObjectSection<'a>) -> Range<u64> {
            section.address()..section.address() + section.size()
        }

        ExplicitModuleSectionInfo {
            base_svma,
            text_svma: text.as_ref().map(svma_range),
            text: text_data,
            stubs_svma: None,
            stub_helper_svma: None,
            got_svma: got.as_ref().map(svma_range),
            unwind_info: None,
            eh_frame_svma: eh_frame.as_ref().map(svma_range),
            eh_frame: eh_frame_data,
            eh_frame_hdr_svma: eh_frame_hdr.as_ref().map(svma_range),
            eh_frame_hdr: eh_frame_hdr_data,
            debug_frame: None,
            text_segment_svma: None,
            text_segment: None,
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

struct MappingInfo {
    path: PathBuf,
    code_id: Option<CodeId>,
    avma_range: AvmaRange,
    mapping_type: MappingType,
}

impl MappingInfo {
    pub fn new_elf(path: &Path, avma_range: AvmaRange) -> Self {
        Self {
            path: path.to_owned(),
            code_id: None,
            avma_range,
            mapping_type: MappingType::Elf,
        }
    }

    pub fn new_pe(pe_mapping: &SuspectedPeMapping) -> Self {
        Self {
            path: pe_mapping.path.clone(),
            code_id: Some(pe_mapping.code_id.clone()),
            avma_range: pe_mapping.avma_range,
            mapping_type: MappingType::Pe,
        }
    }

    pub fn compute_base_avma<'data, O: Object<'data>>(
        &self,
        file: &O,
        mapping_start_file_offset: u64,
    ) -> Option<u64> {
        let base_avma = match self.mapping_type {
            MappingType::Pe => {
                // For the PE correlation hack, we can't use the mapping offsets as they correspond to
                // an anonymous mapping. Instead, the base address is pre-determined from the PE header
                // mapping.
                self.avma_range.start()
            }
            MappingType::Elf => {
                let bias = compute_vma_bias(
                    file,
                    mapping_start_file_offset,
                    self.avma_range.start(),
                    self.avma_range.size(),
                )?;
                let base_svma = samply_symbols::relative_address_base(file);
                base_svma.wrapping_add(bias)
            }
        };
        Some(base_avma)
    }
}

enum MappingType {
    Elf,
    Pe,
}

struct SymbolTableFromSimpleperf {
    min_vaddr: u64,
    file_offset_of_min_vaddr_in_elf_file: Option<u64>,
    symbol_table: Arc<SymbolTable>,
}

struct KernelImageMapping {
    lib_handle: LibraryHandle,
    base_address: u64,
    end_address: u64,
}

#[cfg(unix)]
fn path_from_unix_bytes(path_slice: &[u8]) -> Option<&Path> {
    use std::os::unix::ffi::OsStrExt;
    Some(Path::new(std::ffi::OsStr::from_bytes(path_slice)))
}

// Returns None on Windows if path_slice is not valid utf-8.
#[cfg(not(unix))]
fn path_from_unix_bytes(path_slice: &[u8]) -> Option<&Path> {
    Some(Path::new(std::str::from_utf8(path_slice).ok()?))
}
