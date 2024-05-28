use std::borrow::Cow;
use std::marker::PhantomData;
use std::slice;
use std::sync::{Arc, Mutex};

use addr2line::{LookupResult, SplitDwarfLoad};
use debugid::DebugId;
use gimli::{EndianSlice, RunTimeEndian};
use object::{
    ObjectMap, ObjectSection, ObjectSegment, SectionFlags, SectionIndex, SectionKind, SymbolKind,
};
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::dwarf::convert_frames;
use crate::path_mapper::PathMapper;
use crate::shared::{
    relative_address_base, ExternalFileAddressInFileRef, ExternalFileAddressRef, ExternalFileRef,
    FramesLookupResult, LookupAddress, SymbolInfo,
};
use crate::symbol_map::{
    GetInnerSymbolMap, GetInnerSymbolMapWithLookupFramesExt, SymbolMapTrait,
    SymbolMapTraitWithExternalFileSupport,
};
use crate::{demangle, Error, ExternalFileSymbolMap, FileContents, SyncAddressInfo};

enum FullSymbolListEntry<'a, Symbol> {
    /// A synthesized symbol for a function start address that's known
    /// from some other information (not from the symbol table).
    Synthesized,
    /// A synthesized symbol for the entry point of the object.
    SynthesizedEntryPoint,
    Symbol(Symbol),
    Export(object::Export<'a>),
    EndAddress,
}

impl<'a, Symbol: object::ObjectSymbol<'a>> std::fmt::Debug for FullSymbolListEntry<'a, Symbol> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Synthesized => write!(f, "Synthesized"),
            Self::SynthesizedEntryPoint => write!(f, "SynthesizedEntryPoint"),
            Self::Symbol(arg0) => f
                .debug_tuple("Symbol")
                .field(&arg0.name().unwrap())
                .finish(),
            Self::Export(arg0) => f
                .debug_tuple("Export")
                .field(&std::str::from_utf8(arg0.name()).unwrap())
                .finish(),
            Self::EndAddress => write!(f, "EndAddress"),
        }
    }
}

impl<'a, Symbol: object::ObjectSymbol<'a>> FullSymbolListEntry<'a, Symbol> {
    fn name(&self, addr: u32) -> Option<Cow<'a, str>> {
        let name = match self {
            FullSymbolListEntry::EndAddress => return None,
            FullSymbolListEntry::Synthesized => format!("fun_{addr:x}").into(),
            FullSymbolListEntry::SynthesizedEntryPoint => "EntryPoint".into(),
            FullSymbolListEntry::Symbol(symbol) => {
                String::from_utf8_lossy(symbol.name_bytes().ok()?)
            }
            FullSymbolListEntry::Export(export) => String::from_utf8_lossy(export.name()),
        };
        Some(name)
    }

    fn counts_as_proper_symbol(&self) -> bool {
        match self {
            FullSymbolListEntry::Symbol(_) | FullSymbolListEntry::Export(_) => true,
            FullSymbolListEntry::EndAddress
            | FullSymbolListEntry::Synthesized
            | FullSymbolListEntry::SynthesizedEntryPoint => false,
        }
    }
}

struct SymbolList<'a, Symbol> {
    entries: Vec<(u32, FullSymbolListEntry<'a, Symbol>)>,
}

