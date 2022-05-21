use byteorder::LittleEndian;
use debugid::{CodeId, DebugId};
use framehop::aarch64::UnwindRegsAarch64;
use framehop::x86_64::UnwindRegsX86_64;
use framehop::{FrameAddress, Module, ModuleSvmaInfo, ModuleUnwindData, TextByteData, Unwinder};
use fxprof_processed_profile::{
    CpuDelta, Frame, LibraryInfo, ProcessHandle, Profile, ReferenceTimestamp, ThreadHandle,
    Timestamp,
};
use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::{DsoBuildId, DsoKey, PerfFileReader, SampleTimeRange};
use linux_perf_event_reader::consts::{
    PERF_CONTEXT_MAX, PERF_REG_ARM64_LR, PERF_REG_ARM64_PC, PERF_REG_ARM64_SP, PERF_REG_ARM64_X29,
    PERF_REG_X86_BP, PERF_REG_X86_IP, PERF_REG_X86_SP,
};
use linux_perf_event_reader::records::{
    CommOrExecRecord, ForkOrExitRecord, Mmap2FileId, Mmap2Record, MmapRecord, ParsedRecord, Regs,
    SampleRecord,
};
use linux_perf_event_reader::RawDataU64;
use object::{Object, ObjectSection, ObjectSegment, SectionKind};
use profiler_get_symbols::{debug_id_for_object, DebugIdExt};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use std::{fs::File, ops::Range, path::Path};

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.len() < 1 {
        eprintln!("Usage: {} <path>", std::env::args().next().unwrap());
        std::process::exit(1);
    }
    let path = args.next().unwrap();
    let path = Path::new(&path)
        .canonicalize()
        .expect("Couldn't form absolute path");

    let input_file = File::open(&path).unwrap();
    let reader = BufReader::new(input_file);
    let perf_file = PerfFileReader::parse_file(reader).expect("Parsing failed");

    let profile = match perf_file.arch().unwrap() {
        Some("x86_64") => {
            let cache = framehop::x86_64::CacheX86_64::new();
            convert::<framehop::x86_64::UnwinderX86_64<Vec<u8>>, ConvertRegsX86_64, _>(
                perf_file,
                path.parent(),
                cache,
            )
        }
        Some("aarch64") => {
            let cache = framehop::aarch64::CacheAarch64::new();
            convert::<framehop::aarch64::UnwinderAarch64<Vec<u8>>, ConvertRegsAarch64, _>(
                perf_file,
                path.parent(),
                cache,
            )
        }
        Some(other_arch) => {
            eprintln!("Unrecognized arch {}", other_arch);
            std::process::exit(1);
        }
        None => {
            eprintln!("Can't unwind because I don't know the arch");
            std::process::exit(1);
        }
    };

    let output_file = File::create("profile-conv.json").unwrap();
    let writer = BufWriter::new(output_file);
    serde_json::to_writer(writer, &profile).expect("Couldn't write JSON");
    eprintln!("Saved converted profile to profile-conv.json");
}

trait ConvertRegs {
    type UnwindRegs;
    fn convert_regs(regs: &Regs) -> (u64, u64, Self::UnwindRegs);
}

struct ConvertRegsX86_64;
impl ConvertRegs for ConvertRegsX86_64 {
    type UnwindRegs = UnwindRegsX86_64;
    fn convert_regs(regs: &Regs) -> (u64, u64, UnwindRegsX86_64) {
        let ip = regs.get(PERF_REG_X86_IP).unwrap();
        let sp = regs.get(PERF_REG_X86_SP).unwrap();
        let bp = regs.get(PERF_REG_X86_BP).unwrap();
        let regs = UnwindRegsX86_64::new(ip, sp, bp);
        (ip, sp, regs)
    }
}

struct ConvertRegsAarch64;
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
}

