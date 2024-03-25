use std::{borrow::Cow, slice, sync::Mutex};

use debugid::DebugId;
use gimli::{EndianSlice, RunTimeEndian};
use object::{
    ObjectMap, ObjectSection, ObjectSegment, SectionFlags, SectionIndex, SectionKind, SymbolKind,
};

use crate::demangle;
use crate::dwarf::get_frames;
use crate::path_mapper::PathMapper;
use crate::shared::{
    relative_address_base, AddressInfo, ExternalFileAddressInFileRef, ExternalFileAddressRef,
    ExternalFileRef, FramesLookupResult, SymbolInfo,
};
use crate::symbol_map::SymbolMapTrait;

pub trait FunctionAddressesComputer<'data> {
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>;
}

enum FullSymbolListEntry<'a, Symbol: object::ObjectSymbol<'a>> {
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
    fn name(&self, addr: u32) -> Result<Cow<'a, str>, ()> {
        match self {
            FullSymbolListEntry::Synthesized => Ok(format!("fun_{addr:x}").into()),
            FullSymbolListEntry::SynthesizedEntryPoint => Ok("EntryPoint".into()),
            FullSymbolListEntry::Symbol(symbol) => match symbol.name_bytes() {
                Ok(name) => Ok(String::from_utf8_lossy(name)),
                Err(_) => Err(()),
            },
            FullSymbolListEntry::Export(export) => Ok(String::from_utf8_lossy(export.name())),
            FullSymbolListEntry::EndAddress => Err(()),
        }
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

impl std::fmt::Debug for SvmaFileRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvmaFileRange")
            .field("svma", &format!("{:#x}", &self.svma))
            .field("file_offset", &format!("{:#x}", &self.file_offset))
            .field("size", &format!("{:#x}", &self.size))
            .finish()
    }
}

pub struct ObjectSymbolMapInner<'a, Symbol: object::ObjectSymbol<'a>> {
    entries: Vec<(u32, FullSymbolListEntry<'a, Symbol>)>,
    debug_id: DebugId,
    arch: Option<&'static str>,
    path_mapper: Mutex<PathMapper<()>>,
    object_map: ObjectMap<'a>,
    context: Option<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    svma_file_ranges: Vec<SvmaFileRange>,
    image_base_address: u64,
}

#[test]
fn test_symbolmap_is_send() {
    use object::ReadRef;
    fn assert_is_send<T: Send>() {}
    #[allow(unused)]
    fn wrapper<'a, R: ReadRef<'a> + Send + Sync>() {
        assert_is_send::<ObjectSymbolMapInner<<object::read::File<'a, R> as object::Object>::Symbol>>(
        );
    }
}

impl<'a, Symbol: object::ObjectSymbol<'a>> ObjectSymbolMapInner<'a, Symbol> {
    pub fn new<'file, O>(
        object_file: &'file O,
        addr2line_context: Option<addr2line::Context<EndianSlice<'a, RunTimeEndian>>>,
        debug_id: DebugId,
        function_start_addresses: Option<&[u32]>,
        function_end_addresses: Option<&[u32]>,
        arch: Option<&'static str>,
    ) -> Self
    where
        'a: 'file,
        O: object::Object<'a, 'file, Symbol = Symbol>,
    {
        let mut entries: Vec<_> = Vec::new();

        let base_address = relative_address_base(object_file);

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

        let path_mapper = Mutex::new(PathMapper::new());

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

        Self {
            entries,
            debug_id,
            path_mapper,
            object_map: object_file.object_map(),
            context: addr2line_context,
            arch,
            image_base_address: base_address,
            svma_file_ranges,
        }
    }

    fn file_offset_to_svma(&self, offset: u64) -> Option<u64> {
        for svma_file_range in &self.svma_file_ranges {
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

impl<'a, Symbol: object::ObjectSymbol<'a>> SymbolMapTrait for ObjectSymbolMapInner<'a, Symbol> {
    fn debug_id(&self) -> DebugId {
        self.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|&(_, entry)| {
                matches!(
                    entry,
                    FullSymbolListEntry::Symbol(_) | FullSymbolListEntry::Export(_)
                )
            })
            .count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        Box::new(SymbolMapIter {
            inner: self.entries.iter(),
        })
    }

    fn lookup_relative_address(&self, address: u32) -> Option<AddressInfo> {
        let index = match self
            .entries
            .binary_search_by_key(&address, |&(addr, _)| addr)
        {
            Err(0) => return None,
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let (start_addr, entry) = &self.entries[index];
        let next_entry = self.entries.get(index + 1);
        // If the found entry is an EndAddress entry, this means that `address` falls
        // in the dead space between known functions, and we consider it to be not found.
        // In that case, entry.name returns Err().
        if let (Ok(name), Some((end_addr, _))) = (entry.name(*start_addr), next_entry) {
            let function_size = end_addr - *start_addr;

            let mut path_mapper = self.path_mapper.lock().unwrap();

            let svma = self.image_base_address + u64::from(address);
            let frames = match get_frames(svma, self.context.as_ref(), &mut path_mapper) {
                Some(frames) => FramesLookupResult::Available(frames),
                None => {
                    if let Some(entry) = self.object_map.get(svma) {
                        let external_file_name = entry.object(&self.object_map);
                        let external_file_name = std::str::from_utf8(external_file_name).unwrap();
                        let offset_from_symbol = (svma - entry.address()) as u32;
                        let (file_name, name_in_archive) = match external_file_name.find('(') {
                            Some(index) => {
                                // This is an "archive" reference of the form
                                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../../js/src/build/libjs_static.a(Unified_cpp_js_src13.o)"
                                let (path, paren_rest) = external_file_name.split_at(index);
                                let name_in_archive =
                                    paren_rest.trim_start_matches('(').trim_end_matches(')');
                                (path, Some(name_in_archive))
                            }
                            None => {
                                // This is a reference to a regular object file. Example:
                                // "/Users/mstange/code/obj-m-opt/toolkit/library/build/../../components/sessionstore/Unified_cpp_sessionstore0.o"
                                (external_file_name, None)
                            }
                        };
                        FramesLookupResult::External(ExternalFileAddressRef {
                            file_ref: ExternalFileRef {
                                file_name: file_name.to_owned(),
                                arch: self.arch.map(ToOwned::to_owned),
                            },
                            address_in_file: ExternalFileAddressInFileRef {
                                name_in_archive: name_in_archive.map(ToOwned::to_owned),
                                symbol_name: entry.name().to_owned(),
                                offset_from_symbol,
                            },
                        })
                    } else {
                        FramesLookupResult::Unavailable
                    }
                }
            };

            let name = demangle::demangle_any(&name);
            Some(AddressInfo {
                symbol: SymbolInfo {
                    address: *start_addr,
                    size: Some(function_size),
                    name,
                },
                frames,
            })
        } else {
            None
        }
    }

    fn lookup_svma(&self, svma: u64) -> Option<AddressInfo> {
        let relative_address = svma.checked_sub(self.image_base_address)?.try_into().ok()?;
        // 4200608 2103456 2097152
        self.lookup_relative_address(relative_address)
    }

    fn lookup_offset(&self, offset: u64) -> Option<AddressInfo> {
        let svma = self.file_offset_to_svma(offset)?;
        self.lookup_svma(svma)
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
            let &(address, ref symbol) = self.inner.next()?;
            let name = match symbol.name(address) {
                Ok(name) => name,
                Err(_) => continue,
            };
            return Some((address, name));
        }
    }
}
