use pdb::{
    AddressMap, DebugInformation, Error, FallibleIterator, IdIndex, InlineSiteSymbol, Inlinee,
    LineInfo, LineProgram, ModuleInfo, PdbInternalSectionOffset, Result, Source, StringTable,
    SymbolData, SymbolIndex, PDB,
};
use pdb_addr2line::TypeFormatter;
use std::{borrow::Cow, collections::BTreeMap};

#[derive(Clone)]
pub struct Frame<'a> {
    pub function: Option<String>,
    pub location: Option<Location<'a>>,
}

#[derive(Clone)]
pub struct Location<'a> {
    pub file: Option<Cow<'a, str>>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

struct Procedure {
    start_rva: u32,
    end_rva: u32,
    module_index: u16,
    symbol_index: SymbolIndex,
}

pub struct Addr2LineContext<'a, 's, 't> {
    address_map: &'a AddressMap<'s>,
    string_table: &'a StringTable<'s>,
    type_formatter: &'a TypeFormatter<'t>,
    modules: Vec<ModuleInfo<'s>>,
    procedures: Vec<Procedure>,
}

impl<'a, 's, 't> Addr2LineContext<'a, 's, 't> {
    pub fn new<S: Source<'s> + 's>(
        pdb: &mut PDB<'s, S>,
        address_map: &'a AddressMap<'s>,
        string_table: &'a StringTable<'s>,
        dbi: &'a DebugInformation<'s>,
        type_formatter: &'a TypeFormatter<'t>,
    ) -> Result<Self> {
        let mut modules = Vec::new();
        let mut procedures = Vec::new();

        let mut module_iter = dbi.modules()?;
        let mut prev_start_rva = None;
        while let Some(module) = module_iter.next()? {
            let module_info = match pdb.module_info(&module)? {
                Some(m) => m,
                None => continue,
            };
            let module_index = modules.len();
            let mut symbols_iter = module_info.symbols()?;
            while let Some(symbol) = symbols_iter.next()? {
                if let Ok(SymbolData::Procedure(proc)) = symbol.parse() {
                    if proc.len == 0 {
                        continue;
                    }
                    let start_rva = match proc.offset.to_rva(address_map) {
                        Some(rva) => rva,
                        None => continue,
                    };

                    let procedure = Procedure {
                        start_rva: start_rva.0,
                        end_rva: start_rva.0 + proc.len,
                        module_index: module_index as u16,
                        symbol_index: symbol.index(),
                    };

                    // De-duplicate original-order-consecutive procedures, keeping last
                    if prev_start_rva == Some(start_rva) {
                        *procedures.last_mut().unwrap() = procedure;
                    } else {
                        procedures.push(procedure);
                    }
                    prev_start_rva = Some(start_rva);
                }
            }
            modules.push(module_info);
        }

        // Sort and de-duplicate, so that we can use binary search during lookup.
        procedures.sort_by_key(|p| p.start_rva);
        procedures.dedup_by_key(|p| p.start_rva);

        Ok(Self {
            address_map,
            string_table,
            type_formatter,
            modules,
            procedures,
        })
    }

    pub fn total_symbol_count(&self) -> usize {
        self.procedures.len()
    }

    pub fn find_function(&self, address: u32) -> Result<Option<(u32, String)>> {
        let proc = match self.lookup_proc(address) {
            Some(proc) => proc,
            None => return Ok(None),
        };

        let start_rva = proc.start_rva;
        let module_info = &self.modules[proc.module_index as usize];
        let mut symbols_iter = module_info.symbols_at(proc.symbol_index)?;

        let proc = match symbols_iter.next()? {
            Some(symbol) => match symbol.parse()? {
                SymbolData::Procedure(proc) => proc,
                _ => panic!("Did we store a bad symbol offset?"),
            },
            None => panic!("Did we store a bad symbol offset?"),
        };

        let mut formatted_function_name = String::new();
        let _ = self.type_formatter.write_function(
            &mut formatted_function_name,
            &proc.name.to_string(),
            proc.type_index,
        );
        Ok(Some((start_rva, formatted_function_name)))
    }

    pub fn find_frames(&self, address: u32) -> Result<Option<(u32, Vec<Frame<'a>>)>> {
        let proc = match self.lookup_proc(address) {
            Some(proc) => proc,
            None => return Ok(None),
        };
        let module_info = &self.modules[proc.module_index as usize];
        let line_program = module_info.line_program()?;

        let inlinees: BTreeMap<IdIndex, Inlinee> = module_info
            .inlinees()?
            .map(|i| Ok((i.index(), i)))
            .collect()?;

        let frames = self.find_frames_from_procedure(
            address,
            module_info,
            proc.symbol_index,
            proc.end_rva,
            &line_program,
            &inlinees,
        )?;
        Ok(Some((proc.start_rva, frames)))
    }

