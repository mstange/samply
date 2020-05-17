use super::type_dumper::{ParentScope, TypeDumper};
use pdb::{FallibleIterator, Result, SymbolData, PDB};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Frame<'s> {
    pub function: Option<String>,
    pub location: Option<Location<'s>>,
}

#[derive(Clone)]
pub struct Location<'s> {
    pub file: Option<std::borrow::Cow<'s, str>>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

pub struct Addr2LineContext<'a, 's>
where
    's: 'a,
{
    address_map: &'a pdb::AddressMap<'s>,
    string_table: &'a pdb::StringTable<'s>,
    dbi: &'a pdb::DebugInformation<'s>,
    type_dumper: &'a TypeDumper<'a>,
    id_finder: pdb::ItemFinder<'a, pdb::IdIndex>,
}

impl<'a, 's> Addr2LineContext<'a, 's> {
    pub fn new(
        address_map: &'a pdb::AddressMap<'s>,
        string_table: &'a pdb::StringTable<'s>,
        dbi: &'a pdb::DebugInformation<'s>,
        ipi: &'a pdb::ItemInformation<'s, pdb::IdIndex>,
        type_dumper: &'a TypeDumper<'a>,
    ) -> Result<Self> {
        // Fill id_finder
        let mut id_finder = ipi.finder();
        let mut id_iter = ipi.iter();
        while let Some(_) = id_iter.next()? {
            id_finder.update(&id_iter);
        }

        Ok(Self {
            address_map,
            string_table,
            dbi,
            type_dumper,
            id_finder,
        })
    }

    pub fn find_frames<'b, 't, S>(
        &self,
        pdb: &mut PDB<'t, S>,
        address: u32,
    ) -> Result<Vec<Frame<'b>>>
    where
        S: pdb::Source<'t>,
        's: 't,
        S: 's,
        's: 'b,
        'a: 'b,
    {
        let mut modules = self.dbi.modules()?.filter_map(|m| pdb.module_info(&m));
        while let Some(module_info) = modules.next()? {
            let proc_symbol = module_info.symbols()?.find_map(|symbol| {
                if let Ok(SymbolData::Procedure(proc)) = symbol.parse() {
                    let start_rva = match proc.offset.to_rva(&self.address_map) {
                        Some(rva) => rva,
                        None => return Ok(None),
                    };

                    let procedure_rva_range = start_rva.0..(start_rva.0 + proc.len);
                    if !procedure_rva_range.contains(&address) {
                        return Ok(None);
                    }
                    return Ok(Some((symbol.index(), proc, procedure_rva_range)));
                }
                Ok(None)
            })?;

            if let Some((symbol_index, proc, procedure_rva_range)) = proc_symbol {
                let line_program = module_info.line_program()?;

                let inlinees: BTreeMap<pdb::IdIndex, pdb::Inlinee> = module_info
                    .inlinees()?
                    .map(|i| Ok((i.index(), i)))
                    .collect()?;

                return self.find_frames_from_procedure(
                    address,
                    &module_info,
                    symbol_index,
                    proc,
                    procedure_rva_range,
                    &line_program,
                    &inlinees,
                );
            }
        }
        Ok(vec![])
    }

