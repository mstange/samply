mod perf_event;
pub mod perf_event_raw;
mod perf_file;
mod raw_data;
mod reader;
mod unaligned;
mod utils;

use framehop::aarch64::UnwindRegsAarch64;
use framehop::x86_64::UnwindRegsX86_64;
use framehop::{Module, ModuleSectionAddressRanges, TextByteData, Unwinder};
use object::{Object, ObjectSection, ObjectSegment};
use perf_event::{Event, Mmap2Event, Regs, SampleEvent};
use perf_event_raw::{
    PERF_REG_ARM64_LR, PERF_REG_ARM64_PC, PERF_REG_ARM64_SP, PERF_REG_ARM64_X29, PERF_REG_X86_BP,
    PERF_REG_X86_IP, PERF_REG_X86_SP,
};
pub use perf_file::PerfFile;
use std::collections::HashSet;
use std::collections::{hash_map::Entry, HashMap};
use std::path::PathBuf;
use std::{fs::File, io::Read, ops::Range, path::Path, sync::Arc};

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.len() < 1 {
        eprintln!("Usage: {} <path>", std::env::args().next().unwrap());
        std::process::exit(1);
    }
    let path = args.next().unwrap();

    let mut data = Vec::new();
    let mut file = File::open(path).unwrap();
    file.read_to_end(&mut data).unwrap();
    let file = PerfFile::parse(&data).expect("Parsing failed");

    if let Some(hostname) = file.hostname().unwrap() {
        eprintln!("Hostname: {}", hostname);
    }

    if let Some(os_release) = file.os_release().unwrap() {
        eprintln!("OS release: {}", os_release);
    }

    if let Some(perf_version) = file.perf_version().unwrap() {
        eprintln!("Perf version: {}", perf_version);
    }

    if let Some(arch) = file.arch().unwrap() {
        eprintln!("Arch: {}", arch);
    }

    if let Some(nr_cpus) = file.nr_cpus().unwrap() {
        eprintln!(
            "CPUs: {} online ({} available)",
            nr_cpus.nr_cpus_online.get(file.endian()),
            nr_cpus.nr_cpus_available.get(file.endian())
        );
    }

    if let Some(build_ids) = file.build_ids().unwrap() {
        eprintln!("Build IDs:");
        for (build_id_ev, filename) in build_ids {
            eprintln!(
                " - PID {}, build ID {}, filename {}",
                build_id_ev.pid.get(file.endian()),
                build_id_ev
                    .build_id
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<String>>()
                    .join(""),
                std::str::from_utf8(filename).unwrap()
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
        // eprintln!("regs: 0x{:x}, {:?}", ip, regs);
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
    let mut processes: HashMap<u32, Process<U>> = HashMap::new();
    let mut image_cache = ImageCache::new();
    let mut processed_samples = Vec::new();
    let mut all_image_stack_frames = HashSet::new();

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
                    process.handle_sample::<C>(e, cache, &mut image_cache)
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
                eprintln!("Comm: {:?}", e);
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
            Event::Mmap2(e) => {
                let process = processes
                    .entry(e.pid)
                    .or_insert_with(|| Process::new(e.pid, format!("<{}>", e.pid).into_bytes()));
                process.handle_mmap(e);
            }
            Event::Lost(_) => {}
            Event::Throttle(_) => {}
            Event::Unthrottle(_) => {}
            Event::ContextSwitch(_) => {}
            Event::Raw(_) => {}
        }
    }
    eprintln!(
        "Have {} events, converted into {} processed samples.",
        count,
        processed_samples.len()
    );
    eprintln!("{:#?}", processed_samples);
}

