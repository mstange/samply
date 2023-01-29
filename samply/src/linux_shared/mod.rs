mod context_switch;
mod kernel_symbols;

use byteorder::LittleEndian;
use context_switch::{ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData};
use debugid::{CodeId, DebugId};
use framehop::aarch64::UnwindRegsAarch64;
use framehop::x86_64::UnwindRegsX86_64;
use framehop::{FrameAddress, Module, ModuleSvmaInfo, ModuleUnwindData, TextByteData, Unwinder};
use fxprof_processed_profile::{
    CategoryColor, CategoryPairHandle, CpuDelta, Frame, LibraryInfo, ProcessHandle, Profile,
    ReferenceTimestamp, SamplingInterval, ThreadHandle, Timestamp,
};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{AttributeDescription, DsoInfo, DsoKey};
use linux_perf_event_reader::constants::{
    PERF_CONTEXT_GUEST, PERF_CONTEXT_GUEST_KERNEL, PERF_CONTEXT_GUEST_USER, PERF_CONTEXT_KERNEL,
    PERF_CONTEXT_MAX, PERF_CONTEXT_USER, PERF_REG_ARM64_LR, PERF_REG_ARM64_PC, PERF_REG_ARM64_SP,
    PERF_REG_ARM64_X29, PERF_REG_X86_BP, PERF_REG_X86_IP, PERF_REG_X86_SP,
};
use linux_perf_event_reader::{
    AttrFlags, CommOrExecRecord, CommonData, ContextSwitchRecord, CpuMode, ForkOrExitRecord,
    Mmap2FileId, Mmap2Record, MmapRecord, PerfEventType, RawDataU64, Regs, SampleRecord,
    SamplingPolicy, SoftwareCounterType,
};
use object::{Object, ObjectSection, ObjectSegment, SectionKind};
use samply_symbols::{debug_id_for_object, object, DebugIdExt};
use wholesym::samply_symbols;

use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::path::PathBuf;
use std::time::SystemTime;
use std::{ops::Range, path::Path};

use self::kernel_symbols::KernelSymbols;

pub trait ConvertRegs {
    type UnwindRegs;
    fn convert_regs(regs: &Regs) -> (u64, u64, Self::UnwindRegs);
    fn regs_mask() -> u64;
}

pub struct ConvertRegsX86_64;
impl ConvertRegs for ConvertRegsX86_64 {
    type UnwindRegs = UnwindRegsX86_64;
    fn convert_regs(regs: &Regs) -> (u64, u64, UnwindRegsX86_64) {
        let ip = regs.get(PERF_REG_X86_IP).unwrap();
        let sp = regs.get(PERF_REG_X86_SP).unwrap();
        let bp = regs.get(PERF_REG_X86_BP).unwrap();
        let regs = UnwindRegsX86_64::new(ip, sp, bp);
        (ip, sp, regs)
    }

    fn regs_mask() -> u64 {
        1 << PERF_REG_X86_IP | 1 << PERF_REG_X86_SP | 1 << PERF_REG_X86_BP
    }
}

pub struct ConvertRegsAarch64;
impl ConvertRegs for ConvertRegsAarch64 {
    type UnwindRegs = UnwindRegsAarch64;
    fn convert_regs(regs: &Regs) -> (u64, u64, UnwindRegsAarch64) {
        let ip = regs.get(PERF_REG_ARM64_PC).unwrap();
        let lr = regs.get(PERF_REG_ARM64_LR).unwrap();
        let sp = regs.get(PERF_REG_ARM64_SP).unwrap();
        let fp = regs.get(PERF_REG_ARM64_X29).unwrap();
        let regs = UnwindRegsAarch64::new(lr, sp, fp);
        (ip, sp, regs)
    }

    fn regs_mask() -> u64 {
        1 << PERF_REG_ARM64_PC
            | 1 << PERF_REG_ARM64_LR
            | 1 << PERF_REG_ARM64_SP
            | 1 << PERF_REG_ARM64_X29
    }
}

#[derive(Debug, Clone)]
pub struct EventInterpretation {
    pub main_event_attr_index: usize,
    #[allow(unused)]
    pub main_event_name: String,
    pub sampling_is_time_based: Option<u64>,
    pub have_context_switches: bool,
    pub sched_switch_attr_index: Option<usize>,
}

impl EventInterpretation {
    pub fn divine_from_attrs(attrs: &[AttributeDescription]) -> Self {
        let main_event_attr_index = 0;
        let main_event_name = attrs[0]
            .name
            .as_deref()
            .unwrap_or("<unnamed event>")
            .to_string();
        let sampling_is_time_based = match (attrs[0].attr.type_, attrs[0].attr.sampling_policy) {
            (_, SamplingPolicy::NoSampling) => {
                panic!("Can only convert profiles with sampled events")
            }
            (_, SamplingPolicy::Frequency(freq)) => {
                let nanos = 1_000_000_000 / freq;
                Some(nanos)
            }
            (
                PerfEventType::Software(
                    SoftwareCounterType::CpuClock | SoftwareCounterType::TaskClock,
                ),
                SamplingPolicy::Period(period),
            ) => {
                // Assume that we're using a nanosecond clock. TODO: Check how we can know this for sure
                let nanos = u64::from(period);
                Some(nanos)
            }
            (_, SamplingPolicy::Period(_)) => None,
        };
        let have_context_switches = attrs[0].attr.flags.contains(AttrFlags::CONTEXT_SWITCH);
        let sched_switch_attr_index = attrs
            .iter()
            .position(|attr_desc| attr_desc.name.as_deref() == Some("sched:sched_switch"));

        Self {
            main_event_attr_index,
            main_event_name,
            sampling_is_time_based,
            have_context_switches,
            sched_switch_attr_index,
        }
    }
}

