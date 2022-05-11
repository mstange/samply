mod perf_event;
pub mod perf_event_raw;
mod perf_file;
mod raw_data;
mod reader;
mod unaligned;
mod utils;

use debugid::{CodeId, DebugId};
use framehop::aarch64::UnwindRegsAarch64;
use framehop::x86_64::UnwindRegsX86_64;
use framehop::{Module, ModuleSvmaInfo, TextByteData, Unwinder};
use object::{Object, ObjectSection, ObjectSegment};
use perf_event::{Event, Mmap2Event, Mmap2FileId, MmapEvent, Regs, SampleEvent};
use perf_event_raw::{
    PERF_CONTEXT_MAX, PERF_REG_ARM64_LR, PERF_REG_ARM64_PC, PERF_REG_ARM64_SP, PERF_REG_ARM64_X29,
    PERF_REG_X86_BP, PERF_REG_X86_IP, PERF_REG_X86_SP,
};
pub use perf_file::{DsoKey, PerfFile};
use profiler_get_symbols::{
    debug_id_for_object, AddressDebugInfo, CandidatePathInfo, DebugIdExt, FileAndPathHelper,
    FileAndPathHelperResult, FileLocation, FilePath, OptionallySendFuture, SymbolicationQuery,
    SymbolicationResult, SymbolicationResultKind,
};
use std::collections::HashSet;
use std::collections::{hash_map::Entry, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::{fs::File, ops::Range, path::Path};

use crate::perf_file::DsoBuildId;

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.len() < 1 {
        eprintln!("Usage: {} <path>", std::env::args().next().unwrap());
        std::process::exit(1);
    }
    let path = args.next().unwrap();

    let file = File::open(path).unwrap();
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
    let data = &mmap[..];
    let file = PerfFile::parse(data).expect("Parsing failed");

    if let Some(hostname) = file.hostname().unwrap() {
        println!("Hostname: {}", hostname);
    }

    if let Some(os_release) = file.os_release().unwrap() {
        println!("OS release: {}", os_release);
    }

    if let Some(perf_version) = file.perf_version().unwrap() {
        println!("Perf version: {}", perf_version);
    }

    if let Some(arch) = file.arch().unwrap() {
        println!("Arch: {}", arch);
    }

    if let Some(nr_cpus) = file.nr_cpus().unwrap() {
        println!(
            "CPUs: {} online ({} available)",
            nr_cpus.nr_cpus_online.get(file.endian()),
            nr_cpus.nr_cpus_available.get(file.endian())
        );
    }

    let build_ids = file.build_ids().unwrap();
    if !build_ids.is_empty() {
        println!("Build IDs:");
        for (dso_key, DsoBuildId { path, build_id }) in build_ids {
            println!(
                " - DSO key {}, build ID {}, path {}",
                dso_key.name(),
                build_id
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<String>>()
                    .join(""),
                std::str::from_utf8(&path).unwrap()
            );
        }
    }

    match file.arch().unwrap() {
        Some("x86_64") => {
            let mut cache = framehop::x86_64::CacheX86_64::new();
            do_the_thing::<framehop::x86_64::UnwinderX86_64<Vec<u8>>, ConvertRegsX86_64>(
                &file, &mut cache,
            );
        }
        Some("aarch64") => {
            let mut cache = framehop::aarch64::CacheAarch64::new();
            do_the_thing::<framehop::aarch64::UnwinderAarch64<Vec<u8>>, ConvertRegsAarch64>(
                &file, &mut cache,
            );
        }
        Some(other_arch) => {
            eprintln!("Unrecognized arch {}", other_arch);
        }
        None => {
            eprintln!("Can't unwind because I don't know the arch");
        }
    }
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

fn do_the_thing<U, C>(file: &PerfFile, cache: &mut U::Cache)
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
    C: ConvertRegs<UnwindRegs = U::UnwindRegs>,
{
    let mut processes: HashMap<i32, Process<U>> = HashMap::new();
    let mut image_cache = ImageCache::new();
    let mut processed_samples = Vec::new();
    let mut all_image_stack_frames = HashSet::new();
    let mut kernel_modules = AddedModules(Vec::new());
    let build_ids = file.build_ids().ok().unwrap_or_default();
    let little_endian = file.endian() == unaligned::Endianness::LittleEndian;

    let mut events = file.events();
    let mut count = 0;
    while let Ok(Some(event)) = events.next() {
        count += 1;
        match event {
            Event::Sample(e) => {
                let pid = e.pid.expect("Can't handle samples without pids");
                let process = processes
                    .entry(pid)
                    .or_insert_with(|| Process::new(pid, format!("<{}>", pid).into_bytes()));
                if let Some(processed_sample) =
                    process.handle_sample::<C>(e, cache, &kernel_modules)
                {
                    for frame in &processed_sample.stack {
                        if let StackFrame::InImage(frame) = frame {
                            all_image_stack_frames.insert(frame.clone());
                        }
                    }
                    processed_samples.push(processed_sample);
                }
            }
            Event::Comm(e) => {
                println!("Comm: {:?}", e);
                match processes.entry(e.pid) {
                    Entry::Occupied(mut entry) => {
                        entry.get_mut().set_name(e.name);
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(Process::new(e.pid, e.name));
                    }
                }
            }
            Event::Exit(_e) => {
                // todo
            }
            Event::Fork(_) => {}
            Event::Mmap(e) => {
                let dso_key = match DsoKey::detect(&e.path, e.cpu_mode) {
                    Some(dso_key) => dso_key,
                    None => continue,
                };
                let build_id = build_ids.get(&dso_key).map(|db| &db.build_id[..]);
                if e.pid == -1 {
                    // println!(
                    //     "kernel mmap: 0x{:016x}-0x{:016x} (page offset 0x{:016x}) {:?} ({})",
                    //     e.address,
                    //     e.address + e.length,
                    //     e.page_offset,
                    //     std::str::from_utf8(&e.path),
                    //     e.is_executable
                    // );

                    if !e.is_executable {
                        continue;
                    }

                    let debug_id =
                        build_id.map(|buildid| DebugId::from_identifier(buildid, little_endian));

                    let start_addr = e.address;
                    let end_addr = e.address + e.length;

                    let path_str = std::str::from_utf8(&e.path).unwrap();
                    let path = Path::new(path_str);
                    let base_address = start_addr;
                    let address_range = start_addr..end_addr;
                    let image = image_cache.index_for_image(path, &dso_key, debug_id);
                    println!(
                        "0x{:016x}-0x{:016x} {:?} {:?}",
                        address_range.start,
                        address_range.end,
                        build_id.map(CodeId::from_binary),
                        path
                    );
                    kernel_modules.0.push(AddedModule {
                        address_range,
                        base_address,
                        image,
                    });
                    kernel_modules
                        .0
                        .sort_unstable_by_key(|m| m.address_range.start);
                } else {
                    let process = processes.entry(e.pid).or_insert_with(|| {
                        Process::new(e.pid, format!("<{}>", e.pid).into_bytes())
                    });
                    process.handle_mmap(e, &dso_key, build_id, &mut image_cache);
                }
            }
            Event::Mmap2(e) => {
                let dso_key = match DsoKey::detect(&e.path, e.cpu_mode) {
                    Some(dso_key) => dso_key,
                    None => continue,
                };
                let build_id = build_ids.get(&dso_key).map(|db| &db.build_id[..]);
                let process = processes
                    .entry(e.pid)
                    .or_insert_with(|| Process::new(e.pid, format!("<{}>", e.pid).into_bytes()));
                process.handle_mmap2(e, &dso_key, build_id, &mut image_cache);
            }
            Event::Lost(_) => {}
            Event::Throttle(_) => {}
            Event::Unthrottle(_) => {}
            Event::ContextSwitch(_) => {}
            Event::Raw(_) => {}
        }
    }
    println!(
        "Have {} events, converted into {} processed samples.",
        count,
        processed_samples.len()
    );
    // eprintln!("{:#?}", processed_samples);

    let mut address_results: Vec<HashMap<u32, Vec<String>>> =
        image_cache.images.iter().map(|_| HashMap::new()).collect();
    let libs: Vec<HelperLib> = image_cache
        .images
        .iter()
        .enumerate()
        .filter_map(|(image_index, image)| {
            let debug_name = image.dso_key.name().to_string();
            let debug_id = image.debug_id?;
            Some(HelperLib {
                debug_name,
                handle: ImageCacheHandle(image_index as u32),
                path: image.path.clone(),
                debug_id,
                dso_key: image.dso_key.clone(),
            })
        })
        .collect();
    let helper = Helper::with_libs(libs.clone(), false);
    for lib in libs {
        let addresses: Vec<u32> = all_image_stack_frames
            .iter()
            .filter_map(|sf| {
                if sf.image == lib.handle {
                    Some(sf.relative_lookup_address)
                } else {
                    None
                }
            })
            .collect();
        let f = profiler_get_symbols::get_symbolication_result(
            SymbolicationQuery {
                debug_name: &lib.debug_name,
                debug_id: lib.debug_id,
                result_kind: SymbolicationResultKind::SymbolsForAddresses {
                    addresses: &addresses,
                    with_debug_info: true,
                },
            },
            &helper,
        );
        let r: Result<MySymbolicationResult, _> = futures::executor::block_on(f);
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Symbolication error: {:?}", e);
                continue;
            }
        };
        address_results[lib.handle.0 as usize] = r.map;
    }

    for sample in processed_samples {
        println!(
            "Sample at t={} pid={} tid={}",
            sample.timestamp, sample.pid, sample.tid
        );
        for frame in sample.stack {
            match frame {
                StackFrame::InImage(StackFrameInImage {
                    image,
                    relative_lookup_address,
                }) => {
                    if let Some(frame_strings) =
                        address_results[image.0 as usize].get(&relative_lookup_address)
                    {
                        for frame_string in frame_strings {
                            println!("  {}", frame_string);
                        }
                    } else {
                        println!(
                            "  0x{:x} (in {})",
                            relative_lookup_address,
                            image_cache.images[image.0 as usize].dso_key.name()
                        );
                    }
                }
                StackFrame::Address(address) => {
                    println!("  0x{:x}", address);
                }
                StackFrame::TruncatedStackMarker => {
                    println!("  <truncated stack>");
                }
            }
        }
        println!();
    }
}