impl<'a, Symbol: object::ObjectSymbol<'a> + 'a> SymbolList<'a, Symbol> {
    pub fn new<'file, O>(
        object_file: &'file O,
        base_address: u64,
        function_start_addresses: Option<&[u32]>,
        function_end_addresses: Option<&[u32]>,
    ) -> Self
    where
        'a: 'file,
        O: object::Object<'a, Symbol<'file> = Symbol>,
    {
        let mut entries: Vec<_> = Vec::new();

        // Compute the executable sections upfront. This will be used to filter out uninteresting symbols.
        let executable_sections: Vec<SectionIndex> = object_file
            .sections()
            .filter_map(|section| match (section.kind(), section.flags()) {
                // Match executable sections.
                (SectionKind::Text, _) => Some(section.index()),

                // Match sections in debug files which correspond to executable sections in the original binary.
                // "SectionKind::EmptyButUsedToBeText"
                (SectionKind::UninitializedData, SectionFlags::Elf { sh_flags })
                    if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0 =>
                {
                    Some(section.index())
                }

                _ => None,
            })
            .collect();

        // Build a list of symbol start and end entries. We add entries in the order "best to worst".

        // 1. Normal symbols
        // 2. Dynamic symbols (only used by ELF files, I think)
        entries.extend(
            object_file
                .symbols()
                .chain(object_file.dynamic_symbols())
                .filter(|symbol| {
                    // Filter out symbols with no address.
                    if symbol.address() == 0 {
                        return false;
                    }

                    // Filter out non-Text symbols which don't have a symbol size.
                    match symbol.kind() {
                        SymbolKind::Text => {
                            // Keep. This is a regular function symbol. On mach-O these don't have sizes.
                        }
                        SymbolKind::Label if symbol.size() != 0 => {
                            // Keep. This catches some useful kernel symbols, e.g. asm_exc_page_fault,
                            // which is a NOTYPE symbol (= SymbolKind::Label).
                            //
                            // We require a non-zero symbol size in this case, in order to filter out some
                            // bad symbols in the middle of functions. For example, the android32-local/libmozglue.so
                            // fixture has a NOTYPE symbol with zero size at 0x9850f.
                        }
                        _ => return false, // Cull.
                    }

                    // Filter out symbols from non-executable sections.
                    match symbol.section_index() {
                        Some(section_index) => executable_sections.contains(&section_index),
                        _ => false,
                    }
                })
                .filter_map(|symbol| {
                    Some((
                        u32::try_from(symbol.address().checked_sub(base_address)?).ok()?,
                        FullSymbolListEntry::Symbol(symbol),
                    ))
                }),
        );

        // 3. Exports (only used by exe / dll objects)
        if let Ok(exports) = object_file.exports() {
            for export in exports {
                entries.push((
                    (export.address() - base_address) as u32,
                    FullSymbolListEntry::Export(export),
                ));
            }
        }

        // 4. Placeholder symbols based on function start addresses
        if let Some(function_start_addresses) = function_start_addresses {
            // Use function start addresses with synthesized symbols of the form fun_abcdef
            // as the ultimate fallback.
            // These synhesized symbols make it so that, for libraries which only contain symbols
            // for a small subset of their functions, we will show placeholder function names
            // rather than plain incorrect function names.
            entries.extend(
                function_start_addresses
                    .iter()
                    .map(|address| (*address, FullSymbolListEntry::Synthesized)),
            );
        }

        // 5. A placeholder symbol for the entry point.
        if let Some(entry_point) = object_file.entry().checked_sub(base_address) {
            entries.push((
                entry_point as u32,
                FullSymbolListEntry::SynthesizedEntryPoint,
            ));
        }

        // 6. End addresses from text section ends
        // These entries serve to "terminate" the last function of each section,
        // so that addresses in the following section are not considered
        // to be part of the last function of that previous section.
        entries.extend(
            object_file
                .sections()
                .filter(|s| s.kind() == SectionKind::Text)
                .filter_map(|section| {
                    let vma_end_address = section.address().checked_add(section.size())?;
                    let end_address = vma_end_address.checked_sub(base_address)?;
                    let end_address = u32::try_from(end_address).ok()?;
                    Some((end_address, FullSymbolListEntry::EndAddress))
                }),
        );

        // 7. End addresses for sized symbols
        // These addresses serve to "terminate" functions symbols.
        entries.extend(
            object_file
                .symbols()
                .filter(|symbol| {
                    symbol.kind() == SymbolKind::Text && symbol.address() != 0 && symbol.size() != 0
                })
                .filter_map(|symbol| {
                    Some((
                        u32::try_from(
                            symbol
                                .address()
                                .checked_add(symbol.size())?
                                .checked_sub(base_address)?,
                        )
                        .ok()?,
                        FullSymbolListEntry::EndAddress,
                    ))
                }),
        );

        // 8. End addresses for known functions ends
        // These addresses serve to "terminate" functions from function_start_addresses.
        // They come from .eh_frame or .pdata info, which has the function size.
        if let Some(function_end_addresses) = function_end_addresses {
            entries.extend(
                function_end_addresses
                    .iter()
                    .map(|address| (*address, FullSymbolListEntry::EndAddress)),
            );
        }

        // Done.
        // Now that all entries are added, sort and de-duplicate so that we only
        // have one entry per address.
        // If multiple entries for the same address are present, only the first
        // entry for that address is kept. (That's also why we use a stable sort
        // here.)
        // We have added entries in the order best to worst, so we keep the "best"
        // symbol for each address.
        entries.sort_by_key(|(address, _)| *address);
        entries.dedup_by_key(|(address, _)| *address);

        Self { entries }
    }

    pub fn lookup_relative_address(&self, address: u32) -> Option<(u32, u32, Cow<'a, str>)> {
        let index = match self
            .entries
            .binary_search_by_key(&address, |&(addr, _)| addr)
        {
            Err(0) => return None,
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let (start_addr, entry) = &self.entries[index];
        let (end_addr, _next_entry) = self.entries.get(index + 1)?;
        let name = match entry {
            FullSymbolListEntry::EndAddress => {
                // If the found entry is an EndAddress entry, this means that `address` falls
                // in the dead space between known functions, and we consider it to be not found.
                return None;
            }
            _ => entry.name(*start_addr)?,
        };
        Some((*start_addr, *end_addr, name))
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
}

struct SvmaFileRanges(Vec<SvmaFileRange>);

impl SvmaFileRanges {
    pub fn from_object<'data, O: object::Object<'data>>(object_file: &O) -> Self {
        let mut svma_file_ranges: Vec<SvmaFileRange> = object_file
            .segments()
            .map(SvmaFileRange::from_segment)
            .collect();

        if svma_file_ranges.is_empty() {
            // If no segment is found, fall back to using section information.
            svma_file_ranges = object_file
                .sections()
                .filter_map(SvmaFileRange::from_section)
                .collect();
        }

        Self(svma_file_ranges)
    }

    fn file_offset_to_svma(&self, offset: u64) -> Option<u64> {
        for svma_file_range in &self.0 {
            if svma_file_range.file_offset <= offset
                && offset < svma_file_range.file_offset + svma_file_range.size
            {
                let offset_from_range_start = offset - svma_file_range.file_offset;
                let svma = svma_file_range.svma.checked_add(offset_from_range_start)?;
                return Some(svma);
            }
        }
        None
    }
}

impl std::fmt::Debug for SvmaFileRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvmaFileRange")
            .field("svma", &format!("{:#x}", &self.svma))
            .field("file_offset", &format!("{:#x}", &self.file_offset))
            .field("size", &format!("{:#x}", &self.size))
            .finish()
    }
}