pub type BoxedProductNameGenerator = Box<dyn FnOnce(&str) -> String>;

pub struct Converter<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    cache: U::Cache,
    profile: Profile,
    processes: Processes<U>,
    threads: Threads,
    stack_converter: StackConverter,
    timestamp_converter: TimestampConverter,
    current_sample_time: u64,
    build_ids: HashMap<DsoKey, DsoInfo>,
    little_endian: bool,
    have_product_name: bool,
    delayed_product_name_generator: Option<BoxedProductNameGenerator>,
    linux_version: Option<String>,
    extra_binary_artifact_dir: Option<PathBuf>,
    context_switch_handler: ContextSwitchHandler,
    off_cpu_weight_per_sample: i32,
    have_context_switches: bool,
    kernel_symbols: Option<KernelSymbols>,
}

const DEFAULT_OFF_CPU_SAMPLING_INTERVAL_NS: u64 = 1_000_000; // 1ms

impl<U> Converter<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        product: &str,
        delayed_product_name_generator: Option<BoxedProductNameGenerator>,
        build_ids: HashMap<DsoKey, DsoInfo>,
        linux_version: Option<&str>,
        first_sample_time: u64,
        little_endian: bool,
        cache: U::Cache,
        extra_binary_artifact_dir: Option<&Path>,
        interpretation: EventInterpretation,
    ) -> Self {
        let interval = match interpretation.sampling_is_time_based {
            Some(nanos) => SamplingInterval::from_nanos(nanos),
            None => SamplingInterval::from_millis(1),
        };
        let mut profile = Profile::new(
            product,
            ReferenceTimestamp::from_system_time(SystemTime::now()),
            interval,
        );
        let user_category = profile.add_category("User", CategoryColor::Yellow).into();
        let kernel_category = profile.add_category("Kernel", CategoryColor::Orange).into();
        let (off_cpu_sampling_interval_ns, off_cpu_weight_per_sample) =
            match &interpretation.sampling_is_time_based {
                Some(interval_ns) => (*interval_ns, 1),
                None => (DEFAULT_OFF_CPU_SAMPLING_INTERVAL_NS, 0),
            };
        Self {
            profile,
            cache,
            processes: Processes(HashMap::new()),
            threads: Threads(HashMap::new()),
            stack_converter: StackConverter {
                user_category,
                kernel_category,
            },
            timestamp_converter: TimestampConverter::with_reference_timestamp(first_sample_time),
            current_sample_time: first_sample_time,
            build_ids,
            little_endian,
            have_product_name: delayed_product_name_generator.is_none(),
            delayed_product_name_generator,
            linux_version: linux_version.map(ToOwned::to_owned),
            extra_binary_artifact_dir: extra_binary_artifact_dir.map(ToOwned::to_owned),
            off_cpu_weight_per_sample,
            context_switch_handler: ContextSwitchHandler::new(off_cpu_sampling_interval_ns),
            have_context_switches: interpretation.have_context_switches,
            kernel_symbols: KernelSymbols::new_for_running_kernel(),
        }
    }

    pub fn finish(self) -> Profile {
        self.profile
    }

    pub fn handle_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(&mut self, e: SampleRecord) {
        let pid = e.pid.expect("Can't handle samples without pids");
        let tid = e.tid.expect("Can't handle samples without tids");
        let timestamp = e
            .timestamp
            .expect("Can't handle samples without timestamps");
        self.current_sample_time = timestamp;

        let profile_timestamp = self.timestamp_converter.convert_time(timestamp);

        let is_main = pid == tid;
        let process = self.processes.get_by_pid(pid, &mut self.profile);

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(&e, &process.unwinder, &mut self.cache, &mut stack);

        let thread =
            self.threads
                .get_by_tid(tid, process.profile_process, is_main, &mut self.profile);

        if thread.last_sample_timestamp == Some(timestamp) {
            // Duplicate sample. Ignore.
            return;
        }

        let thread_handle = thread.profile_thread;

        let off_cpu_sample = self
            .context_switch_handler
            .handle_sample(timestamp, &mut thread.context_switch_data);
        if let Some(off_cpu_sample) = off_cpu_sample {
            let cpu_delta_ns = self
                .context_switch_handler
                .consume_cpu_delta(&mut thread.context_switch_data);
            process_off_cpu_sample_group(
                off_cpu_sample,
                thread_handle,
                cpu_delta_ns,
                &self.timestamp_converter,
                self.off_cpu_weight_per_sample,
                &thread.off_cpu_stack,
                &mut self.profile,
            );
        }
        // Clear any saved off-CPU stack.
        thread.off_cpu_stack = Vec::new();

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

        let frames = self.stack_converter.convert_stack(stack);
        self.profile
            .add_sample(thread_handle, profile_timestamp, frames, cpu_delta, 1);
        thread.last_sample_timestamp = Some(timestamp);
    }

    pub fn handle_sched_switch<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: SampleRecord,
    ) {
        let pid = e.pid.expect("Can't handle samples without pids");
        let tid = e.tid.expect("Can't handle samples without tids");
        let is_main = pid == tid;
        let process = self.processes.get_by_pid(pid, &mut self.profile);

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(&e, &process.unwinder, &mut self.cache, &mut stack);

        let stack = self
            .stack_converter
            .convert_stack_no_kernel(&stack)
            .collect();

        let thread =
            self.threads
                .get_by_tid(tid, process.profile_process, is_main, &mut self.profile);
        thread.off_cpu_stack = stack;
    }

    /// Get the stack contained in this sample, and put it into `stack`.
    ///
    /// We can have both the kernel stack and the user stack, or just one of
    /// them, or neither.
    ///
    /// If this sample has a kernel stack, it's always in `e.callchain`.
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
        }
    }

    pub fn handle_mmap(&mut self, e: MmapRecord) {
        if !e.is_executable {
            return;
        }

        let mut path = e.path.as_slice();
        let dso_key = match DsoKey::detect(&path, e.cpu_mode) {
            Some(dso_key) => dso_key,
            None => return,
        };
        let mut build_id = None;
        if let Some(dso_info) = self.build_ids.get(&dso_key) {
            build_id = Some(&dso_info.build_id[..]);
            // Overwrite the path from the mmap record with the path from the build ID info.
            // These paths are usually the same, but in some cases the path from the build
            // ID info can be "better". For example, the synthesized mmap event for the
            // kernel vmlinux image usually has "[kernel.kallsyms]_text" whereas the build
            // ID info might have the full path to a kernel debug file, e.g.
            // "/usr/lib/debug/boot/vmlinux-4.16.0-1-amd64".
            path = Cow::Borrowed(&dso_info.path);
        }

        if e.pid == -1 {
            let lib = self.kernel_lib(e.address, e.length, dso_key, build_id, &path);
            self.profile.add_kernel_lib(lib);
        } else {
            let process = self.processes.get_by_pid(e.pid, &mut self.profile);
            if let Some(lib) = add_module_to_unwinder(
                &mut process.unwinder,
                &path,
                e.page_offset,
                e.address,
                e.length,
                build_id,
                self.extra_binary_artifact_dir.as_deref(),
            ) {
                self.profile.add_lib(process.profile_process, lib);
            }
        }
    }

    pub fn handle_mmap2(&mut self, e: Mmap2Record) {
        const PROT_EXEC: u32 = 0b100;
        if e.protection & PROT_EXEC == 0 {
            // Ignore non-executable mappings.
            return;
        }

        let path = e.path.as_slice();
        let build_id = match &e.file_id {
            Mmap2FileId::BuildId(build_id) => Some(&build_id[..]),
            Mmap2FileId::InodeAndVersion(_) => {
                let dso_key = match DsoKey::detect(&path, e.cpu_mode) {
                    Some(dso_key) => dso_key,
                    None => return,
                };
                self.build_ids.get(&dso_key).map(|db| &db.build_id[..])
            }
        };

        let process = self.processes.get_by_pid(e.pid, &mut self.profile);
        if let Some(lib) = add_module_to_unwinder(
            &mut process.unwinder,
            &path,
            e.page_offset,
            e.address,
            e.length,
            build_id,
            self.extra_binary_artifact_dir.as_deref(),
        ) {
            self.profile.add_lib(process.profile_process, lib);
        }
    }

    pub fn handle_context_switch(&mut self, e: ContextSwitchRecord, common: CommonData) {
        let pid = common.pid.expect("Can't handle samples without pids");
        let tid = common.tid.expect("Can't handle samples without tids");
        let timestamp = common
            .timestamp
            .expect("Can't handle context switch without time");
        let is_main = pid == tid;
        let process = self.processes.get_by_pid(pid, &mut self.profile);
        let process_handle = process.profile_process;
        let thread = self
            .threads
            .get_by_tid(tid, process_handle, is_main, &mut self.profile);

        match e {
            ContextSwitchRecord::In { .. } => {
                let off_cpu_sample = self
                    .context_switch_handler
                    .handle_switch_in(timestamp, &mut thread.context_switch_data);
                if let Some(off_cpu_sample) = off_cpu_sample {
                    let cpu_delta_ns = self
                        .context_switch_handler
                        .consume_cpu_delta(&mut thread.context_switch_data);
                    process_off_cpu_sample_group(
                        off_cpu_sample,
                        thread.profile_thread,
                        cpu_delta_ns,
                        &self.timestamp_converter,
                        self.off_cpu_weight_per_sample,
                        &thread.off_cpu_stack,
                        &mut self.profile,
                    );
                }
                // Clear the saved off-CPU stack.
                thread.off_cpu_stack = Vec::new();
            }
            ContextSwitchRecord::Out { .. } => {
                self.context_switch_handler
                    .handle_switch_out(timestamp, &mut thread.context_switch_data);
            }
        }
    }

    pub fn handle_thread_start(&mut self, e: ForkOrExitRecord) {
        let is_main = e.pid == e.tid;
        let start_time = self.timestamp_converter.convert_time(e.timestamp);
        let process = self.processes.get_by_pid(e.pid, &mut self.profile);
        let process_handle = process.profile_process;
        if is_main {
            self.profile
                .set_process_start_time(process_handle, start_time);
        }
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);
        let thread_handle = thread.profile_thread;
        self.profile
            .set_thread_start_time(thread_handle, start_time);
    }

    pub fn handle_thread_end(&mut self, e: ForkOrExitRecord) {
        let is_main = e.pid == e.tid;
        let end_time = self.timestamp_converter.convert_time(e.timestamp);
        let process = self.processes.get_by_pid(e.pid, &mut self.profile);
        let process_handle = process.profile_process;
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);
        let thread_handle = thread.profile_thread;
        self.profile.set_thread_end_time(thread_handle, end_time);
        self.threads.0.remove(&e.tid);
        if is_main {
            self.profile.set_process_end_time(process_handle, end_time);
            self.processes.0.remove(&e.pid);
        }
    }

    pub fn handle_thread_name_update(&mut self, e: CommOrExecRecord, timestamp: Option<u64>) {
        let is_main = e.pid == e.tid;
        if e.is_execve {
            // Mark the old thread / process as ended.
            // If the COMM record doesn't have a timestamp, take the last seen
            // timestamp from the previous sample.
            let timestamp = match timestamp {
                Some(0) | None => self.current_sample_time,
                Some(ts) => ts,
            };
            let time = self.timestamp_converter.convert_time(timestamp);
            if let Some(t) = self.threads.0.get(&e.tid) {
                self.profile.set_thread_end_time(t.profile_thread, time);
                self.threads.0.remove(&e.tid);
            }
            if is_main {
                if let Some(p) = self.processes.0.get(&e.pid) {
                    self.profile.set_process_end_time(p.profile_process, time);
                    self.processes.0.remove(&e.pid);
                }
            }
        }

        let process_handle = self
            .processes
            .get_by_pid(e.pid, &mut self.profile)
            .profile_process;

        let name = e.name.as_slice();
        let name = String::from_utf8_lossy(&name);
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);
        let thread_handle = thread.profile_thread;

        self.profile.set_thread_name(thread_handle, &name);
        if is_main {
            self.profile.set_process_name(process_handle, &name);
        }

        if e.is_execve {
            // Mark this as the start time of the new thread / process.
            let time = self
                .timestamp_converter
                .convert_time(self.current_sample_time);
            self.profile.set_thread_start_time(thread_handle, time);
            if is_main {
                self.profile.set_process_start_time(process_handle, time);
            }
        }

        if self.delayed_product_name_generator.is_some() && name != "perf-exec" {
            let generator = self.delayed_product_name_generator.take().unwrap();
            let product = generator(&name);
            self.profile.set_product(&product);
            self.have_product_name = true;
        }
    }

    fn kernel_lib(
        &self,
        base_address: u64,
        len: u64,
        dso_key: DsoKey,
        build_id: Option<&[u8]>,
        path: &[u8],
    ) -> LibraryInfo {
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
            .map(|id| DebugId::from_identifier(id, self.little_endian));

        let debug_path = match self.linux_version.as_deref() {
            Some(linux_version) if path.starts_with("[kernel.kallsyms]") => {
                // Take a guess at the vmlinux debug file path.
                format!("/usr/lib/debug/boot/vmlinux-{}", linux_version)
            }
            _ => path.clone(),
        };
        let symbol_table = match (&dso_key, &build_id, self.kernel_symbols.as_ref()) {
            (DsoKey::Kernel, Some(build_id), Some(kernel_symbols))
                if build_id == &kernel_symbols.build_id && kernel_symbols.base_avma != 0 =>
            {
                // Run `sudo sh -c "echo 0 > /proc/sys/kernel/kptr_restrict"` to get here without root.
                Some(kernel_symbols.symbol_table.clone())
            }
            _ => None,
        };

        LibraryInfo {
            base_avma: base_address,
            avma_range: base_address..(base_address + len),
            debug_id: debug_id.unwrap_or_default(),
            path,
            debug_path,
            code_id: build_id.map(|build_id| CodeId::from_binary(&build_id)),
            name: dso_key.name().to_string(),
            debug_name: dso_key.name().to_string(),
            arch: None,
            symbol_table,
        }
    }
}