struct Process<U> {
    #[allow(unused)]
    pid: u32,
    name: Vec<u8>,
    unwinder: U,
    pending_modules: Vec<PendingModule>,
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
    pub fn new(pid: u32, name: Vec<u8>) -> Self {
        Self {
            pid,
            name,
            unwinder: U::default(),
            pending_modules: Vec::new(),
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
    pub fn handle_mmap(&mut self, e: Mmap2Event) {
        if e.inode == 0 {
            return;
        }

        let start_addr = e.address;
        let end_addr = e.address + e.length;
        match self
            .pending_modules
            .iter_mut()
            .find(|m| m.path == e.filename)
        {
            Some(m) => {
                m.min_start = m.min_start.min(start_addr);
                m.max_end = m.max_end.min(end_addr);
            }
            None => {
                self.pending_modules.push(PendingModule {
                    path: e.filename,
                    min_start: start_addr,
                    max_end: end_addr,
                });
            }
        }
    }

    fn flush_pending_mappings(&mut self, image_cache: &mut ImageCache) {
        for mapping in self.pending_modules.drain(..) {
            let path_str = std::str::from_utf8(&mapping.path).unwrap();
            let path = Path::new(path_str);
            let base_address = mapping.min_start;
            let address_range = mapping.min_start..mapping.max_end;
            add_module(
                &mut self.unwinder,
                path,
                base_address,
                address_range.clone(),
            );
            let image = image_cache.index_for_image(path);
            self.added_modules.0.push(AddedModule {
                address_range,
                base_address,
                image,
            });
        }
    }

    pub fn handle_sample<C: ConvertRegs<UnwindRegs = U::UnwindRegs>>(
        &mut self,
        e: SampleEvent,
        cache: &mut U::Cache,
        image_cache: &mut ImageCache,
    ) -> Option<ProcessedSample> {
        self.flush_pending_mappings(image_cache);
        let regs = e.regs.unwrap();
        let stack = e.stack.as_slice();
        let (pc, sp, regs) = C::convert_regs(&regs);
        let mut read_stack = |addr: u64| {
            let offset = addr.checked_sub(sp).ok_or(())?;
            let p_start = usize::try_from(offset).map_err(|_| ())?;
            let p_end = p_start.checked_add(8).ok_or(())?;
            if let Some(p) = stack.get(p_start..p_end) {
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
        let mut stack = Vec::new();
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
                Some((image, relative_lookup_address)) => StackFrame::InImage(StackFrameInImage {
                    image,
                    relative_lookup_address,
                }),
                None => StackFrame::Address(address),
            };
            stack.push(stack_frame);
            // eprintln!("got frame: {:?}", frame);
        }

        Some(ProcessedSample {
            timestamp: e.timestamp.unwrap(),
            pid: e.pid.unwrap(),
            tid: e.tid.unwrap(),
            stack,
        })
    }
}

struct PendingModule {
    path: Vec<u8>,
    min_start: u64,
    max_end: u64,
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

    pub fn index_for_image(&mut self, path: &Path) -> ImageCacheHandle {
        match self
            .images
            .iter()
            .enumerate()
            .find(|(_, image)| image.path == path)
        {
            Some((index, _)) => ImageCacheHandle(index as u32),
            None => {
                let index = self.images.len() as u32;
                self.images.push(Image {
                    path: path.to_owned(),
                });
                ImageCacheHandle(index)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessedSample {
    pub timestamp: u64,
    pub pid: u32,
    pub tid: u32,
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
    path: PathBuf,
}

pub fn add_module<U>(
    unwinder: &mut U,
    objpath: &Path,
    base_address: u64,
    image_address_range: Range<u64>,
) where
    U: Unwinder<Module = Module<Vec<u8>>>,
{
    let mut buf = Vec::new();
    let mut file = match std::fs::File::open(objpath) {
        Ok(file) => file,
        Err(_) => {
            let mut p = Path::new("/Users/mstange/code/linux-perf-data/fixtures").to_owned();
            p.push(objpath.file_name().unwrap());
            match std::fs::File::open(&p) {
                Ok(file) => file,
                Err(_) => {
                    eprintln!("Could not open file {:?}", objpath);
                    return;
                }
            }
        }
    };
    file.read_to_end(&mut buf).unwrap();

    fn section_data<'a>(section: &impl ObjectSection<'a>) -> Option<Vec<u8>> {
        section.data().ok().map(|data| data.to_owned())
    }

    let file = match object::File::parse(&buf[..]) {
        Ok(file) => file,
        Err(_) => {
            eprintln!("file {:?} had unrecognized format", objpath);
            return;
        }
    };

    let text = file
        .section_by_name(".text");
    let text_env = file.section_by_name("text_env");
    let eh_frame = file
        .section_by_name(".eh_frame");
    let got = file
        .section_by_name(".got");
    let eh_frame_hdr = file.section_by_name(".eh_frame_hdr");

    let unwind_data = match (
        eh_frame.as_ref().and_then(section_data),
        eh_frame_hdr.as_ref().and_then(section_data),
    ) {
        (Some(eh_frame), Some(eh_frame_hdr)) => {
            framehop::ModuleUnwindData::EhFrameHdrAndEhFrame(
                Arc::new(eh_frame_hdr),
                Arc::new(eh_frame),
            )
        }
        (Some(eh_frame), None) => framehop::ModuleUnwindData::EhFrame(Arc::new(eh_frame)),
        (None, _) => framehop::ModuleUnwindData::None,
    };

    let text_data = if let Some(text_segment) = file
        .segments()
        .find(|segment| segment.name_bytes() == Ok(Some(b"__TEXT")))
    {
        let (start, size) = text_segment.file_range();
        let address_range = base_address + start..base_address + start + size;
        text_segment
            .data()
            .ok()
            .map(|data| TextByteData::new(data.to_owned(), address_range))
    } else if let Some(text_section) = &text {
        if let Some((start, size)) = text_section.file_range() {
            let address_range = base_address + start..base_address + start + size;
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

    fn address_range<'a>(
        section: &Option<impl ObjectSection<'a>>,
        base_address: u64,
    ) -> Option<Range<u64>> {
        section
            .as_ref()
            .and_then(|section| section.file_range())
            .map(|(start, size)| base_address + start..base_address + start + size)
    }

    let module = framehop::Module::new(
        objpath.to_string_lossy().to_string(),
        image_address_range,
        base_address,
        ModuleSectionAddressRanges {
            text: address_range(&text, base_address),
            text_env: address_range(&text_env, base_address),
            stubs: None,
            stub_helper: None,
            eh_frame: address_range(&eh_frame, base_address),
            eh_frame_hdr: address_range(&eh_frame_hdr, base_address),
            got: address_range(&got, base_address),
        },
        unwind_data,
        text_data,
    );
    unwinder.add_module(module);
}