    fn lookup_proc(&self, address: u32) -> Option<&Procedure> {
        let last_procedure_starting_lte_address = match self
            .procedures
            .binary_search_by_key(&address, |p| p.start_rva)
        {
            Err(0) => return None,
            Ok(i) => i,
            Err(i) => i - 1,
        };
        assert!(self.procedures[last_procedure_starting_lte_address].start_rva <= address);
        if address >= self.procedures[last_procedure_starting_lte_address].end_rva {
            return None;
        }
        Some(&self.procedures[last_procedure_starting_lte_address])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn find_frames_from_procedure(
        &self,
        address: u32,
        module_info: &ModuleInfo,
        symbol_index: SymbolIndex,
        procedure_end_rva: u32,
        line_program: &LineProgram,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
    ) -> Result<Vec<Frame<'a>>> {
        let mut symbols_iter = module_info.symbols_at(symbol_index)?;

        let proc = match symbols_iter.next()? {
            Some(symbol) => match symbol.parse()? {
                SymbolData::Procedure(proc) => proc,
                _ => panic!("Did we store a bad symbol offset?"),
            },
            None => panic!("Did we store a bad symbol offset?"),
        };

        let mut formatted_function_name = String::new();
        let _ = self.type_formatter.write_function(
            &mut formatted_function_name,
            &proc.name.to_string(),
            proc.type_index,
        );
        let function = Some(formatted_function_name);

        // Ordered outside to inside, until just before the end of this function.
        let mut frames_per_address: BTreeMap<u32, Vec<_>> = BTreeMap::new();

        let frame = Frame {
            function,
            location: None,
        };
        frames_per_address.insert(address, vec![frame]);

        let lines_for_proc = line_program.lines_at_offset(proc.offset);
        if let Some(line_info) = self.find_line_info_containing_address_no_size(
            lines_for_proc,
            address,
            procedure_end_rva,
        ) {
            let location = self.line_info_to_location(line_info, &line_program);
            let frame = &mut frames_per_address.get_mut(&address).unwrap()[0];
            frame.location = Some(location.clone());
        }

        let mut inline_symbols_iter = symbols_iter;
        while let Some(symbol) = inline_symbols_iter.next()? {
            match symbol.parse() {
                Ok(SymbolData::Procedure(_)) => {
                    // This is the start of the procedure *after* the one we care about. We're done.
                    break;
                }
                Ok(SymbolData::InlineSite(site)) => {
                    if let Some(frame) = self.frames_for_address_for_inline_symbol(
                        site,
                        address,
                        &inlinees,
                        proc.offset,
                        &line_program,
                    ) {
                        frames_per_address
                            .get_mut(&address)
                            .unwrap()
                            .push(frame.clone());
                    }
                }
                _ => {}
            }
        }

        // Now order from inside to outside.
        for (_address, frames) in frames_per_address.iter_mut() {
            frames.reverse();
        }

        Ok(frames_per_address.into_iter().next().unwrap().1)
    }

    fn frames_for_address_for_inline_symbol(
        &self,
        site: InlineSiteSymbol,
        address: u32,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
        proc_offset: PdbInternalSectionOffset,
        line_program: &LineProgram,
    ) -> Option<Frame<'a>> {
        // This inlining site only covers the address if it has a line info that covers this address.
        let inlinee = inlinees.get(&site.inlinee)?;
        let lines = inlinee.lines(proc_offset, &site);
        let line_info = match self.find_line_info_containing_address_with_size(lines, address) {
            Some(line_info) => line_info,
            None => return None,
        };

        let mut formatted_name = String::new();
        let _ = self
            .type_formatter
            .write_id(&mut formatted_name, site.inlinee);
        let function = Some(formatted_name);

        let location = self.line_info_to_location(line_info, line_program);

        Some(Frame {
            function,
            location: Some(location),
        })
    }

    fn find_line_info_containing_address_no_size(
        &self,
        iterator: impl FallibleIterator<Item = LineInfo, Error = Error> + Clone,
        address: u32,
        outer_end_rva: u32,
    ) -> Option<LineInfo> {
        let start_rva_iterator = iterator
            .clone()
            .map(|line_info| Ok(line_info.offset.to_rva(&self.address_map).unwrap().0));
        let outer_end_rva_iterator = fallible_once(Ok(outer_end_rva));
        let end_rva_iterator = start_rva_iterator
            .clone()
            .skip(1)
            .chain(outer_end_rva_iterator);
        let mut line_iterator = start_rva_iterator.zip(end_rva_iterator).zip(iterator);
        while let Ok(Some(((start_rva, end_rva), line_info))) = line_iterator.next() {
            if start_rva <= address && address < end_rva {
                return Some(line_info);
            }
        }
        None
    }

    fn find_line_info_containing_address_with_size(
        &self,
        mut iterator: impl FallibleIterator<Item = LineInfo, Error = Error> + Clone,
        address: u32,
    ) -> Option<LineInfo> {
        while let Ok(Some(line_info)) = iterator.next() {
            let length = match line_info.length {
                Some(l) => l,
                None => continue,
            };
            let start_rva = line_info.offset.to_rva(&self.address_map).unwrap().0;
            let end_rva = start_rva + length;
            if start_rva <= address && address < end_rva {
                return Some(line_info);
            }
        }
        None
    }

    fn line_info_to_location(
        &self,
        line_info: LineInfo,
        line_program: &LineProgram,
    ) -> Location<'a> {
        let file = line_program
            .get_file_info(line_info.file_index)
            .and_then(|file_info| file_info.name.to_string_lossy(&self.string_table))
            .ok();
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