struct TimestampConverter {
    reference_ns: u64,
}

impl TimestampConverter {
    pub fn with_reference_timestamp(reference_ns: u64) -> Self {
        Self { reference_ns }
    }

    pub fn convert_time(&self, ktime_ns: u64) -> Timestamp {
        Timestamp::from_nanos_since_reference(ktime_ns.saturating_sub(self.reference_ns))
    }
}

fn process_off_cpu_sample_group(
    off_cpu_sample: OffCpuSampleGroup,
    thread_handle: ThreadHandle,
    cpu_delta_ns: u64,
    timestamp_converter: &TimestampConverter,
    off_cpu_weight_per_sample: i32,
    off_cpu_stack: &[(Frame, CategoryPairHandle)],
    profile: &mut Profile,
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
    let frames = off_cpu_stack.iter().cloned();
    let profile_timestamp = timestamp_converter.convert_time(begin_timestamp);
    profile.add_sample(thread_handle, profile_timestamp, frames, cpu_delta, weight);

    if sample_count > 1 {
        // Emit a "rest sample" with a CPU delta of zero covering the rest of the paused range.
        let cpu_delta = CpuDelta::from_nanos(0);
        let weight = i32::try_from(sample_count - 1).unwrap_or(0) * off_cpu_weight_per_sample;
        let frames = off_cpu_stack.iter().cloned();
        let profile_timestamp = timestamp_converter.convert_time(end_timestamp);
        profile.add_sample(thread_handle, profile_timestamp, frames, cpu_delta, weight);
    }
}