pub struct ObjectSymbolMapInner<'a, Symbol, FC: FileContents + 'static, DDM> {
    list: SymbolList<'a, Symbol>,
    debug_id: DebugId,
    path_mapper: Mutex<PathMapper<()>>,
    object_map: ObjectMap<'a>,
    context: Option<Mutex<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>>,
    dwp_package:
        Option<addr2line::gimli::DwarfPackage<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    svma_file_ranges: SvmaFileRanges,
    image_base_address: u64,
    dwo_dwarf_maker: &'a DDM,
    cached_external_file: Mutex<Option<ExternalFileSymbolMap<FC>>>,
    _phantom: PhantomData<FC>,
}

impl<'a, Symbol, FC, DDM> ObjectSymbolMapInner<'a, Symbol, FC, DDM>
where
    Symbol: object::ObjectSymbol<'a> + 'a,
    FC: FileContents + 'static,
    DDM: DwoDwarfMaker<FC>,
{
    fn frames_lookup_for_object_map_references(&self, svma: u64) -> Option<FramesLookupResult> {
        let entry = self.object_map.get(svma)?;
        let object_map_file = entry.object(&self.object_map);
        let file_path = std::str::from_utf8(object_map_file.path()).ok()?;
        let offset_from_symbol = (svma - entry.address()) as u32;
        let symbol_name = entry.name().to_owned();
        let address_in_file = match object_map_file.member() {
            Some(member) => {
                // This is an "archive" reference of the form
                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../../js/src/build/libjs_static.a(Unified_cpp_js_src13.o)"
                ExternalFileAddressInFileRef::MachoOsoArchive {
                    name_in_archive: std::str::from_utf8(member).ok()?.to_owned(),
                    symbol_name,
                    offset_from_symbol,
                }
            }
            None => {
                // This is a reference to a regular object file. Example:
                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../components/sessionstore/Unified_cpp_sessionstore0.o"
                ExternalFileAddressInFileRef::MachoOsoObject {
                    symbol_name,
                    offset_from_symbol,
                }
            }
        };
        Some(FramesLookupResult::External(ExternalFileAddressRef {
            file_ref: ExternalFileRef::MachoExternalObject {
                file_path: file_path.to_owned(),
            },
            address_in_file,
        }))
    }

    fn try_lookup_external_impl(
        &self,
        external: &ExternalFileAddressRef,
        mut request: ExternalLookupRequest<FC>,
    ) -> Option<FramesLookupResult> {
        match &external.file_ref {
            ExternalFileRef::MachoExternalObject { file_path } => {
                {
                    let cached_external_file = self.cached_external_file.lock().unwrap();
                    match &*cached_external_file {
                        Some(external_file) if external_file.file_path() == file_path => {
                            return external_file
                                .lookup(&external.address_in_file)
                                .map(FramesLookupResult::Available);
                        }
                        _ => {}
                    }
                }
                let file_contents = match request {
                    ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed => {
                        return Some(FramesLookupResult::External(external.clone()))
                    }
                    ExternalLookupRequest::UseThisMaybeAndReplyOrTellMeWhatElseYouNeed(
                        maybe_file_contents,
                    ) => maybe_file_contents?,
                };
                let external_file = ExternalFileSymbolMap::new(file_path, file_contents).ok()?;
                let lookup_result = external_file
                    .lookup(&external.address_in_file)
                    .map(FramesLookupResult::Available);

                *self.cached_external_file.lock().unwrap() = Some(external_file);

                lookup_result
            }
            ExternalFileRef::ElfExternalDwo { .. } => {
                let ctx = self.context.as_ref()?;
                let ExternalFileAddressInFileRef::ElfDwo { svma, .. } = &external.address_in_file
                else {
                    return None;
                };
                let ctx = ctx.lock().unwrap();
                let mut lookup_result = ctx.find_frames(*svma);
                // We use a loop here so that we can retry the lookup with a "continue"
                // after we've fed the DWO data into the addr2line context.
                loop {
                    break match lookup_result {
                        LookupResult::Load { load, continuation } => {
                            if !external.matches_split_dwarf_load(&load) {
                                request = ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed;
                            }
                            let file_contents = match request {
                                ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed => {
                                    return Some(FramesLookupResult::External(
                                        ExternalFileAddressRef::with_split_dwarf_load(&load, *svma),
                                    ))
                                }
                                ExternalLookupRequest::UseThisMaybeAndReplyOrTellMeWhatElseYouNeed(file_contents) => file_contents,
                            };
                            let maybe_dwarf = file_contents
                                .and_then(|file_contents| {
                                    self.dwo_dwarf_maker
                                        .add_dwo_and_make_dwarf(file_contents)
                                        .ok()
                                        .flatten()
                                })
                                .map(|mut dwo_dwarf| {
                                    dwo_dwarf.make_dwo(&*load.parent);
                                    Arc::new(dwo_dwarf)
                                });
                            use addr2line::LookupContinuation;
                            request = ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed;
                            lookup_result = continuation.resume(maybe_dwarf);
                            continue;
                        }
                        LookupResult::Output(Ok(frame_iter)) => {
                            let mut path_mapper = self.path_mapper.lock().unwrap();
                            convert_frames(frame_iter, &mut path_mapper)
                                .map(FramesLookupResult::Available)
                        }
                        LookupResult::Output(Err(_)) => None,
                    };
                }
            }
        }
    }
}

