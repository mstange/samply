use pdb::{
    AddressMap, DebugInformation, Error, FallibleIterator, IdIndex, InlineSiteSymbol, Inlinee,
    LineInfo, LineProgram, ModuleInfo, PdbInternalSectionOffset, Result, Source, StringTable,
    SymbolData, SymbolIndex, PDB,
};
use pdb::{RawString, TypeIndex};
use pdb_addr2line::TypeFormatter;
use std::collections::btree_map::Entry;
use std::rc::Rc;
use std::{borrow::Cow, cell::RefCell, collections::BTreeMap};

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

#[derive(Clone)]
struct Procedure<'a> {
    start_rva: u32,
    end_rva: u32,
    module_index: u16,
    symbol_index: SymbolIndex,
    offset: PdbInternalSectionOffset,
    name: RawString<'a>,
    type_index: TypeIndex,
}

struct ExtendedProcedureInfo {
    name: Rc<String>,
}

struct ExtendedModuleInfo<'a> {
    inlinees: BTreeMap<IdIndex, Inlinee<'a>>,
    line_program: LineProgram<'a>,
}

pub struct CachedPdbInfo<'s> {
    address_map: AddressMap<'s>,
    string_table: StringTable<'s>,
    modules: Vec<ModuleInfo<'s>>,
}

impl<'s> CachedPdbInfo<'s> {
    pub fn try_from_pdb<S: Source<'s> + 's>(
        pdb: &mut PDB<'s, S>,
        dbi: &DebugInformation<'s>,
    ) -> Result<Self> {
        let mut modules = Vec::new();
        let mut module_iter = dbi.modules()?;
        while let Some(module) = module_iter.next()? {
            let module_info = match pdb.module_info(&module)? {
                Some(m) => m,
                None => continue,
            };
            modules.push(module_info);
        }

        let address_map = pdb.address_map()?;
        let string_table = pdb.string_table()?;

        Ok(Self {
            address_map,
            string_table,
            modules,
        })
    }
}

pub struct Addr2LineContext<'a, 's, 't> {
    address_map: &'a AddressMap<'s>,
    string_table: &'a StringTable<'s>,
    type_formatter: &'a TypeFormatter<'t>,
    modules: &'a [ModuleInfo<'s>],
    procedures: Vec<Procedure<'a>>,
    procedure_cache: RefCell<BTreeMap<u32, ExtendedProcedureInfo>>,
    module_cache: RefCell<BTreeMap<u16, Rc<ExtendedModuleInfo<'a>>>>,
}