#[derive(Debug, Clone, Copy)]
struct StackConverter {
    user_category: CategoryPairHandle,
    kernel_category: CategoryPairHandle,
}

impl StackConverter {
    fn convert_stack(
        &self,
        stack: Vec<StackFrame>,
    ) -> impl Iterator<Item = (Frame, CategoryPairHandle)> {
        let user_category = self.user_category;
        let kernel_category = self.kernel_category;
        stack.into_iter().rev().filter_map(move |frame| {
            let (location, mode) = match frame {
                StackFrame::InstructionPointer(addr, mode) => {
                    (Frame::InstructionPointer(addr), mode)
                }
                StackFrame::ReturnAddress(addr, mode) => (Frame::ReturnAddress(addr), mode),
                StackFrame::TruncatedStackMarker => return None,
            };
            let category = match mode {
                StackMode::User => user_category,
                StackMode::Kernel => kernel_category,
            };
            Some((location, category))
        })
    }

    fn convert_stack_no_kernel<'a>(
        &self,
        stack: &'a [StackFrame],
    ) -> impl Iterator<Item = (Frame, CategoryPairHandle)> + 'a {
        let user_category = self.user_category;
        stack.iter().rev().filter_map(move |frame| {
            let (location, mode) = match *frame {
                StackFrame::InstructionPointer(addr, mode) => {
                    (Frame::InstructionPointer(addr), mode)
                }
                StackFrame::ReturnAddress(addr, mode) => (Frame::ReturnAddress(addr), mode),
                StackFrame::TruncatedStackMarker => return None,
            };
            match mode {
                StackMode::User => Some((location, user_category)),
                StackMode::Kernel => None,
            }
        })
    }
}