    pub fn find_frames_from_procedure<'b>(
        &self,
        address: u32,
        module_info: &pdb::ModuleInfo,
        symbol_index: pdb::SymbolIndex,
        proc: pdb::ProcedureSymbol,
        procedure_rva_range: std::ops::Range<u32>,
        line_program: &pdb::LineProgram,
        inlinees: &BTreeMap<pdb::IdIndex, pdb::Inlinee>,
    ) -> Result<Vec<Frame<'b>>>
    where
        's: 'b,
        'a: 'b,
    {
        self.find_frames_for_addresses_from_procedure(
            &[address],
            module_info,
            symbol_index,
            proc,
            procedure_rva_range,
            line_program,
            inlinees,
        )
        .map(|map| map.into_iter().next().unwrap().1)
    }

    /// addresses must be sorted, low to high
    pub fn find_frames_for_addresses_from_procedure<'b>(
        &self,
        addresses: &[u32],
        module_info: &pdb::ModuleInfo,
        symbol_index: pdb::SymbolIndex,
        proc: pdb::ProcedureSymbol,
        procedure_rva_range: std::ops::Range<u32>,
        line_program: &pdb::LineProgram,
        inlinees: &BTreeMap<pdb::IdIndex, pdb::Inlinee>,
    ) -> Result<BTreeMap<u32, Vec<Frame<'b>>>>
    where
        's: 'b,
        'a: 'b,
    {
        let function = self
            .type_dumper
            .dump_function(&proc.name.to_string(), proc.type_index, None)
            .ok();

        // Ordered outside to inside, until just before the end of this function.
        let mut frames_per_address: BTreeMap<u32, Vec<_>> = BTreeMap::new();

        for &address in addresses {
            let frame = Frame {
                function: function.clone(),
                location: None,
            };
            frames_per_address.insert(address, vec![frame]);
        }

        let lines_for_proc = line_program.lines_at_offset(proc.offset);
        for (addresses_subset, line_info) in self
            .find_line_infos_containing_addresses_no_size(
                lines_for_proc,
                addresses,
                procedure_rva_range.end,
            )
            .into_iter()
        {
            let location = self.line_info_to_location(line_info, &line_program);
            for address in addresses_subset {
                let frame = &mut frames_per_address.get_mut(address).unwrap()[0];
                frame.location = Some(location.clone());
            }
        }

        let mut inline_symbols_iter = module_info.symbols_at(symbol_index)?;

        // Skip the procedure symbol that we're currently in.
        inline_symbols_iter.next()?;

        while let Some(symbol) = inline_symbols_iter.next()? {
            match symbol.parse() {
                Ok(SymbolData::Procedure(_)) => {
                    // This is the start of the procedure *after* the one we care about. We're done.
                    break;
                }
                Ok(SymbolData::InlineSite(site)) => {
                    if let Some(inline_frames_for_addresses) = self
                        .frames_for_addresses_for_inline_symbol(
                            site,
                            addresses,
                            &inlinees,
                            proc.offset,
                            &line_program,
                        )
                    {
                        for (addresses_subset, frame) in inline_frames_for_addresses.into_iter() {
                            for address in addresses_subset {
                                frames_per_address
                                    .get_mut(address)
                                    .unwrap()
                                    .push(frame.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Now order from inside to outside.
        for (_address, frames) in frames_per_address.iter_mut() {
            frames.reverse();
        }

        Ok(frames_per_address)
    }

    fn frames_for_addresses_for_inline_symbol<'b, 'addresses>(
        &self,
        site: pdb::InlineSiteSymbol,
        addresses: &'addresses [u32],
        inlinees: &BTreeMap<pdb::IdIndex, pdb::Inlinee>,
        proc_offset: pdb::PdbInternalSectionOffset,
        line_program: &pdb::LineProgram,
    ) -> Option<Vec<(&'addresses [u32], Frame<'b>)>>
    where
        's: 'b,
        'a: 'b,
        'b: 'addresses,
    {
        // This inlining site only covers the address if it has a line info that covers this address.
        let inlinee = inlinees.get(&site.inlinee)?;
        let lines = inlinee.lines(proc_offset, &site);
        let line_infos = self.find_line_infos_containing_addresses_with_size(lines, addresses);
        if line_infos.is_empty() {
            return None;
        }

        let function = match self.id_finder.find(site.inlinee).and_then(|i| i.parse()) {
            Ok(pdb::IdData::Function(f)) => {
                // TODO: Do cross-module resolution when looking up scope ID
                let scope = f
                    .scope
                    .and_then(|scope| self.id_finder.find(scope).ok())
                    .and_then(|i| i.parse().ok())
                    .map(|id_data| ParentScope::WithId(id_data));

                self.type_dumper
                    .dump_function(&f.name.to_string(), f.function_type, scope)
                    .ok()
            }
            Ok(pdb::IdData::MemberFunction(m)) => self
                .type_dumper
                .dump_function(
                    &m.name.to_string(),
                    m.function_type,
                    Some(ParentScope::WithType(m.parent)),
                )
                .ok(),
            _ => None,
        };

        let mut frames = Vec::new();
        for (address_range, line_info) in line_infos.into_iter() {
            let location = self.line_info_to_location(line_info, line_program);

            frames.push((
                address_range,
                Frame {
                    function: function.clone(),
                    location: Some(location),
                },
            ));
        }
        Some(frames)
    }

    fn find_line_infos_containing_addresses_no_size<'addresses>(
        &self,
        iterator: impl FallibleIterator<Item = pdb::LineInfo, Error = pdb::Error> + Clone,
        addresses: &'addresses [u32],
        outer_end_rva: u32,
    ) -> Vec<(&'addresses [u32], pdb::LineInfo)>
    where
        'a: 'addresses,
        's: 'addresses,
    {
        let start_rva_iterator = iterator
            .clone()
            .map(|line_info| Ok(line_info.offset.to_rva(&self.address_map).unwrap().0));
        let outer_end_rva_iterator = fallible_once(Ok(outer_end_rva));
        let end_rva_iterator = start_rva_iterator
            .clone()
            .skip(1)
            .chain(outer_end_rva_iterator);
        let mut line_iterator = start_rva_iterator.zip(end_rva_iterator).zip(iterator);
        let mut line_infos = Vec::new();
        while let Ok(Some(((start_rva, end_rva), line_info))) = line_iterator.next() {
            let range = start_rva..end_rva;
            let covered_addresses = get_addresses_covered_by_range(addresses, range);
            if !covered_addresses.is_empty() {
                line_infos.push((covered_addresses, line_info));
            }
        }
        line_infos
    }

    fn find_line_infos_containing_addresses_with_size<'addresses>(
        &self,
        mut iterator: impl FallibleIterator<Item = pdb::LineInfo, Error = pdb::Error> + Clone,
        addresses: &'addresses [u32],
    ) -> Vec<(&'addresses [u32], pdb::LineInfo)>
    where
        'a: 'addresses,
        's: 'addresses,
    {
        let mut line_infos = Vec::new();
        while let Ok(Some(line_info)) = iterator.next() {
            let length = match line_info.length {
                Some(l) => l,
                None => continue,
            };
            let start_rva = line_info.offset.to_rva(&self.address_map).unwrap().0;
            let end_rva = start_rva + length;
            let range = start_rva..end_rva;
            let covered_addresses = get_addresses_covered_by_range(addresses, range);
            if !covered_addresses.is_empty() {
                line_infos.push((covered_addresses, line_info));
            }
        }
        line_infos
    }

    fn line_info_to_location<'b>(
        &self,
        line_info: pdb::LineInfo,
        line_program: &pdb::LineProgram,
    ) -> Location<'b>
    where
        'a: 'b,
        's: 'b,
    {
        let file = line_program
            .get_file_info(line_info.file_index)
            .and_then(|file_info| file_info.name.to_string_lossy(&self.string_table))
            .ok()
            .map(|name| name);
        Location {
            file,
            line: Some(line_info.line_start),
            column: line_info.column_start,
        }
    }
}

fn fallible_once<T, E>(value: std::result::Result<T, E>) -> Once<T, E> {
    Once { value: Some(value) }
}

struct Once<T, E> {
    value: Option<std::result::Result<T, E>>,
}

impl<T, E> FallibleIterator for Once<T, E> {
    type Item = T;
    type Error = E;

    fn next(&mut self) -> std::result::Result<Option<Self::Item>, Self::Error> {
        match self.value.take() {
            Some(Ok(value)) => Ok(Some(value)),
            Some(Err(err)) => Err(err),
            None => Ok(None),
        }
    }
}

pub fn get_addresses_covered_by_range(addresses: &[u32], range: std::ops::Range<u32>) -> &[u32] {
    let index_of_first_address_gte_range_start = match addresses.binary_search(&range.start) {
        Ok(i) => i,
        Err(i) => i,
    };
    // Compute the index of the first item *outside* the range (one past last)
    let index_of_first_address_gt_range_end = match addresses.binary_search(&range.end) {
        Ok(i) => i,
        Err(i) => i,
    };
    if index_of_first_address_gt_range_end > index_of_first_address_gte_range_start {
        &addresses[index_of_first_address_gte_range_start..index_of_first_address_gt_range_end]
    } else {
        &[]
    }
}