fn convert<U, C, R>(
    mut file: PerfFileReader<R>,
    extra_dir: Option<&Path>,
    cache: U::Cache,
) -> Profile
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
    C: ConvertRegs<UnwindRegs = U::UnwindRegs>,
    R: Read,
{
    let build_ids = file.build_ids().ok().unwrap_or_default();
    let SampleTimeRange {
        first_sample_time, ..
    } = file.sample_time_range().unwrap().unwrap();
    let little_endian = file.endian() == linux_perf_data::Endianness::LittleEndian;
    let host = file.hostname().unwrap().unwrap_or("<unknown host>");
    let perf_version = file.perf_version().unwrap().unwrap_or("<unknown host>");
    let linux_version = file.perf_version().unwrap();

    let product = "Converted perf profile";
    let mut converter = Converter::<U>::new(
        product,
        build_ids,
        first_sample_time,
        host,
        perf_version,
        linux_version,
        little_endian,
        cache,
        extra_dir,
    );

    while let Ok(Some(record)) = file.next_record() {
        match record {
            ParsedRecord::Sample(e) => {
                converter.handle_sample::<C>(e);
            }
            ParsedRecord::Fork(e) => {
                converter.handle_thread_start(e);
            }
            ParsedRecord::Comm(e) => {
                converter.handle_thread_name_update(e);
            }
            ParsedRecord::Exit(e) => {
                converter.handle_thread_end(e);
            }
            ParsedRecord::Mmap(e) => {
                converter.handle_mmap(e);
            }
            ParsedRecord::Mmap2(e) => {
                converter.handle_mmap2(e);
            }
            _ => {}
        }
    }

    converter.finish()
}

struct Converter<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    cache: U::Cache,
    profile: Profile,
    processes: Processes<U>,
    threads: Threads,
    kernel_modules: Vec<LibraryInfo>,
    first_sample_time: u64,
    current_sample_time: u64,
    build_ids: HashMap<DsoKey, DsoBuildId>,
    little_endian: bool,
    have_product_name: bool,
    host: String,
    perf_version: String,
    linux_version: Option<String>,
    extra_binary_artifact_dir: Option<PathBuf>,
}