struct Processes<U>(HashMap<i32, Process<U>>)
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default;

impl<U> Processes<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    pub fn get_by_pid(&mut self, pid: i32, profile: &mut Profile) -> &mut Process<U> {
        self.0.entry(pid).or_insert_with(|| {
            let name = format!("<{}>", pid);
            let handle = profile.add_process(
                &name,
                pid as u32,
                Timestamp::from_millis_since_reference(0.0),
            );
            Process {
                profile_process: handle,
                unwinder: U::default(),
            }
        })
    }
}

struct Threads(HashMap<i32, Thread>);

impl Threads {
    pub fn get_by_tid(
        &mut self,
        tid: i32,
        process_handle: ProcessHandle,
        is_main: bool,
        profile: &mut Profile,
    ) -> &mut Thread {
        self.0.entry(tid).or_insert_with(|| {
            let profile_thread = profile.add_thread(
                process_handle,
                tid as u32,
                Timestamp::from_millis_since_reference(0.0),
                is_main,
            );
            Thread {
                profile_thread,
                context_switch_data: Default::default(),
                last_sample_timestamp: None,
                off_cpu_stack: Vec::new(),
            }
        })
    }
}

struct Thread {
    profile_thread: ThreadHandle,
    context_switch_data: ThreadContextSwitchData,
    last_sample_timestamp: Option<u64>,
    off_cpu_stack: Vec<(Frame, CategoryPairHandle)>,
}

struct Process<U> {
    pub profile_process: ProcessHandle,
    pub unwinder: U,
}

#[derive(Clone, Debug)]
pub enum StackFrame {
    InstructionPointer(u64, StackMode),
    ReturnAddress(u64, StackMode),
    TruncatedStackMarker,
}

#[derive(Debug, Clone, Copy)]
pub enum StackMode {
    User,
    Kernel,
}

impl StackMode {
    /// Detect stack mode from a "context frame".
    ///
    /// Context frames are present in sample callchains; they're u64 addresses
    /// which are `>= PERF_CONTEXT_MAX`.
    pub fn from_context_frame(frame: u64) -> Option<Self> {
        match frame {
            PERF_CONTEXT_KERNEL | PERF_CONTEXT_GUEST_KERNEL => Some(Self::Kernel),
            PERF_CONTEXT_USER | PERF_CONTEXT_GUEST | PERF_CONTEXT_GUEST_USER => Some(Self::User),
            _ => None,
        }
    }
}

impl From<CpuMode> for StackMode {
    /// Convert CpuMode into StackMode.
    fn from(cpu_mode: CpuMode) -> Self {
        match cpu_mode {
            CpuMode::Kernel | CpuMode::GuestKernel => Self::Kernel,
            _ => Self::User,
        }
    }
}

fn open_file_with_fallback(
    path: &Path,
    extra_dir: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    match (std::fs::File::open(path), extra_dir, path.file_name()) {
        (Err(_), Some(extra_dir), Some(filename)) => {
            let p: PathBuf = [extra_dir, Path::new(filename)].iter().collect();
            std::fs::File::open(p)
        }
        (result, _, _) => result,
    }
}

// A file range in an object file, such as a segment or a section,
// for which we know the corresponding Stated Virtual Memory Address (SVMA).
#[derive(Clone)]
struct SvmaFileRange {
    svma: u64,
    file_offset: u64,
    size: u64,
}

impl SvmaFileRange {
    pub fn from_segment<'data, S: ObjectSegment<'data>>(segment: S) -> Self {
        let svma = segment.address();
        let (file_offset, size) = segment.file_range();
        SvmaFileRange {
            svma,
            file_offset,
            size,
        }
    }

    pub fn from_section<'data, S: ObjectSection<'data>>(section: S) -> Option<Self> {
        let svma = section.address();
        let (file_offset, size) = section.file_range()?;
        Some(SvmaFileRange {
            svma,
            file_offset,
            size,
        })
    }

    pub fn encompasses_file_range(&self, other_file_offset: u64, other_file_size: u64) -> bool {
        let self_file_range_end = self.file_offset + self.size;
        let other_file_range_end = other_file_offset + other_file_size;
        self.file_offset <= other_file_offset && other_file_range_end <= self_file_range_end
    }

    pub fn is_encompassed_by_file_range(
        &self,
        other_file_offset: u64,
        other_file_size: u64,
    ) -> bool {
        let self_file_range_end = self.file_offset + self.size;
        let other_file_range_end = other_file_offset + other_file_size;
        other_file_offset <= self.file_offset && self_file_range_end <= other_file_range_end
    }
}

impl Debug for SvmaFileRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvmaFileRange")
            .field("svma", &format!("{:#x}", &self.svma))
            .field("file_offset", &format!("{:#x}", &self.file_offset))
            .field("size", &format!("{:#x}", &self.size))
            .finish()
    }
}