impl<'a, 's, 't> Addr2LineContext<'a, 's, 't> {
    pub fn new(
        pdb_info: &'a CachedPdbInfo<'s>,
        type_formatter: &'a TypeFormatter<'t>,
    ) -> Result<Self> {
        let mut procedures = Vec::new();

        for (module_index, module_info) in pdb_info.modules.iter().enumerate() {
            let mut symbols_iter = module_info.symbols()?;
            while let Some(symbol) = symbols_iter.next()? {
                if let Ok(SymbolData::Procedure(proc)) = symbol.parse() {
                    if proc.len == 0 {
                        continue;
                    }
                    let start_rva = match proc.offset.to_rva(&pdb_info.address_map) {
                        Some(rva) => rva.0,
                        None => continue,
                    };

                    procedures.push(Procedure {
                        start_rva,
                        end_rva: start_rva + proc.len,
                        module_index: module_index as u16,
                        symbol_index: symbol.index(),
                        offset: proc.offset,
                        name: proc.name,
                        type_index: proc.type_index,
                    });
                }
            }
        }

        // Sort and de-duplicate, so that we can use binary search during lookup.
        // If we have multiple procs at the same address (as a result of identical code folding),
        // we'd like to keep the last instance that we encountered in the original order.
        // dedup_by_key keeps the *first* element of consecutive duplicates, so we reverse first
        // and then use a stable sort before we de-duplicate.
        procedures.reverse();
        procedures.sort_by_key(|p| p.start_rva);
        procedures.dedup_by_key(|p| p.start_rva);

        Ok(Self {
            address_map: &pdb_info.address_map,
            string_table: &pdb_info.string_table,
            type_formatter,
            modules: &pdb_info.modules,
            procedures,
            procedure_cache: RefCell::new(BTreeMap::new()),
            module_cache: RefCell::new(BTreeMap::new()),
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
        let name = self.get_procedure_name(proc);
        Ok(Some((start_rva, (*name).clone())))
    }

    pub fn find_frames(&self, address: u32) -> Result<Option<(u32, Vec<Frame<'a>>)>> {
        let proc = match self.lookup_proc(address) {
            Some(proc) => proc,
            None => return Ok(None),
        };
        let module_info = &self.modules[proc.module_index as usize];
        let module = self.get_extended_module_info(proc.module_index)?;
        let frames = self.find_frames_from_procedure(
            address,
            module_info,
            proc,
            &module.line_program,
            &module.inlinees,
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

    fn compute_procedure_name(&self, proc: &Procedure) -> String {
        let mut formatted_function_name = String::new();
        let _ = self.type_formatter.write_function(
            &mut formatted_function_name,
            &proc.name.to_string(),
            proc.type_index,
        );
        formatted_function_name
    }

    fn get_procedure_name(&self, proc: &Procedure) -> Rc<String> {
        let mut cache = self.procedure_cache.borrow_mut();
        cache
            .entry(proc.start_rva)
            .or_insert_with(|| ExtendedProcedureInfo {
                name: Rc::new(self.compute_procedure_name(proc)),
            })
            .name
            .clone()
    }

    fn compute_extended_module_info(&self, module_index: u16) -> Result<ExtendedModuleInfo<'a>> {
        let module_info = &self.modules[module_index as usize];
        let line_program = module_info.line_program()?;

        let inlinees: BTreeMap<IdIndex, Inlinee> = module_info
            .inlinees()?
            .map(|i| Ok((i.index(), i)))
            .collect()?;

        Ok(ExtendedModuleInfo {
            inlinees,
            line_program,
        })
    }

    fn get_extended_module_info(&self, module_index: u16) -> Result<Rc<ExtendedModuleInfo<'a>>> {
        let mut cache = self.module_cache.borrow_mut();
        match cache.entry(module_index) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let m = self.compute_extended_module_info(module_index)?;
                Ok(e.insert(Rc::new(m)).clone())
            }
        }
    }

    fn find_frames_from_procedure(
        &self,
        address: u32,
        module_info: &ModuleInfo,
        proc: &Procedure,
        line_program: &LineProgram,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
    ) -> Result<Vec<Frame<'a>>> {
        let location = self
            .find_line_info_containing_address(proc, line_program, address)?
            .map(|line_info| self.line_info_to_location(line_info, &line_program));

        let frame = Frame {
            function: Some((*self.get_procedure_name(proc)).clone()),
            location,
        };

        // Ordered outside to inside, until just before the end of this function.
        let mut frames = vec![frame];

        let mut inline_symbols_iter = module_info.symbols_at(proc.symbol_index)?.skip(1);
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
                        frames.push(frame.clone());
                    }
                }
                _ => {}
            }
        }

        // Now order from inside to outside.
        frames.reverse();

        Ok(frames)
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

    fn find_line_info_containing_address(
        &self,
        proc: &Procedure,
        line_program: &LineProgram,
        address: u32,
    ) -> Result<Option<LineInfo>> {
        let lines_for_proc = line_program.lines_at_offset(proc.offset);
        let mut iterator = lines_for_proc.map(|line_info| {
            let rva = line_info.offset.to_rva(&self.address_map).unwrap().0;
            Ok((rva, line_info))
        });
        let mut next_item = iterator.next()?;
        while let Some((start_rva, line_info)) = next_item {
            next_item = iterator.next()?;
            let end_rva = match &next_item {
                Some((rva, _)) => *rva,
                None => proc.end_rva,
            };
            if start_rva <= address && address < end_rva {
                return Ok(Some(line_info));
            }
        }
        Ok(None)
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