impl<U> Converter<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        product: &str,
        build_ids: HashMap<DsoKey, DsoBuildId>,
        first_sample_time: u64,
        host: &str,
        perf_version: &str,
        linux_version: Option<&str>,
        little_endian: bool,
        cache: U::Cache,
        extra_binary_artifact_dir: Option<&Path>,
    ) -> Self {
        Self {
            profile: Profile::new(
                product,
                ReferenceTimestamp::from_system_time(SystemTime::now()),
                Duration::from_millis(1),
            ),
            cache,
            processes: Processes(HashMap::new()),
            threads: Threads(HashMap::new()),
            kernel_modules: Vec::new(),
            first_sample_time,
            current_sample_time: first_sample_time,
            build_ids,
            little_endian,
            have_product_name: false,
            host: host.to_string(),
            perf_version: perf_version.to_string(),
            linux_version: linux_version.map(ToOwned::to_owned),
            extra_binary_artifact_dir: extra_binary_artifact_dir.map(ToOwned::to_owned),
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
        let cpu_delta = if let Some(period) = e.period {
            // If the observed perf event is one of the clock time events, or cycles, then we should convert it to a CpuDelta.
            // TODO: Detect event type
            CpuDelta::from_nanos(period)
        } else {
            CpuDelta::from_nanos(0)
        };
        let is_main = pid == tid;
        let process = self
            .processes
            .get_by_pid(pid, &mut self.profile, &self.kernel_modules);

        let mut stack = Vec::new();
        Self::get_sample_stack::<C>(&e, &process.unwinder, &mut self.cache, &mut stack);

        let thread =
            self.threads
                .get_by_tid(tid, process.profile_process, is_main, &mut self.profile);
        let timestamp = self.convert_time(timestamp);
        let frames = stack.into_iter().rev().filter_map(|frame| match frame {
            StackFrame::InstructionPointer(addr) => Some(Frame::InstructionPointer(addr)),
            StackFrame::ReturnAddress(addr) => Some(Frame::ReturnAddress(addr)),
            StackFrame::TruncatedStackMarker => None,
        });
        self.profile
            .add_sample(thread, timestamp, frames, cpu_delta, 1);
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
    ///    or more explicitly `perf record --call-graph fp`), stack unwinding
    ///    happens in the kernel, and the user stack is appended to e.callchain.
    ///    We can just get it from there.
    ///  - With DWARF unwinding (`perf record --call-graph dwarf`), we need to
    ///    do unwinding now, based on the register values in `e.user_regs` and
    ///    and the raw stack bytes in `e.user_stack`.
    fn get_sample_stack<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        e: &SampleRecord,
        unwinder: &U,
        cache: &mut U::Cache,
        stack: &mut Vec<StackFrame>,
    ) {
        stack.truncate(0);

        // Get the first fragment of the stack from e.callchain.
        if let Some(callchain) = e.callchain {
            let mut is_first_frame = true;
            for i in 0..callchain.len() {
                let address = callchain.get(i).unwrap();
                if address >= PERF_CONTEXT_MAX {
                    // Ignore synthetic addresses like 0xffffffffffffff80.
                    continue;
                }

                let stack_frame = match is_first_frame {
                    true => StackFrame::InstructionPointer(address),
                    false => StackFrame::ReturnAddress(address),
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
                    FrameAddress::InstructionPointer(addr) => StackFrame::InstructionPointer(addr),
                    FrameAddress::ReturnAddress(addr) => StackFrame::ReturnAddress(addr.into()),
                };
                stack.push(stack_frame);
            }
        }

        if stack.is_empty() {
            if let Some(ip) = e.ip {
                stack.push(StackFrame::InstructionPointer(ip));
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
            // ID info can be "better". For example, sometimes the synthesized mmap event for
            // the kernel vmlinux image usually has "[kernel.kallsyms]_text" whereas the
            // build ID info might have the full path to a kernel debug file, e.g.
            // "/usr/lib/debug/boot/vmlinux-4.16.0-1-amd64".
            path = Cow::Borrowed(&dso_info.path);
        }

        if e.pid == -1 {
            let debug_id = build_id.map(|id| DebugId::from_identifier(id, self.little_endian));
            let path = std::str::from_utf8(&path).unwrap().to_string();
            let mut debug_path = path.clone();
            if debug_path.starts_with("[kernel.kallsyms]") {
                if let Some(linux_version) = self.linux_version.as_deref() {
                    // Take a guess at the vmlinux debug file path.
                    debug_path = format!("/usr/lib/debug/boot/vmlinux-{}", linux_version);
                }
            }

            self.kernel_modules.push(LibraryInfo {
                base_avma: e.address,
                avma_range: e.address..(e.address + e.length),
                debug_id: debug_id.unwrap_or_default(),
                path,
                debug_path,
                code_id: build_id.map(CodeId::from_binary),
                name: dso_key.name().to_string(),
                debug_name: dso_key.name().to_string(),
                arch: None,
            });
        } else {
            let process = self
                .processes
                .get_by_pid(e.pid, &mut self.profile, &self.kernel_modules);
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

        let process = self
            .processes
            .get_by_pid(e.pid, &mut self.profile, &self.kernel_modules);
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

    pub fn handle_thread_start(&mut self, e: ForkOrExitRecord) {
        let is_main = e.pid == e.tid;
        let start_time = self.convert_time(e.timestamp);
        let process = self
            .processes
            .get_by_pid(e.pid, &mut self.profile, &self.kernel_modules);
        let process_handle = process.profile_process;
        if is_main {
            self.profile
                .set_process_start_time(process_handle, start_time);
        }
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);
        self.profile.set_thread_start_time(thread, start_time);
    }

    pub fn handle_thread_end(&mut self, e: ForkOrExitRecord) {
        let is_main = e.pid == e.tid;
        let end_time = self.convert_time(e.timestamp);
        let process = self
            .processes
            .get_by_pid(e.pid, &mut self.profile, &self.kernel_modules);
        let process_handle = process.profile_process;
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);
        self.profile.set_thread_end_time(thread, end_time);
        self.threads.0.remove(&e.tid);
        if is_main {
            self.profile.set_process_end_time(process_handle, end_time);
            self.processes.0.remove(&e.pid);
        }
    }

    pub fn handle_thread_name_update(&mut self, e: CommOrExecRecord) {
        let is_main = e.pid == e.tid;
        if e.is_execve {
            // Mark the old thread / process as ended.
            // Unfortunately the COMM records don't come with a timestamp, so we just take
            // the last seen timestamp from the previosu sample.
            // TODO: Verify that this is true for all COMM records and not just for the
            // synthesized COMM records at the start of the profile.
            let time = self.convert_time(self.current_sample_time);
            if let Some(t) = self.threads.0.get(&e.tid) {
                self.profile.set_thread_end_time(*t, time);
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
            .get_by_pid(e.pid, &mut self.profile, &self.kernel_modules)
            .profile_process;

        let name = e.name.as_slice();
        let name = String::from_utf8_lossy(&name);
        let thread = self
            .threads
            .get_by_tid(e.tid, process_handle, is_main, &mut self.profile);

        self.profile.set_thread_name(thread, &name);
        if is_main {
            self.profile.set_process_name(process_handle, &name);
        }

        if e.is_execve {
            // Mark this as the start time of the new thread / process.
            let time = self.convert_time(self.current_sample_time);
            self.profile.set_thread_start_time(thread, time);
            if is_main {
                self.profile.set_process_start_time(process_handle, time);
            }
        }

        if !self.have_product_name && name != "perf-exec" {
            let product = format!(
                "{} on {} (perf version {})",
                name, self.host, self.perf_version
            );
            self.profile.set_product(&product);
            self.have_product_name = true;
        }
    }

    fn convert_time(&self, ktime_ns: u64) -> Timestamp {
        Timestamp::from_nanos_since_reference(ktime_ns.saturating_sub(self.first_sample_time))
    }
}

struct Processes<U>(HashMap<i32, Process<U>>)
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default;

impl<U> Processes<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    pub fn get_by_pid(
        &mut self,
        pid: i32,
        profile: &mut Profile,
        global_modules: &[LibraryInfo],
    ) -> &mut Process<U> {
        self.0.entry(pid).or_insert_with(|| {
            let name = format!("<{}>", pid);
            let handle = profile.add_process(
                &name,
                pid as u32,
                Timestamp::from_millis_since_reference(0.0),
            );
            for module in global_modules.iter().cloned() {
                profile.add_lib(handle, module);
            }
            Process {
                profile_process: handle,
                unwinder: U::default(),
            }
        })
    }
}

struct Threads(HashMap<i32, ThreadHandle>);

impl Threads {
    pub fn get_by_tid(
        &mut self,
        tid: i32,
        process_handle: ProcessHandle,
        is_main: bool,
        profile: &mut Profile,
    ) -> ThreadHandle {
        *self.0.entry(tid).or_insert_with(|| {
            profile.add_thread(
                process_handle,
                tid as u32,
                Timestamp::from_millis_since_reference(0.0),
                is_main,
            )
        })
    }
}

struct Process<U> {
    pub profile_process: ProcessHandle,
    pub unwinder: U,
}

#[derive(Clone, Debug)]
pub enum StackFrame {
    InstructionPointer(u64),
    ReturnAddress(u64),
    TruncatedStackMarker,
}

fn open_file_with_fallback(
    path: &Path,
    extra_dir: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    match (std::fs::File::open(path), extra_dir, path.file_name()) {
        (Err(_), Some(extra_dir), Some(filename)) => {
            let p: PathBuf = [extra_dir, Path::new(filename)].iter().collect();
            std::fs::File::open(&p)
        }
        (result, _, _) => result,
    }
}

fn compute_image_bias<'data: 'file, 'file>(
    file: &'file impl Object<'data, 'file>,
    mapping_start_file_offset: u64,
    mapping_start_avma: u64,
    mapping_size: u64,
) -> Option<u64> {
    let mapping_end_file_offset = mapping_start_file_offset + mapping_size;

    // Find one of the text sections in this mapping, to map file offsets to SVMAs.
    // It would make more sense to ELF LOAD commands (which object exposes as
    // segments), this does not work for the synthetic .so files created by
    // `perf inject --jit` - they don't have LOAD commands.
    let (section_start_file_offset, section_start_svma) = match file
        .sections()
        .filter(|s| s.kind() == SectionKind::Text)
        .find_map(|s| match s.file_range() {
            Some((section_start_file_offset, section_size)) => {
                let section_end_file_offset = section_start_file_offset + section_size;
                if mapping_start_file_offset <= section_start_file_offset
                    && section_end_file_offset <= mapping_end_file_offset
                {
                    Some((section_start_file_offset, s.address()))
                } else {
                    None
                }
            }
            _ => None,
        }) {
        Some(section_info) => section_info,
        None => {
            println!(
                "Could not find section covering file offset range 0x{:x}..0x{:x}",
                mapping_start_file_offset, mapping_end_file_offset
            );
            return None;
        }
    };

    let section_start_avma =
        mapping_start_avma + (section_start_file_offset - mapping_start_file_offset);

    // Compute the offset between AVMAs and SVMAs. This is the bias of the image.
    Some(section_start_avma - section_start_svma)
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
    let filename = objpath.file_name().unwrap();

    let file = open_file_with_fallback(objpath, extra_binary_artifact_dir).ok();
    if file.is_none() && !path.starts_with('[') {
        eprintln!("Could not open file {:?}", objpath);
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
            section.data().ok().map(|data| data.to_owned())
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

        // Compute the AVMA that maps to SVMA zero. This is also called the "bias" of the
        // image. On ELF it is also the image load address.
        let base_svma = 0;
        base_avma = compute_image_bias(
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
        base_avma = mapping_start_avma - mapping_start_file_offset;
        debug_id = build_id
            .map(|id| DebugId::from_identifier(id, true))
            .unwrap_or_default();
        code_id = build_id.map(CodeId::from_binary);
    }

    let name = filename.to_string_lossy().to_string();
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
    })
}
