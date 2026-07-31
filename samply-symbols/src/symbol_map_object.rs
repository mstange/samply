use std::borrow::Cow;
use std::marker::PhantomData;
use std::slice;
use std::sync::{Arc, Mutex};

use addr2line::{LookupResult, SplitDwarfLoad};
use debugid::DebugId;
use gimli::{EndianSlice, RunTimeEndian};
use object::{
    ObjectMap, ObjectSection, ObjectSegment, ObjectSymbolTable, SectionFlags, SectionIndex,
    SectionKind, SymbolKind,
};
use samply_object::relative_address_base;
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::dwarf::convert_frames;
use crate::generation::SymbolMapGeneration;
use crate::shared::{
    ExternalFileAddressInFileRef, ExternalFileAddressRef, ExternalFileRef, FramesLookupResult,
    LookupAddress, SymbolInfo,
};
use crate::symbol_map::{
    GetInnerSymbolMap, GetInnerSymbolMapWithLookupFramesExt, SymbolMapTrait,
    SymbolMapTraitWithExternalFileSupport,
};
use crate::{
    demangle, Error, ExternalFileSymbolMap, FileContents, FrameDebugInfo, FunctionNameHandle,
    SourceFilePath, SourceFilePathHandle, SymbolMapStringInterner, SymbolNameHandle,
    SyncAddressInfo,
};

enum FullSymbolListEntry<'a, Symbol> {
    /// A synthesized symbol for a function start address that's known
    /// from some other information (not from the symbol table).
    Synthesized,
    /// A synthesized symbol for the entry point of the object.
    SynthesizedEntryPoint,
    Symbol(Symbol),
    Export(object::Export<'a>),
    PltStub(String),
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
            Self::PltStub(arg0) => f.debug_tuple("PltStub").field(arg0).finish(),
            Self::EndAddress => write!(f, "EndAddress"),
        }
    }
}

impl<'a, Symbol: object::ObjectSymbol<'a>> FullSymbolListEntry<'a, Symbol> {
    fn name(&self, addr: u32) -> Option<Cow<'_, str>> {
        let name = match self {
            FullSymbolListEntry::EndAddress => return None,
            FullSymbolListEntry::Synthesized => format!("fun_{addr:x}").into(),
            FullSymbolListEntry::SynthesizedEntryPoint => "EntryPoint".into(),
            FullSymbolListEntry::Symbol(symbol) => {
                String::from_utf8_lossy(symbol.name_bytes().ok()?)
            }
            FullSymbolListEntry::Export(export) => String::from_utf8_lossy(export.name()),
            FullSymbolListEntry::PltStub(name) => Cow::Borrowed(name.as_str()),
        };
        Some(name)
    }

    fn counts_as_proper_symbol(&self) -> bool {
        match self {
            FullSymbolListEntry::Symbol(_)
            | FullSymbolListEntry::Export(_)
            | FullSymbolListEntry::PltStub(_) => true,
            FullSymbolListEntry::EndAddress
            | FullSymbolListEntry::Synthesized
            | FullSymbolListEntry::SynthesizedEntryPoint => false,
        }
    }
}

struct ElfPltInfo {
    plt_start: u64,
    plt_header_size: u64,
    plt_entry_size: u64,
    got_plt_start: u64,
    got_entry_size: u64,
    got_reserved_entries_size: u64,
}

impl ElfPltInfo {
    fn for_object<'data, O: object::Object<'data>>(object_file: &O) -> Option<Self> {
        let plt_section = object_file.section_by_name(".plt")?;
        let got_plt_section = object_file.section_by_name(".got.plt")?;

        let (plt_header_size, plt_entry_size) = match object_file.architecture() {
            object::Architecture::X86_64
            | object::Architecture::X86_64_X32
            | object::Architecture::I386 => (16u64, 16u64),
            object::Architecture::Aarch64 | object::Architecture::Aarch64_Ilp32 => (32u64, 16u64),
            _ => return None,
        };

        let got_entry_size = if object_file.is_64() { 8u64 } else { 4u64 };

        Some(Self {
            plt_start: plt_section.address(),
            plt_header_size,
            plt_entry_size,
            got_plt_start: got_plt_section.address(),
            got_entry_size,
            got_reserved_entries_size: 3 * got_entry_size,
        })
    }