struct MySymbolicationResult {
    map: HashMap<u32, Vec<String>>,
}

impl SymbolicationResult for MySymbolicationResult {
    fn from_full_map<S>(_map: Vec<(u32, S)>) -> Self
    where
        S: std::ops::Deref<Target = str>,
    {
        panic!("Should not be called")
    }

    fn for_addresses(_addresses: &[u32]) -> Self {
        // yes correct
        Self {
            map: HashMap::new(),
        }
    }

    fn add_address_symbol(
        &mut self,
        address: u32,
        _symbol_address: u32,
        symbol_name: &str,
        _function_size: Option<u32>,
    ) {
        self.map.insert(address, vec![symbol_name.to_owned()]);
    }

    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo) {
        let funcs: Vec<String> = info
            .frames
            .into_iter()
            .map(|frame| {
                let mut s = frame.function.unwrap_or_else(|| "<unknown>".to_string());
                if let Some(file) = frame.file_path {
                    s.push_str(" (");
                    let file = match file {
                        FilePath::Normal(f) => f,
                        FilePath::Mapped { raw, .. } => raw,
                    };
                    s.push_str(&file);
                    if let Some(line) = frame.line_number {
                        s.push_str(&format!(":{}", line));
                    }
                    s.push(')');
                }
                s
            })
            .collect();
        self.map.insert(address, funcs);
    }

    fn set_total_symbol_count(&mut self, _total_symbol_count: u32) {
        // ignored
    }
}