impl<'a, Symbol, FC, DDM> SymbolMapTrait for ObjectSymbolMapInner<'a, Symbol, FC, DDM>
where
    Symbol: object::ObjectSymbol<'a> + 'a,
    FC: FileContents + 'static,
    DDM: DwoDwarfMaker<FC>,
{
    fn debug_id(&self) -> DebugId {
        self.debug_id
    }

    fn symbol_count(&self) -> usize {
        let iter = self.list.entries.iter();
        iter.filter(|&(_, entry)| entry.counts_as_proper_symbol())
            .count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        Box::new(SymbolMapIter {
            inner: self.list.entries.iter(),
        })
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        let (svma, relative_address) = match address {
            LookupAddress::Relative(relative_address) => (
                self.image_base_address
                    .checked_add(u64::from(relative_address))?,
                relative_address,
            ),
            LookupAddress::Svma(svma) => (
                svma,
                u32::try_from(svma.checked_sub(self.image_base_address)?).ok()?,
            ),
            LookupAddress::FileOffset(offset) => {
                let svma = self.svma_file_ranges.file_offset_to_svma(offset)?;
                (
                    svma,
                    u32::try_from(svma.checked_sub(self.image_base_address)?).ok()?,
                )
            }
        };
        let (start_addr, end_addr, name) = self.list.lookup_relative_address(relative_address)?;
        let function_size = end_addr - start_addr;
        let name = demangle::demangle_any(&name);
        let symbol = SymbolInfo {
            address: start_addr,
            size: Some(function_size),
            name,
        };

        let mut frames = None;
        if let Some(context) = self.context.as_ref() {
            let context = context.lock().unwrap();
            let mut lookup_result = context.find_frames(svma);

            // We use a loop here so that we can retry the lookup with a "continue"
            // after we've fed the DWP data into the addr2line context.
            frames = loop {
                break match lookup_result {
                    LookupResult::Load { load, continuation } => {
                        if let Some(dwp) = self.dwp_package.as_ref() {
                            if let Ok(maybe_cu) = dwp.find_cu(load.dwo_id, &*load.parent) {
                                use addr2line::LookupContinuation;
                                lookup_result = continuation.resume(maybe_cu.map(Arc::new));
                                continue;
                            }
                        }
                        Some(FramesLookupResult::External(
                            ExternalFileAddressRef::with_split_dwarf_load(&load, svma),
                        ))
                    }
                    LookupResult::Output(Ok(frame_iter)) => {
                        let mut path_mapper = self.path_mapper.lock().unwrap();
                        convert_frames(frame_iter, &mut path_mapper)
                            .map(FramesLookupResult::Available)
                    }
                    LookupResult::Output(Err(_)) => {
                        drop(lookup_result);
                        drop(context);
                        self.frames_lookup_for_object_map_references(svma)
                    }
                };
            }
        }
        if frames.is_none() {
            frames = self.frames_lookup_for_object_map_references(svma);
        }
        Some(SyncAddressInfo { symbol, frames })
    }
}