    fn header_relative_address(&self, base_address: u64) -> Option<u32> {
        relative_address_u32(self.plt_start, base_address)
    }

    fn stub_relative_address(&self, base_address: u64, reloc_offset: u64) -> Option<u32> {
        let got_slot_offset = reloc_offset
            .checked_sub(self.got_plt_start)?
            .checked_sub(self.got_reserved_entries_size)?;
        if got_slot_offset % self.got_entry_size != 0 {
            return None;
        }

        let slot_index = got_slot_offset / self.got_entry_size;
        let plt_addr = self
            .plt_start
            .checked_add(self.plt_header_size + slot_index * self.plt_entry_size)?;
        relative_address_u32(plt_addr, base_address)
    }
}

fn relative_address_u32(address: u64, base_address: u64) -> Option<u32> {
    u32::try_from(address.checked_sub(base_address)?).ok()
}

fn is_executable_section<'data, S: ObjectSection<'data>>(section: &S) -> bool {
    match (section.kind(), section.flags()) {
        (SectionKind::Text, _) => true,
        (_, SectionFlags::MachO { flags })
            if flags & object::macho::S_ATTR_PURE_INSTRUCTIONS != 0 =>
        {
            true
        }
        (SectionKind::UninitializedData, SectionFlags::Elf { sh_flags })
            if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0 =>
        {
            true
        }
        _ => false,
    }
}

fn dynamic_symbol_name<'a, SymbolTable>(
    symbol_table: Option<&SymbolTable>,
    index: object::SymbolIndex,
) -> Option<&'a str>
where
    SymbolTable: ObjectSymbolTable<'a>,
{
    let symbol = symbol_table?.symbol_by_index(index).ok()?;
    object::ObjectSymbol::name(&symbol).ok()
}

struct SymbolList<'a, Symbol> {
    entries: Vec<(u32, FullSymbolListEntry<'a, Symbol>)>,
}

impl<'a, Symbol: object::ObjectSymbol<'a> + 'a> SymbolList<'a, Symbol> {
    fn add_elf_plt_symbols<'file, O>(
        entries: &mut Vec<(u32, FullSymbolListEntry<'a, Symbol>)>,
        object_file: &'file O,
        base_address: u64,
    ) where
        'a: 'file,
        O: object::Object<'a, Symbol<'file> = Symbol>,
    {
        let Some(plt_info) = ElfPltInfo::for_object(object_file) else {
            return;
        };
        let Some(dynamic_relocations) = object_file.dynamic_relocations() else {
            return;
        };

        if let Some(header_rel) = plt_info.header_relative_address(base_address) {
            entries.push((
                header_rel,
                FullSymbolListEntry::PltStub("<PLT header>".to_owned()),
            ));
        }

        let dynsym_table = object_file.dynamic_symbol_table();
        for (reloc_offset, reloc) in dynamic_relocations {
            let object::RelocationTarget::Symbol(symbol_index) = reloc.target() else {
                continue;
            };
            let Some(plt_rel) = plt_info.stub_relative_address(base_address, reloc_offset) else {
                continue;
            };
            let Some(symbol_name) = dynamic_symbol_name(dynsym_table.as_ref(), symbol_index) else {
                continue;
            };
            entries.push((
                plt_rel,
                FullSymbolListEntry::PltStub(format!("{symbol_name}@plt")),
            ));
        }
    }

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

        // On Mach-O, executable sections other than `__TEXT,__text` (e.g. `__TEXT,__objc_stubs`,
        // `__TEXT,__jsc_int`) contain real function-like symbols, but the `object` crate doesn't
        // know to classify them as `SymbolKind::Text` -- it reports them as `SymbolKind::Unknown`.
        // We therefore accept `Unknown` symbols on Mach-O. We don't do this on ELF, where
        // `Unknown` corresponds to `STT_NOTYPE`.
        let allow_unknown_kind = object_file
            .sections()
            .any(|s| matches!(s.flags(), SectionFlags::MachO { .. }));

        // Compute the executable sections upfront. This will be used to filter out uninteresting symbols.
        let executable_sections: Vec<SectionIndex> = object_file
            .sections()
            .filter(is_executable_section)
            .map(|section| section.index())
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