struct Process<U> {
    #[allow(unused)]
    pid: i32,
    name: Vec<u8>,
    unwinder: U,
    added_modules: AddedModules,
}

struct AddedModules(pub Vec<AddedModule>);

impl AddedModules {
    pub fn map_address(&self, address: u64) -> Option<(ImageCacheHandle, u32)> {
        let module = match self
            .0
            .binary_search_by_key(&address, |m| m.address_range.start)
        {
            Ok(i) => &self.0[i],
            Err(insertion_index) => {
                if insertion_index == 0 {
                    // address is before first known module
                    return None;
                }
                let i = insertion_index - 1;
                let module = &self.0[i];
                if module.address_range.end <= address {
                    // address is after this module
                    return None;
                }
                module
            }
        };
        if address < module.base_address {
            // Invalid base address
            return None;
        }
        let relative_address = u32::try_from(address - module.base_address).ok()?;
        Some((module.image, relative_address))
    }
}

impl<U: Unwinder + Default> Process<U> {
    pub fn new(pid: i32, name: Vec<u8>) -> Self {
        Self {
            pid,
            name,
            unwinder: U::default(),
            added_modules: AddedModules(Vec::new()),
        }
    }
}

impl<U> Process<U> {
    pub fn set_name(&mut self, name: Vec<u8>) {
        self.name = name;
    }
}