pub struct SymbolMapIter<'data, 'map, Symbol: object::ObjectSymbol<'data>> {
    inner: slice::Iter<'map, (u32, FullSymbolListEntry<'data, Symbol>)>,
}

impl<'data, 'map, Symbol: object::ObjectSymbol<'data>> Iterator
    for SymbolMapIter<'data, 'map, Symbol>
{
    type Item = (u32, Cow<'map, str>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (address, entry) = self.inner.next()?;
            let Some(name) = entry.name(*address) else {
                continue;
            };
            return Some((*address, name));
        }
    }
}

pub trait ObjectSymbolMapOuter<FC> {
    fn make_symbol_map_inner(&self) -> Result<ObjectSymbolMapInnerWrapper<'_, FC>, Error>;
}

pub struct ObjectSymbolMap<FC: 'static, OSMO: ObjectSymbolMapOuter<FC>>(
    Yoke<ObjectSymbolMapInnerWrapper<'static, FC>, Box<OSMO>>,
);

impl<FC, OSMO: ObjectSymbolMapOuter<FC> + 'static> ObjectSymbolMap<FC, OSMO> {
    pub fn new(outer: OSMO) -> Result<Self, Error> {
        let outer_and_inner = Yoke::<ObjectSymbolMapInnerWrapper<FC>, _>::try_attach_to_cart(
            Box::new(outer),
            |outer| outer.make_symbol_map_inner(),
        )?;
        Ok(ObjectSymbolMap(outer_and_inner))
    }
}

impl<FC: FileContents + 'static, OSMO: ObjectSymbolMapOuter<FC>> GetInnerSymbolMap
    for ObjectSymbolMap<FC, OSMO>
{
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a) {
        self.0.get().0.as_ref().get_as_symbol_map()
    }
}

impl<FC: FileContents + 'static, OSMO: ObjectSymbolMapOuter<FC>>
    GetInnerSymbolMapWithLookupFramesExt<FC> for ObjectSymbolMap<FC, OSMO>
{
    fn get_inner_symbol_map<'a>(
        &'a self,
    ) -> &'a (dyn SymbolMapTraitWithExternalFileSupport<FC> + Send + Sync + 'a) {
        self.0.get().0.as_ref()
    }
}

#[derive(Yokeable)]
pub struct ObjectSymbolMapInnerWrapper<'data, FC>(
    pub Box<dyn SymbolMapTraitWithExternalFileSupport<FC> + Send + Sync + 'data>,
);