                    // Filter out symbols from non-executable sections.
                    let in_executable_section = match symbol.section_index() {
                        Some(section_index) => executable_sections.contains(&section_index),
                        None => false,
                    };
                    if !in_executable_section {
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
                        SymbolKind::Unknown if allow_unknown_kind => {
                            // On mach-O __TEXT,__objc_stubs etc. can be reported as Unknown
                        }
                        _ => return false, // Cull.
                    }

                    true
                })
                .filter_map(|symbol| {
                    Some((
                        u32::try_from(symbol.address().checked_sub(base_address)?).ok()?,
                        FullSymbolListEntry::Symbol(symbol),
                    ))
                }),
        );

        // PLT stub symbols for ELF.
        //
        // PLT stubs can have unwind info but no symbol table entry at the stub address,
        // which makes them show up as "fun_XXXX". Derive their names from .got.plt
        // dynamic relocations instead.
        Self::add_elf_plt_symbols(&mut entries, object_file, base_address);

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
                .filter(is_executable_section)
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

    pub fn lookup_relative_address(&self, address: u32) -> Option<(u32, u32, Cow<'_, str>)> {
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
    object_map: ObjectMap<'a>,
    context: Option<Mutex<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>>,
    dwp_package:
        Option<addr2line::gimli::DwarfPackage<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    svma_file_ranges: SvmaFileRanges,
    image_base_address: u64,
    dwo_dwarf_maker: &'a DDM,
    cached_external_file: Mutex<Option<ExternalFileSymbolMap<FC>>>,
    string_interner: Mutex<SymbolMapStringInterner<'a>>,
    _phantom: PhantomData<FC>,
}

/// If the DWARF only gave us an abbreviated name for the outermost frame, replace it
/// with the (demangled) name of the symbol which contains the looked-up address.
///
/// The outermost frame is, by definition, the function which contains the address, so
/// the symbol table describes the same function that the DWARF does - and sometimes
/// describes it better.
///
/// clang only emits `DW_AT_linkage_name` on a subprogram DIE when it considers the
/// linkage name "descriptive", and `-mllvm=-dwarf-linkage-names=Abstract` (which
/// local Firefox builds use, to keep the debug info small) narrows this further: only
/// *abstract* subprogram DIEs keep their linkage name, i.e. the DIEs that inlined
/// frames point at, which have no symbol of their own. Concrete out-of-line
/// definitions get none. For those, all addr2line can give us is `DW_AT_name`, which
/// is the bare unqualified name with no namespace, no class and no parameters:
/// "ProcessPostTraversal" rather than
/// "mozilla::RestyleManager::ProcessPostTraversal(mozilla::dom::Element*,
/// mozilla::ServoRestyleState&, mozilla::ServoPostTraversalFlags)".
///
/// In theory, we could derive the full name from the DWARF: the chain of parent DIEs gives
/// the namespace and class qualification, and the `DW_TAG_formal_parameter` children
/// give the signature. But assembling it means implementing a C++ type printer, and
/// `-gsimple-template-names` (also used by local Firefox builds) adds to the work by storing
/// template arguments in `DW_TAG_template_type_parameter` children instead of
/// spelling them out in `DW_AT_name`. gimli and addr2line do none of this, so we take
/// the symbol name instead, which is exact and already at hand.
///
/// What we cannot know here is where addr2line's name came from: `raw_name()` returns
/// the linkage name if there is one and `DW_AT_name` otherwise, and doesn't tell us
/// which. Nor can we just always prefer the symbol name, because it isn't always the
/// better one - gcc names the symbol for a cloned function `gobble_file.constprop.0`
/// while the DWARF calls it `gobble_file`, and ld64 / ThinLTO similarly append
/// `.llvm.<hash>` to promoted local symbols.
///
/// So we only take the symbol name when it looks like a more qualified spelling of the
/// name we already have - see [`is_more_qualified_spelling`]. That covers the case we
/// care about and leaves the compiler-generated-suffix cases alone. On builds that do
/// have linkage names on concrete DIEs the two strings are equal and this is a no-op.
///
/// Inline frames are never touched: they have no symbol of their own, and their
/// abstract DIEs do still carry linkage names.
fn prefer_symbol_name_for_outer_frame<'a>(
    frames: &mut [FrameDebugInfo],
    symbol_name: &str,
    string_interner: &mut SymbolMapStringInterner<'a>,
) {
    // addr2line yields the innermost (most deeply inlined) frame first, so the
    // outermost frame is the last one.
    let Some(outer_frame) = frames.last_mut() else {
        return;
    };
    if let Some(dwarf_name) = outer_frame
        .function
        .and_then(|handle| string_interner.resolve(handle.into()))
    {
        if !is_more_qualified_spelling(symbol_name, &dwarf_name) {
            return;
        }
    }
    outer_frame.function = Some(string_interner.intern_owned(symbol_name).into());
}