impl<U> Process<U>
where
    U: Unwinder<Module = Module<Vec<u8>>>,
{
    pub fn handle_mmap(
        &mut self,
        e: MmapEvent,
        dso_key: &DsoKey,
        build_id: Option<&[u8]>,
        image_cache: &mut ImageCache,
    ) {
        // println!(
        //     "raw1 ({}): 0x{:016x}-0x{:016x} {:?}",
        //     self.pid,
        //     e.address,
        //     e.address + e.length,
        //     std::str::from_utf8(&e.path)
        // );

        if !e.is_executable {
            // Ignore non-executable mappings.
            return;
        }

        let path_str = std::str::from_utf8(&e.path).unwrap();
        let path = Path::new(path_str);
        let start_addr = e.address;
        let end_addr = e.address + e.length;
        let address_range = start_addr..end_addr;

        let (debug_id, base_address) = match add_module(
            &mut self.unwinder,
            path,
            e.page_offset,
            e.address,
            e.length,
            build_id,
        ) {
            Some(module_info) => module_info,
            None => return,
        };
        let image = image_cache.index_for_image(path, dso_key, Some(debug_id));
        println!(
            "0x{:016x}-0x{:016x} {:?}",
            start_addr,
            e.address + e.length,
            path
        );
        self.added_modules.0.push(AddedModule {
            address_range,
            base_address,
            image,
        });
        self.added_modules
            .0
            .sort_unstable_by_key(|m| m.address_range.start);
    }

    pub fn handle_mmap2(
        &mut self,
        e: Mmap2Event,
        dso_key: &DsoKey,
        build_id: Option<&[u8]>,
        image_cache: &mut ImageCache,
    ) {
        // println!(
        //     "raw2 ({}): 0x{:016x}-0x{:016x} (page offset 0x{:016x}) {:?}",
        //     self.pid,
        //     e.address,
        //     e.address + e.length,
        //     e.page_offset,
        //     std::str::from_utf8(&e.path)
        // );

        if e.protection & PROT_EXEC == 0 {
            // Ignore non-executable mappings.
            return;
        }

        let build_id = match e.file_id {
            Mmap2FileId::BuildId(build_id) => Some(build_id),
            Mmap2FileId::InodeAndVersion(_) => build_id.map(Vec::from),
        };

        const PROT_EXEC: u32 = 0b100;
        let start_addr = e.address;
        let end_addr = e.address + e.length;

        let path_str = std::str::from_utf8(&e.path).unwrap();
        let path = Path::new(path_str);
        let address_range = start_addr..end_addr;
        let (debug_id, base_address) = match add_module(
            &mut self.unwinder,
            path,
            e.page_offset,
            e.address,
            e.length,
            build_id.as_deref(),
        ) {
            Some(module_info) => module_info,
            None => return,
        };
        let image = image_cache.index_for_image(path, dso_key, Some(debug_id));
        println!(
            "0x{:016x}-0x{:016x} {:?} {:?}",
            start_addr,
            e.address + e.length,
            build_id.as_deref().map(CodeId::from_binary),
            path
        );
        self.added_modules.0.push(AddedModule {
            address_range,
            base_address,
            image,
        });
        self.added_modules
            .0
            .sort_unstable_by_key(|m| m.address_range.start);
    }

    pub fn handle_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: SampleEvent,
        cache: &mut U::Cache,
        kernel_modules: &AddedModules,
    ) -> Option<ProcessedSample> {
        let mut stack = Vec::new();

        if let Some(callchain) = e.callchain {
            let mut is_first_frame = true;
            for address in callchain {
                if address >= PERF_CONTEXT_MAX {
                    // Ignore synthetic addresses like 0xffffffffffffff80.
                    continue;
                }

                let lookup_address = if is_first_frame { address } else { address - 1 };
                is_first_frame = false;

                let stack_frame = match self.added_modules.map_address(lookup_address) {
                    Some((image, relative_lookup_address)) => {
                        StackFrame::InImage(StackFrameInImage {
                            image,
                            relative_lookup_address,
                        })
                    }
                    None => match kernel_modules.map_address(address) {
                        Some((image, relative_lookup_address)) => {
                            StackFrame::InImage(StackFrameInImage {
                                image,
                                relative_lookup_address,
                            })
                        }
                        None => StackFrame::Address(address),
                    },
                };
                stack.push(stack_frame);
            }
        }

        if let Some(regs) = e.regs {
            let ustack_bytes = e.stack.as_slice();
            let (pc, sp, regs) = C::convert_regs(&regs);
            let mut read_stack = |addr: u64| {
                let offset = addr.checked_sub(sp).ok_or(())?;
                let p_start = usize::try_from(offset).map_err(|_| ())?;
                let p_end = p_start.checked_add(8).ok_or(())?;
                if let Some(p) = ustack_bytes.get(p_start..p_end) {
                    let val = u64::from_le_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]]);
                    Ok(val)
                } else {
                    // eprintln!("Ran out of stack when trying to read at address 0x{:x}", addr);
                    Err(())
                }
            };
            let mut frames = self.unwinder.iter_frames(pc, regs, cache, &mut read_stack);
            if !self.added_modules.0.is_empty() {
                // eprintln!("trying to unwind now");
            }
            loop {
                let frame = match frames.next() {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(_) => {
                        stack.push(StackFrame::TruncatedStackMarker);
                        break;
                    }
                };
                let address = frame.address_for_lookup();
                let stack_frame = match self.added_modules.map_address(address) {
                    Some((image, relative_lookup_address)) => {
                        StackFrame::InImage(StackFrameInImage {
                            image,
                            relative_lookup_address,
                        })
                    }
                    None => StackFrame::Address(address),
                };
                stack.push(stack_frame);
                // eprintln!("got frame: {:?}", frame);
            }
        }

        Some(ProcessedSample {
            timestamp: e.timestamp.unwrap(),
            pid: e.pid.unwrap(),
            tid: e.tid.unwrap(),
            stack,
        })
    }
}