/// Compute the image base address, in process memory, i.e. as an "actual virtual memory address".
/// We have the section + segment information of the mapped object file, and we know the file offset
/// and size of the mapping, as well as the AVMA at the mapping start.
///
/// An image is mapped into memory using ELF load commands ("segments"). Usually there are multiple
/// ELF load commands, resulting in multiple mappings. Some of these mappings will be read-only,
/// and some will be executable.
///
/// Commonly, the executable mapping is the second of four mappings.
///
/// If we know about all mappings of an image, then the base AVMA is the start AVMA of the first mapping.
/// But sometimes we only have one mapping of an image, for example only the second mapping. This
/// happens if the `perf.data` file only contains mmap information of the executable mappings. In that
/// case, we have to look at the "page offset" of that mapping and find out which part of the file
/// was mapped in that range.
///
/// In that case it's tempting to say "The AVMA is the address at which the file would start, if
/// the entire file was mapped contiguously into memory around our mapping." While this works in
/// many cases, it doesn't work if there are "SVMA gaps" between the segments which have been elided
/// in the file, i.e. it doesn't work for files where the file offset <-> SVMA translation
/// is different for each segment.
///
/// Easy case A:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:         0x0 |----------------| |-------------|
/// AVMA:   0x1750000 |----------------| |-------------|
/// ```
///
/// Easy case B:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:     0x40000 |----------------| |-------------|
/// AVMA:   0x1750000 |----------------| |-------------|
/// ```
///
/// Hard case:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:         0x0 |----------------|         |-------------|
/// AVMA:   0x1750000 |----------------|         |-------------|
/// ```
///
/// One example of the hard case has been observed in `libxul.so`: The `.text` section
/// was in the second segment. In the first segment, SVMAs were equal to their
/// corresponding file offsets. In the second segment, SMAs were 0x1000 bytes higher
/// than their corresponding file offsets. In other words, there was a 0x1000-wide gap
/// between the segments in virtual address space, but this gap was omitted in the file.
/// The SVMA gap exists in the AVMAs too - the "bias" between SVMAs and AVMAs is the same
/// for all segments of an image. So we have to find the SVMA for the mapping by finding
/// a segment or section which overlaps the mapping in file offset space, and then use
/// the matching segment's / section's SVMA to find the SVMA-to-AVMA "bias" for the
/// mapped bytes.
///
/// Once we have the SVMA-to-AVMA bias, we can find the image base AVMA by translating the
/// SVMA of the *first* segment.
///
/// Another interesting edge case we observed was a case where a mapping was seemingly
/// not initiated by an ELF LOAD command: Part of the d8 binary (the V8 shell) was mapped
/// into memory with a mapping that covered only a small part of the `.text` section.
/// Usually, you'd expect a section to be mapped in its entirety, but this was not the
/// case here. So the segment finding code below checks for containment both ways: Whether
/// the mapping is contained in the segment, or whether the segment is contained in the
/// mapping. We also tried a solution where we just check for overlap between the segment
/// and the mapping, but this sometimes got the wrong segment, because the mapping is
/// larger than the segment due to alignment, and can extend into other segments.
fn compute_base_avma<'data, 'file, O>(
    file: &'file O,
    mapping_start_file_offset: u64,
    mapping_start_avma: u64,
    mapping_size: u64,
) -> Option<u64>
where
    'data: 'file,
    O: Object<'data, 'file>,
{
    let segments: Vec<SvmaFileRange> = file.segments().map(SvmaFileRange::from_segment).collect();

    let text_sections: Vec<SvmaFileRange> = file
        .sections()
        .filter(|s| s.kind() == SectionKind::Text)
        .filter_map(SvmaFileRange::from_section)
        .collect();

    compute_base_avma_impl(
        &segments,
        &text_sections,
        mapping_start_file_offset,
        mapping_start_avma,
        mapping_size,
    )
}

fn compute_base_avma_impl(
    segments: &[SvmaFileRange],
    text_sections: &[SvmaFileRange],
    mapping_file_offset: u64,
    mapping_avma: u64,
    mapping_size: u64,
) -> Option<u64> {
    // Find a segment or section which either fully contains the mapping, or which is fully contained by the mapping.
    // Linux perf simply always uses the .text section as the reference contribution.
    let is_matching_contribution = |contribution: &&SvmaFileRange| {
        contribution.encompasses_file_range(mapping_file_offset, mapping_size)
            || contribution.is_encompassed_by_file_range(mapping_file_offset, mapping_size)
    };

    let ref_contribution = if let Some(contribution) =
        segments.iter().find(is_matching_contribution)
    {
        contribution
    } else if let Some(contribution) = text_sections.iter().find(is_matching_contribution) {
        contribution
    } else {
        println!(
            "Could not find segment or section corresponding to the mapping's file offset range 0x{:x}..0x{:x}",
            mapping_file_offset,
            mapping_file_offset + mapping_size,
        );
        return None;
    };

    // Compute the AVMA at which the reference contribution is located in process memory.
    let ref_avma = if ref_contribution.file_offset > mapping_file_offset {
        mapping_avma + (ref_contribution.file_offset - mapping_file_offset)
    } else {
        mapping_avma - (mapping_file_offset - ref_contribution.file_offset)
    };

    // Compute the image base address in SVMA space. The image base for ELF images
    // is defined as the address of the first segment.
    let base_svma = match segments.first() {
        Some(first_segment) => first_segment.svma,
        None => 0,
    };

    // We have everything we need now.
    let bias = ref_avma - ref_contribution.svma;
    let base_avma = base_svma + bias;
    Some(base_avma)
}