/// Whether `symbol_name` is the same name as `dwarf_name`, just spelled out more
/// fully: `dwarf_name` has to occur in `symbol_name` as a whole identifier which
/// starts at a `::` boundary and is followed by nothing but a template argument list
/// and/or a parameter list.
///
/// `("mozilla::RestyleManager::ProcessPostTraversal(mozilla::dom::Element*)",
/// "ProcessPostTraversal")` qualifies, and so does an exact match, but
/// `("gobble_file.constprop.0", "gobble_file")` does not, because the extra text is a
/// suffix on the identifier rather than added qualification.
fn is_more_qualified_spelling(symbol_name: &str, dwarf_name: &str) -> bool {
    if dwarf_name.is_empty() {
        return false;
    }
    symbol_name.match_indices(dwarf_name).any(|(start, _)| {
        let starts_at_boundary = start == 0 || symbol_name[..start].ends_with("::");
        let rest = &symbol_name[start + dwarf_name.len()..];
        let ends_at_boundary = rest.is_empty() || rest.starts_with('(') || rest.starts_with('<');
        starts_at_boundary && ends_at_boundary
    })
}

impl<'a, Symbol, FC, DDM> ObjectSymbolMapInner<'a, Symbol, FC, DDM>
where
    Symbol: object::ObjectSymbol<'a> + 'a,
    FC: FileContents + 'static,
    DDM: DwoDwarfMaker<FC>,
{
    /// The demangled name of the symbol containing `svma`, for use with
    /// [`prefer_symbol_name_for_outer_frame`].
    fn demangled_symbol_name_for_svma(&self, svma: u64) -> Option<String> {
        let relative_address = u32::try_from(svma.checked_sub(self.image_base_address)?).ok()?;
        let (_start_addr, _end_addr, name) = self.list.lookup_relative_address(relative_address)?;
        Some(demangle::demangle_any(&name))
    }

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
        let mut string_interner = self.string_interner.lock().unwrap();
        match &external.file_ref {
            ExternalFileRef::MachoExternalObject { file_path } => {
                // We have no svma here, but the debug map already told us the (mangled)
                // name of the symbol containing the address - that's how we find the
                // address inside the .o file in the first place.
                let outer_frame_name = match &external.address_in_file {
                    ExternalFileAddressInFileRef::MachoOsoObject { symbol_name, .. }
                    | ExternalFileAddressInFileRef::MachoOsoArchive { symbol_name, .. } => Some(
                        demangle::demangle_any(&String::from_utf8_lossy(symbol_name)),
                    ),
                    ExternalFileAddressInFileRef::ElfDwo { .. } => None,
                };
                let make_result =
                    |frames: Vec<FrameDebugInfo>,
                     string_interner: &mut SymbolMapStringInterner<'a>| {
                        let mut frames = frames;
                        if let Some(name) = &outer_frame_name {
                            prefer_symbol_name_for_outer_frame(&mut frames, name, string_interner);
                        }
                        FramesLookupResult::Available(frames)
                    };

                {
                    let cached_external_file = self.cached_external_file.lock().unwrap();
                    match &*cached_external_file {
                        Some(external_file) if external_file.file_path() == file_path => {
                            return external_file
                                .lookup(&external.address_in_file, &mut string_interner)
                                .map(|frames| make_result(frames, &mut string_interner));
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
                    .lookup(&external.address_in_file, &mut string_interner)
                    .map(|frames| make_result(frames, &mut string_interner));

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
                            let outer_frame_name = self.demangled_symbol_name_for_svma(*svma);
                            convert_frames(frame_iter, &mut string_interner).map(|mut frames| {
                                if let Some(name) = &outer_frame_name {
                                    prefer_symbol_name_for_outer_frame(
                                        &mut frames,
                                        name,
                                        &mut string_interner,
                                    );
                                }
                                FramesLookupResult::Available(frames)
                            })
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

        let name_handle = {
            let mut string_interner = self.string_interner.lock().unwrap();
            string_interner.intern_owned(&name)
        };

        let symbol = SymbolInfo {
            address: start_addr,
            size: Some(function_size),
            name: name_handle.into(),
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
                        let mut string_interner = self.string_interner.lock().unwrap();
                        convert_frames(frame_iter, &mut string_interner).map(|mut frames| {
                            prefer_symbol_name_for_outer_frame(
                                &mut frames,
                                &name,
                                &mut string_interner,
                            );
                            FramesLookupResult::Available(frames)
                        })
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
        if let Some(FramesLookupResult::External(external_file_address_ref)) = frames {
            frames = self.try_lookup_external_impl(
                &external_file_address_ref,
                ExternalLookupRequest::ReplyIfYouHaveOrTellMeWhatYouNeed,
            );
        }
        Some(SyncAddressInfo { symbol, frames })
    }

    fn resolve_function_name(&self, handle: FunctionNameHandle) -> Cow<'_, str> {
        let string_interner = self.string_interner.lock().unwrap();
        let s = string_interner.resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_symbol_name(&self, handle: SymbolNameHandle) -> Cow<'_, str> {
        let string_interner = self.string_interner.lock().unwrap();
        let s = string_interner.resolve(handle.into());
        s.expect("unknown handle?")
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        let string_interner = self.string_interner.lock().unwrap();
        let raw_path = string_interner
            .resolve(handle.into())
            .expect("unknown handle?");
        SourceFilePath::RawPath(raw_path.clone())
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
            object_map: object_file.object_map(),
            context: addr2line_context.map(Mutex::new),
            dwp_package,
            image_base_address: base_address,
            svma_file_ranges: SvmaFileRanges::from_object(object_file),
            dwo_dwarf_maker,
            cached_external_file: Mutex::new(None),
            string_interner: Mutex::new(SymbolMapStringInterner::new(SymbolMapGeneration::new())),
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

#[cfg(test)]
mod tests {
    use super::is_more_qualified_spelling;

    #[test]
    fn qualification_added_by_the_symbol_table() {
        // The case this is all for: -dwarf-linkage-names=Abstract left us with
        // just DW_AT_name for the concrete definition.
        assert!(is_more_qualified_spelling(
            "mozilla::RestyleManager::ProcessPostTraversal(mozilla::dom::Element*, mozilla::ServoRestyleState&, mozilla::ServoPostTraversalFlags)",
            "ProcessPostTraversal"
        ));
        assert!(is_more_qualified_spelling(
            "ns::Widget::Update(ns::Inner&, ns::Holder<int> const&) const",
            "Update"
        ));
        // -gsimple-template-names drops the template arguments from DW_AT_name.
        assert!(is_more_qualified_spelling(
            "mozilla::Maybe<mozilla::ServoRestyleState>::Maybe()",
            "Maybe"
        ));
        assert!(is_more_qualified_spelling(
            "ns::Widget::~Widget()",
            "~Widget"
        ));
        assert!(is_more_qualified_spelling(
            "ns::Widget::operator()(int)",
            "operator()"
        ));
        // Nothing to add, but nothing lost either.
        assert!(is_more_qualified_spelling("main", "main"));
    }

    #[test]
    fn compiler_generated_symbol_suffixes() {
        // The extra text is a suffix on the identifier rather than added
        // qualification, so the DWARF name is the better one and we keep it.
        assert!(!is_more_qualified_spelling(
            "gobble_file.constprop.0",
            "gobble_file"
        ));
        assert!(!is_more_qualified_spelling("foo.llvm.12345", "foo"));
        assert!(!is_more_qualified_spelling("foo [clone .cold]", "foo"));
        // A partial identifier match is not a match.
        assert!(!is_more_qualified_spelling("ns::UpdateAll()", "Update"));
        assert!(!is_more_qualified_spelling("ns::ReUpdate()", "Update"));
        // Unrelated names, e.g. after identical code folding.
        assert!(!is_more_qualified_spelling("ns::Widget::Update()", "Scale"));
        assert!(!is_more_qualified_spelling("anything", ""));
    }
}