struct AddedModule {
    address_range: Range<u64>,
    base_address: u64,
    image: ImageCacheHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ImageCacheHandle(u32);

struct ImageCache {
    images: Vec<Image>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self { images: Vec::new() }
    }

    pub fn index_for_image(
        &mut self,
        path: &Path,
        dso_key: &DsoKey,
        debug_id: Option<DebugId>,
    ) -> ImageCacheHandle {
        match self
            .images
            .iter()
            .enumerate()
            .find(|(_, image)| &image.dso_key == dso_key)
        {
            Some((index, _)) => ImageCacheHandle(index as u32),
            None => {
                let index = self.images.len() as u32;
                self.images.push(Image {
                    path: path.to_owned(),
                    dso_key: dso_key.clone(),
                    debug_id,
                });
                ImageCacheHandle(index)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessedSample {
    pub timestamp: u64,
    pub pid: i32,
    pub tid: i32,
    pub stack: Vec<StackFrame>,
}

#[derive(Clone, Debug)]
pub enum StackFrame {
    InImage(StackFrameInImage),
    Address(u64),
    TruncatedStackMarker,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct StackFrameInImage {
    image: ImageCacheHandle,
    relative_lookup_address: u32,
}

struct Image {
    dso_key: DsoKey,
    path: PathBuf,
    debug_id: Option<DebugId>,
}

pub fn add_module<U>(
    unwinder: &mut U,
    objpath: &Path,
    mapping_start_file_offset: u64,
    mapping_start_avma: u64,
    mapping_size: u64,
    build_id: Option<&[u8]>,
) -> Option<(DebugId, u64)>
where
    U: Unwinder<Module = Module<Vec<u8>>>,
{
    let file = match std::fs::File::open(objpath) {
        Ok(file) => file,
        Err(_) => {
            let mut p = Path::new("/Users/mstange/code/linux-perf-data/fixtures/x86_64").to_owned();
            p.push(objpath.file_name().unwrap());
            match std::fs::File::open(&p) {
                Ok(file) => file,
                Err(_) => {
                    eprintln!("Could not open file {:?}", objpath);
                    return None;
                }
            }
        }
    };
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file).ok()? };

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
        if let Ok(Some(file_build_id)) = file.build_id() {
            if file_build_id != build_id {
                let file_build_id = CodeId::from_binary(file_build_id);
                let expected_build_id = CodeId::from_binary(build_id);
                eprintln!(
                    "File {:?} has non-matching build ID {} (expected {})",
                    objpath, file_build_id, expected_build_id
                );
                return None;
            }
        } else {
            eprintln!(
                "File {:?} does not contain a build ID, but we expected it to have one",
                objpath
            );
            return None;
        }
    }

    // eprintln!("segments: {:?}", file.segments());
    let mapping_end_file_offset = mapping_start_file_offset + mapping_size;
    let mapped_segment = file.segments().find(|segment| {
        let (segment_start_file_offset, segment_size) = segment.file_range();
        let segment_end_file_offset = segment_start_file_offset + segment_size;
        mapping_start_file_offset <= segment_start_file_offset
            && segment_end_file_offset <= mapping_end_file_offset
    })?;

    let (segment_start_file_offset, _segment_size) = mapped_segment.file_range();
    let segment_start_svma = mapped_segment.address();
    let segment_start_avma =
        mapping_start_avma + (segment_start_file_offset - mapping_start_file_offset);

    // Compute the AVMA that maps to SVMA zero. This is also called the "bias" of the
    // image. On ELF it is also the image load address.
    let base_svma = 0;
    let base_avma = segment_start_avma - segment_start_svma;

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
            framehop::ModuleUnwindData::EhFrameHdrAndEhFrame(eh_frame_hdr, eh_frame)
        }
        (Some(eh_frame), None) => framehop::ModuleUnwindData::EhFrame(eh_frame),
        (None, _) => framehop::ModuleUnwindData::None,
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

    let mapping_end_avma = mapping_start_avma + mapping_size;
    let module = framehop::Module::new(
        objpath.to_string_lossy().to_string(),
        mapping_start_avma..mapping_end_avma,
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

    let debug_id = debug_id_for_object(&file)?;
    Some((debug_id, base_avma))
}

struct FileContents(memmap2::Mmap);

impl std::ops::Deref for FileContents {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        &self.0
    }
}
#[derive(Debug, Clone)]
struct Helper {
    libs: Vec<HelperLib>,
    verbose: bool,
}

#[derive(Debug, Clone)]
struct HelperLib {
    debug_name: String,
    debug_id: DebugId,
    dso_key: DsoKey,
    handle: ImageCacheHandle,
    path: PathBuf,
}

impl Helper {
    pub fn with_libs(libs: Vec<HelperLib>, verbose: bool) -> Self {
        Helper { libs, verbose }
    }