#[test]
fn test_compute_base_avma_impl() {
    #[cfg(any())]
    {
        let data = std::fs::read("/Users/mstange/Downloads/jitted-103168-33.so").unwrap();
        let file = object::File::parse(&data[..]).unwrap();
        let segments: Vec<SvmaFileRange> =
            file.segments().map(SvmaFileRange::from_segment).collect();
        let text_sections: Vec<SvmaFileRange> = file
            .sections()
            .filter(|s| s.kind() == SectionKind::Text)
            .filter_map(SvmaFileRange::from_section)
            .collect();
        dbg!(segments);
        dbg!(text_sections);
    }

    // From a local build of the Spidermonkey shell ("js")
    let js_segments = &[
        SvmaFileRange {
            svma: 0x0,
            file_offset: 0x0,
            size: 0x14bd0bc,
        },
        SvmaFileRange {
            svma: 0x14be0c0,
            file_offset: 0x14bd0c0,
            size: 0xf5bf60,
        },
        SvmaFileRange {
            svma: 0x241b020,
            file_offset: 0x2419020,
            size: 0x08e920,
        },
        SvmaFileRange {
            svma: 0x24aa940,
            file_offset: 0x24a7940,
            size: 0x002d48,
        },
    ];
    let js_text_sections = &[
        SvmaFileRange {
            svma: 0x1429b00,
            file_offset: 0x1428b00,
            size: 0xf2b915,
        },
        SvmaFileRange {
            svma: 0x2355418,
            file_offset: 0x2354418,
            size: 0x1a,
        },
        SvmaFileRange {
            svma: 0x2355434,
            file_offset: 0x2354434,
            size: 0x9,
        },
        SvmaFileRange {
            svma: 0x2355440,
            file_offset: 0x2354440,
            size: 0xf40,
        },
    ];
    assert_eq!(
        compute_base_avma_impl(
            js_segments,
            js_text_sections,
            0x14bd0c0,
            0x100014be0c0,
            0xf5bf60
        ),
        Some(0x10000000000)
    );
    assert_eq!(
        compute_base_avma_impl(
            js_segments,
            js_text_sections,
            0x14bd000,
            0x55d605384000,
            0xf5d000
        ),
        Some(0x55d603ec6000)
    );

    // From a local build of the V8 shell ("d8")
    let d8_segments = &[
        SvmaFileRange {
            svma: 0x0,
            file_offset: 0x0,
            size: 0x3c8ed8,
        },
        SvmaFileRange {
            svma: 0x03ca000,
            file_offset: 0x3c9000,
            size: 0xfec770,
        },
        SvmaFileRange {
            svma: 0x13b7770,
            file_offset: 0x13b5770,
            size: 0x0528d0,
        },
        SvmaFileRange {
            svma: 0x140c000,
            file_offset: 0x1409000,
            size: 0x0118f0,
        },
    ];
    let d8_text_sections = &[
        SvmaFileRange {
            svma: 0x3ca000,
            file_offset: 0x3c9000,
            size: 0xfeb86e,
        },
        SvmaFileRange {
            svma: 0x13b5870,
            file_offset: 0x13b4870,
            size: 0x17,
        },
        SvmaFileRange {
            svma: 0x13b5888,
            file_offset: 0x13b4888,
            size: 0x9,
        },
        SvmaFileRange {
            svma: 0x13b58a0,
            file_offset: 0x13b48a0,
            size: 0xed0,
        },
    ];
    assert_eq!(
        compute_base_avma_impl(
            d8_segments,
            d8_text_sections,
            0x1056000,
            0x55d15fe80000,
            0x180000
        ),
        Some(0x55d15ee29000)
    );

    // From a perf-jitted-1234-12.so from before https://github.com/torvalds/linux/commit/babd04386b1df8c364cdaa39ac0e54349502e1e5
    let jitted_so_segments = &[];
    let jitted_so_text_sections = &[SvmaFileRange {
        svma: 0x40,
        file_offset: 0x40,
        size: 0x50,
    }];
    assert_eq!(
        compute_base_avma_impl(
            jitted_so_segments,
            jitted_so_text_sections,
            0x40,
            0x1000040,
            0x50
        ),
        Some(0x1000000)
    );

    // From a perf-jitted-1234-12.so from after https://github.com/torvalds/linux/commit/babd04386b1df8c364cdaa39ac0e54349502e1e5
    let jitted_so_segments = &[SvmaFileRange {
        svma: 0x0,
        file_offset: 0x0,
        size: 0x50, // should be 0x90, but fails to take ELF header size into account
    }];
    let jitted_so_text_sections = &[SvmaFileRange {
        svma: 0x40,
        file_offset: 0x40,
        size: 0x50,
    }];
    assert_eq!(
        compute_base_avma_impl(
            jitted_so_segments,
            jitted_so_text_sections,
            0x40,
            0x1000040,
            0x50
        ),
        Some(0x1000000)
    );

    // Desired perf-jitted-1234-12.so case (would require fixing perf in two places: fix PT_LOAD size, and make synthesizem mmap cover the whole segment instead of just the text section)
    let jitted_so_segments = &[SvmaFileRange {
        svma: 0x0,
        file_offset: 0x0,
        size: 0x90,
    }];
    let jitted_so_text_sections = &[SvmaFileRange {
        svma: 0x40,
        file_offset: 0x40,
        size: 0x50,
    }];
    assert_eq!(
        compute_base_avma_impl(
            jitted_so_segments,
            jitted_so_text_sections,
            0x0,
            0x1000000,
            0x90
        ),
        Some(0x1000000)
    );
}

