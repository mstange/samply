use std::convert::TryFrom;
use std::{borrow::Cow, slice, sync::Mutex};

use debugid::DebugId;
use object::{File, ObjectMap, ReadRef, SectionKind, SymbolKind};

use crate::ExternalFileAddressRef;
use crate::{
    demangle,
    dwarf::{get_frames, Addr2lineContextData},
    path_mapper::PathMapper,
    shared::{
        relative_address_base, AddressInfo, ExternalFileAddressInFileRef, ExternalFileRef,
        SymbolInfo,
    },
    symbol_map::{SymbolMapDataMidTrait, SymbolMapInnerWrapper, SymbolMapTrait},
    Error, FramesLookupResult,
};

pub trait FunctionAddressesComputer<'data> {
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>;
}

pub struct ObjectSymbolMapDataMid<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>> {
    object: File<'data, R>,
    supplementary_object: Option<File<'data, R>>,
    function_addresses_computer: FAC,
    file_data: R,
    supplementary_file_data: Option<R>,
    addr2line_context_data: Addr2lineContextData,
    arch: Option<&'static str>,
    debug_id: DebugId,
}

impl<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>>
    ObjectSymbolMapDataMid<'data, R, FAC>
{
    pub fn new(
        object: File<'data, R>,
        supplementary_object: Option<File<'data, R>>,
        function_addresses_computer: FAC,
        file_data: R,
        supplementary_file_data: Option<R>,
        arch: Option<&'static str>,
        debug_id: DebugId,
    ) -> Self {
        Self {
            object,
            supplementary_object,
            function_addresses_computer,
            file_data,
            supplementary_file_data,
            addr2line_context_data: Addr2lineContextData::new(),
            arch,
            debug_id,
        }
    }
}

impl<'data, R: ReadRef<'data>, FAC: FunctionAddressesComputer<'data>> SymbolMapDataMidTrait
    for ObjectSymbolMapDataMid<'data, R, FAC>
{
    fn make_symbol_map_inner(&self) -> Result<SymbolMapInnerWrapper<'_>, Error> {
        let (function_starts, function_ends) = self
            .function_addresses_computer
            .compute_function_addresses(&self.object);

        let symbol_map = ObjectSymbolMapInner::new(
            &self.object,
            self.supplementary_object.as_ref(),
            self.file_data,
            self.supplementary_file_data,
            self.debug_id,
            function_starts.as_deref(),
            function_ends.as_deref(),
            self.arch,
            &self.addr2line_context_data,
        );
        let symbol_map = SymbolMapInnerWrapper(Box::new(symbol_map));
        Ok(symbol_map)
    }
}

enum FullSymbolListEntry<'a, Symbol: object::ObjectSymbol<'a>> {
    Synthesized,
    Symbol(Symbol),
    Export(object::Export<'a>),
    EndAddress,
}

impl<'a, Symbol: object::ObjectSymbol<'a>> FullSymbolListEntry<'a, Symbol> {
    fn name(&self, addr: u32) -> Result<Cow<'a, str>, ()> {
        match self {
            FullSymbolListEntry::Synthesized => Ok(format!("fun_{:x}", addr).into()),
            FullSymbolListEntry::Symbol(symbol) => match symbol.name_bytes() {
                Ok(name) => Ok(String::from_utf8_lossy(name)),
                Err(_) => Err(()),
            },
            FullSymbolListEntry::Export(export) => Ok(String::from_utf8_lossy(export.name())),
            FullSymbolListEntry::EndAddress => Err(()),
        }
    }
}

pub struct ObjectSymbolMapInner<'data, 'file, Symbol: object::ObjectSymbol<'data>>
where
    'data: 'file,
{
    entries: Vec<(u32, FullSymbolListEntry<'data, Symbol>)>,
    debug_id: DebugId,
    arch: Option<&'static str>,
    path_mapper: Mutex<PathMapper<()>>,
    object_map: ObjectMap<'data>,
    context: Option<addr2line::Context<gimli::EndianSlice<'file, gimli::RunTimeEndian>>>,
    image_base_address: u64,
}

#[test]
fn test_symbolmap_is_send() {
    fn assert_is_send<T: Send>() {}
    #[allow(unused)]
    fn wrapper<'a, R: ReadRef<'a> + Send + Sync>() {
        assert_is_send::<ObjectSymbolMapInner<<object::read::File<'a, R> as object::Object>::Symbol>>(
        );
    }
}

impl<'data, 'file, Symbol: object::ObjectSymbol<'data>> ObjectSymbolMapInner<'data, 'file, Symbol>
where
    'data: 'file,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new<O, R>(
        object_file: &'file O,
        sup_object_file: Option<&'file O>,
        data: R,
        sup_data: Option<R>,
        debug_id: DebugId,
        function_start_addresses: Option<&[u32]>,
        function_end_addresses: Option<&[u32]>,
        arch: Option<&'static str>,
        addr2line_context_data: &'file Addr2lineContextData,
    ) -> Self
    where
        'data: 'file,
        O: object::Object<'data, 'file, Symbol = Symbol>,
        R: ReadRef<'data>,
    {
        let mut entries: Vec<_> = Vec::new();

        let base_address = relative_address_base(object_file);

        // Add entries in the order "best to worst".

        // 1. Normal symbols
        // 2. Dynamic symbols (only used by ELF files, I think)
        use object::ObjectSection;
        entries.extend(
            object_file
                .symbols()
                .chain(object_file.dynamic_symbols())
                .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.address() != 0)
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

        // 5. End addresses from text section ends
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

        // 6. End addresses for sized symbols
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

        // 7. End addresses for known functions ends
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

        let context = addr2line_context_data
            .make_context(data, object_file, sup_data, sup_object_file)
            .ok();

        let path_mapper = Mutex::new(PathMapper::new());

        Self {
            entries,
            debug_id,
            path_mapper,
            object_map: object_file.object_map(),
            context,
            arch,
            image_base_address: base_address,
        }
    }
}

impl<'data, 'file, Symbol: object::ObjectSymbol<'data>> SymbolMapTrait
    for ObjectSymbolMapInner<'data, 'file, Symbol>
where
    'data: 'file,
{
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

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
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

            let vmaddr = self.image_base_address + u64::from(address);
            let frames = match get_frames(vmaddr, self.context.as_ref(), &mut path_mapper) {
                Some(frames) => FramesLookupResult::Available(frames),
                None => {
                    if let Some(entry) = self.object_map.get(vmaddr) {
                        let external_file_name = entry.object(&self.object_map);
                        let external_file_name = std::str::from_utf8(external_file_name).unwrap();
                        let offset_from_symbol = (vmaddr - entry.address()) as u32;
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