    async fn open_file_impl(
        &self,
        location: FileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            FileLocation::Path(path) => {
                if self.verbose {
                    eprintln!("Opening file {:?}", path.to_string_lossy());
                }
                let file = File::open(&path)?;
                Ok(FileContents(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            FileLocation::Custom(_) => {
                panic!("unexpected")
            }
        }
    }
}

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContents;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        debug_id: &DebugId,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        if self.verbose {
            eprintln!(
                "Listing candidates for debug_name {} and debug ID {}",
                debug_name, debug_id
            );
        }
        let mut paths = vec![];

        // Look up (debug_name, debug_id) in the map.
        if let Some(lib) = self
            .libs
            .iter()
            .find(|lib| lib.debug_name == debug_name && &lib.debug_id == debug_id)
        {
            let fixtures_dir = PathBuf::from("/Users/mstange/code/linux-perf-data/fixtures/x86_64");

            if lib.dso_key == DsoKey::Kernel {
                let mut p = fixtures_dir.clone();
                p.push("kernel-symbols");
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(p)));
                let mut p = fixtures_dir.clone();
                p.push("vmlinux-5.4.0-109-generic");
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(p)));
            }

            if lib.dso_key == DsoKey::Vdso64 {
                let mut p = fixtures_dir.clone();
                p.push("vdso64-symbols");
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(p)));
                let mut p = fixtures_dir.clone();
                p.push("vdso.so");
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(p)));
            }

            let path = lib.path.clone();

            // Also consider .so.dbg files in the same directory.
            if debug_name.ends_with(".so") {
                let debug_debug_name = format!("{}.dbg", debug_name);
                if let Some(dir) = path.parent() {
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dir.join(debug_debug_name),
                    )));
                }
            }

            // Fall back to getting symbols from the binary itself.
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                path.clone(),
            )));

            // Also from the fixtures directory.
            let mut p = fixtures_dir;
            p.push(path.file_name().unwrap());
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(p)));
        }

        Ok(paths)
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.open_file_impl(location.clone()))
    }
}