/// Tell the unwinder about this module, and alsos create a ProfileModule
/// so that the profile can be told about this module.
///
/// The unwinder needs to know about it in case we need to do DWARF stack
/// unwinding - it needs to get the unwinding information from the binary.
/// The profile needs to know about this module so that it can assign
/// addresses in the stack to the right module and so that symbolication
/// knows where to get symbols for this module.
fn add_module_to_unwinder<U>(
    unwinder: &mut U,
    path_slice: &[u8],
    mapping_start_file_offset: u64,
    mapping_start_avma: u64,
    mapping_size: u64,
    build_id: Option<&[u8]>,
    extra_binary_artifact_dir: Option<&Path>,
) -> Option<LibraryInfo>
where
    U: Unwinder<Module = Module<Vec<u8>>>,
{
    let path = std::str::from_utf8(path_slice).unwrap();
    let objpath = Path::new(path);

    let file = open_file_with_fallback(objpath, extra_binary_artifact_dir).ok();
    if file.is_none() && !path.starts_with('[') {
        // eprintln!("Could not open file {:?}", objpath);
        // eprintln!("mapping: file_offset: {:#x}, avma: {:#x}, size: {:#x}", mapping_start_file_offset, mapping_start_avma, mapping_size);
    }

    let mapping_end_avma = mapping_start_avma + mapping_size;
    let avma_range = mapping_start_avma..mapping_end_avma;

    let code_id;
    let debug_id;
    let base_avma;

    if let Some(file) = file {
        let mmap = match unsafe { memmap2::MmapOptions::new().map(&file) } {
            Ok(mmap) => mmap,
            Err(err) => {
                eprintln!("Could not mmap file {}: {:?}", path, err);
                return None;
            }
        };

        fn section_data<'a>(section: &impl ObjectSection<'a>) -> Option<Vec<u8>> {
            section.uncompressed_data().ok().map(|data| data.to_vec())
        }

        let file = match object::File::parse(&mmap[..]) {
            Ok(file) => file,
            Err(_) => {
                eprintln!("File {:?} has unrecognized format", objpath);
                return None;
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
                        "File {:?} has non-matching build ID {} (expected {})",
                        objpath, file_build_id, expected_build_id
                    );
                    return None;
                }
                None => {
                    eprintln!(
                        "File {:?} does not contain a build ID, but we expected it to have one",
                        objpath
                    );
                    return None;
                }
            }
        }

        let base_svma = 0;
        base_avma = compute_base_avma(
            &file,
            mapping_start_file_offset,
            mapping_start_avma,
            mapping_size,
        )?;

        let text = file.section_by_name(".text");
        let text_env = file.section_by_name("text_env");
        let eh_frame = file.section_by_name(".eh_frame");
        let got = file.section_by_name(".got");
        let eh_frame_hdr = file.section_by_name(".eh_frame_hdr");

        let unwind_data = match (
            eh_frame.as_ref().and_then(section_data),
            eh_frame_hdr.as_ref().and_then(section_data),
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
            text_segment
                .data()
                .ok()
                .map(|data| TextByteData::new(data.to_owned(), address_range))
        } else if let Some(text_section) = &text {
            if let Some((start, size)) = text_section.file_range() {
                let address_range = base_avma + start..base_avma + start + size;
                text_section
                    .data()
                    .ok()
                    .map(|data| TextByteData::new(data.to_owned(), address_range))
            } else {
                None
            }
        } else {
            None
        };

        fn svma_range<'a>(section: &impl ObjectSection<'a>) -> Range<u64> {
            section.address()..section.address() + section.size()
        }

        let module = Module::new(
            path.to_string(),
            avma_range.clone(),
            base_avma,
            ModuleSvmaInfo {
                base_svma,
                text: text.as_ref().map(svma_range),
                text_env: text_env.as_ref().map(svma_range),
                stubs: None,
                stub_helper: None,
                eh_frame: eh_frame.as_ref().map(svma_range),
                eh_frame_hdr: eh_frame_hdr.as_ref().map(svma_range),
                got: got.as_ref().map(svma_range),
            },
            unwind_data,
            text_data,
        );
        unwinder.add_module(module);

        debug_id = debug_id_for_object(&file)?;
        code_id = file.build_id().ok().flatten().map(CodeId::from_binary);
    } else {
        // Without access to the binary file, make some guesses. We can't really
        // know what the right base address is because we don't have the section
        // information which lets us map between addresses and file offsets, but
        // often svmas and file offsets are the same, so this is a reasonable guess.
        base_avma = mapping_start_avma - mapping_start_file_offset;

        // If we have a build ID, convert it to a debug_id and a code_id.
        debug_id = build_id
            .map(|id| DebugId::from_identifier(id, true)) // TODO: endian
            .unwrap_or_default();
        code_id = build_id.map(CodeId::from_binary);
    }

    let name = objpath
        .file_name()
        .map_or("<unknown>".into(), |f| f.to_string_lossy().to_string());
    Some(LibraryInfo {
        base_avma,
        avma_range,
        debug_id,
        code_id,
        path: path.to_string(),
        debug_path: path.to_string(),
        debug_name: name.clone(),
        name,
        arch: None,
        symbol_table: None,
    })
}

fn kernel_module_build_id(
    path: &Path,
    extra_binary_artifact_dir: Option<&Path>,
) -> Option<Vec<u8>> {
    let file = open_file_with_fallback(path, extra_binary_artifact_dir).ok()?;
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }.ok()?;
    let obj = object::File::parse(&mmap[..]).ok()?;
    match obj.build_id() {
        Ok(Some(build_id)) => Some(build_id.to_owned()),
        _ => None,
    }
}