impl<'a, FC: FileContents + 'static> ObjectSymbolMapInnerWrapper<'a, FC> {
    pub fn new<'file, O, Symbol, DDM>(
        object_file: &'file O,
        addr2line_context: Option<addr2line::Context<EndianSlice<'a, RunTimeEndian>>>,
        dwp_package: Option<addr2line::gimli::DwarfPackage<EndianSlice<'a, RunTimeEndian>>>,
        debug_id: DebugId,
        function_start_addresses: Option<&[u32]>,
        function_end_addresses: Option<&[u32]>,
        dwo_dwarf_maker: &'a DDM,
    ) -> Self
    where
        'a: 'file,
        O: object::Object<'a, Symbol<'file> = Symbol>,
        Symbol: object::ObjectSymbol<'a> + Send + Sync + 'a,
        DDM: DwoDwarfMaker<FC> + Sync,
    {
        let base_address = relative_address_base(object_file);
        let list = SymbolList::new(
            object_file,
            base_address,
            function_start_addresses,
            function_end_addresses,
        );

        let inner = ObjectSymbolMapInner {
            list,
            debug_id,
            path_mapper: Mutex::new(PathMapper::new()),
            object_map: object_file.object_map(),
            context: addr2line_context.map(Mutex::new),
            dwp_package,
            image_base_address: base_address,
            svma_file_ranges: SvmaFileRanges::from_object(object_file),
            dwo_dwarf_maker,
            cached_external_file: Mutex::new(None),
            _phantom: PhantomData,
        };
        Self(Box::new(inner))
    }
}

enum ExternalLookupRequest<FC> {
    ReplyIfYouHaveOrTellMeWhatYouNeed,
    UseThisMaybeAndReplyOrTellMeWhatElseYouNeed(Option<FC>),
}

type Dwarf<'a> =
    addr2line::gimli::Dwarf<addr2line::gimli::EndianSlice<'a, addr2line::gimli::RunTimeEndian>>;

pub trait DwoDwarfMaker<FC> {
    fn add_dwo_and_make_dwarf(&self, file_contents: FC) -> Result<Option<Dwarf<'_>>, Error>;
}

impl<FC> DwoDwarfMaker<FC> for () {
    fn add_dwo_and_make_dwarf(&self, _file_contents: FC) -> Result<Option<Dwarf<'_>>, Error> {
        Ok(None)
    }
}

impl<'a, Symbol, FC, DDM> SymbolMapTraitWithExternalFileSupport<FC>
    for ObjectSymbolMapInner<'a, Symbol, FC, DDM>
where
    Symbol: object::ObjectSymbol<'a> + 'a,
    FC: FileContents + 'static,
    DDM: DwoDwarfMaker<FC>,
{
    fn get_as_symbol_map(&self) -> &dyn SymbolMapTrait {
        self
    }

    fn try_lookup_external(&self, external: &ExternalFileAddressRef) -> Option<FramesLookupResult> {
        self.try_lookup_external_impl(
            external,
            ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed,
        )
    }

    fn try_lookup_external_with_file_contents(
        &self,
        external: &ExternalFileAddressRef,
        file_contents: Option<FC>,
    ) -> Option<FramesLookupResult> {
        self.try_lookup_external_impl(
            external,
            ExternalLookupRequest::UseThisMaybeAndReplyOrTellMeWhatElseYouNeed(file_contents),
        )
    }
}

impl ExternalFileAddressRef {
    fn with_split_dwarf_load(load: &SplitDwarfLoad<EndianSlice<RunTimeEndian>>, svma: u64) -> Self {
        let comp_dir = String::from_utf8_lossy(load.comp_dir.unwrap().slice()).to_string();
        let path = String::from_utf8_lossy(load.path.unwrap().slice()).to_string();
        let dwo_id = load.dwo_id.0;
        ExternalFileAddressRef {
            file_ref: ExternalFileRef::ElfExternalDwo { comp_dir, path },
            address_in_file: ExternalFileAddressInFileRef::ElfDwo { dwo_id, svma },
        }
    }

    fn matches_split_dwarf_load(&self, load: &SplitDwarfLoad<EndianSlice<RunTimeEndian>>) -> bool {
        match (&self.file_ref, &self.address_in_file) {
            (
                ExternalFileRef::ElfExternalDwo { comp_dir, path },
                ExternalFileAddressInFileRef::ElfDwo { dwo_id, .. },
            ) => {
                Some(comp_dir.as_bytes()) == load.comp_dir.map(|r| r.slice())
                    && Some(path.as_bytes()) == load.path.map(|r| r.slice())
                    && *dwo_id == load.dwo_id.0
            }
            _ => false,
        }
    }
}
