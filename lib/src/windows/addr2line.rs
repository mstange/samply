use pdb::{
    AddressMap, DebugInformation, FallibleIterator, FileIndex, IdIndex, InlineSiteSymbol, Inlinee,
    LineProgram, ModuleInfo, PdbInternalSectionOffset, Result, Source, StringTable, SymbolData,
    SymbolIndex, SymbolIter, PDB,
};
use pdb::{RawString, TypeIndex};
use pdb_addr2line::TypeFormatter;
use range_collections::RangeSet;
use std::cmp::Ordering;
use std::collections::btree_map::Entry;
use std::ops::Bound;
use std::rc::Rc;
use std::{borrow::Cow, cell::RefCell, collections::BTreeMap};

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

#[derive(Clone)]
pub struct Frame<'a> {
    pub function: String,
    pub location: Option<Location<'a>>,
}

#[derive(Clone)]
pub struct Location<'a> {
    pub file: Option<Cow<'a, str>>,
    pub line: Option<u32>,
}

pub struct Addr2LineContext<'a, 's, 't> {
    address_map: &'a AddressMap<'s>,
    string_table: &'a StringTable<'s>,
    type_formatter: &'a TypeFormatter<'t>,
    modules: &'a [ModuleInfo<'s>],
    procedures: Vec<Procedure<'a>>,
    procedure_cache: RefCell<ProcedureCache>,
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
                        end_symbol_index: proc.end,
                        offset: proc.offset,
                        name: proc.name,
                        type_index: proc.type_index,
                    });
                }
            }
        }

        // Sort and de-duplicate, so that we can use binary search during lookup.
        // If we have multiple procs at the same probe (as a result of identical code folding),
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
            procedure_cache: RefCell::new(Default::default()),
            module_cache: RefCell::new(BTreeMap::new()),
        })
    }

    pub fn total_symbol_count(&self) -> usize {
        self.procedures.len()
    }

    pub fn find_function(&self, probe: u32) -> Result<Option<(u32, String)>> {
        let proc = match self.lookup_proc(probe) {
            Some(proc) => proc,
            None => return Ok(None),
        };
        let start_rva = proc.start_rva;
        let name = self.get_procedure_name(proc);
        Ok(Some((start_rva, (*name).clone())))
    }

    pub fn find_frames(&self, probe: u32) -> Result<Option<(u32, Vec<Frame<'a>>)>> {
        let proc = match self.lookup_proc(probe) {
            Some(proc) => proc,
            None => return Ok(None),
        };

        let module_info = &self.modules[proc.module_index as usize];
        let module = self.get_extended_module_info(proc.module_index)?;
        let line_program = &module.line_program;
        let inlinees = &module.inlinees;

        let function = (*self.get_procedure_name(proc)).clone();
        let lines = &self.get_procedure_lines(proc, line_program)?[..];
        let search = match lines.binary_search_by_key(&probe, |li| li.start_rva) {
            Err(0) => None,
            Ok(i) => Some(i),
            Err(i) => Some(i - 1),
        };
        let location = search.map(|index| {
            let line_info = &lines[index];
            Location {
                file: self.resolve_filename(&line_program, line_info.file_index),
                line: Some(line_info.line_start),
            }
        });

        let frame = Frame { function, location };

        // Ordered outside to inside, until just before the end of this function.
        let mut frames = vec![frame];

        let inline_ranges = self.get_procedure_inline_ranges(module_info, proc, inlinees)?;
        let mut inline_ranges = &inline_ranges[..];

        loop {
            let current_depth = (frames.len() - 1) as u16;

            // Look up (probe, current_depth) in inline_ranges.
            // `inlined_addresses` is sorted in "breadth-first traversal order", i.e.
            // by `call_depth` first, and then by `start_rva`. See the comment at
            // the sort call for more information about why.
            let search = inline_ranges.binary_search_by(|range| {
                if range.call_depth > current_depth {
                    Ordering::Greater
                } else if range.call_depth < current_depth {
                    Ordering::Less
                } else if range.start_rva > probe {
                    Ordering::Greater
                } else if range.end_rva <= probe {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            });
            let (inline_range, remainder) = match search {
                Ok(index) => (&inline_ranges[index], &inline_ranges[index + 1..]),
                Err(_) => break,
            };
            let mut function = String::new();
            let _ = self
                .type_formatter
                .write_id(&mut function, inline_range.inlinee);
            let file = inline_range
                .file_index
                .and_then(|file_index| self.resolve_filename(line_program, file_index));
            let location = Some(Location {
                file,
                line: inline_range.line_start,
            });
            frames.push(Frame { function, location });

            inline_ranges = remainder;
        }

        // Now order from inside to outside.
        frames.reverse();

        Ok(Some((proc.start_rva, frames)))
    }

    fn lookup_proc(&self, probe: u32) -> Option<&Procedure> {
        let last_procedure_starting_lte_address = match self
            .procedures
            .binary_search_by_key(&probe, |p| p.start_rva)
        {
            Err(0) => return None,
            Ok(i) => i,
            Err(i) => i - 1,
        };
        assert!(self.procedures[last_procedure_starting_lte_address].start_rva <= probe);
        if probe >= self.procedures[last_procedure_starting_lte_address].end_rva {
            return None;
        }
        Some(&self.procedures[last_procedure_starting_lte_address])
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

    fn get_procedure_name(&self, proc: &Procedure) -> Rc<String> {
        let mut cache = self.procedure_cache.borrow_mut();
        let entry = cache.get_entry_mut(proc.start_rva);
        match &entry.name {
            Some(name) => name.clone(),
            None => {
                let name = Rc::new(self.compute_procedure_name(proc));
                entry.name = Some(name.clone());
                name
            }
        }
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

    fn get_procedure_lines(
        &self,
        proc: &Procedure,
        line_program: &LineProgram,
    ) -> Result<Rc<Vec<CachedLineInfo>>> {
        let mut cache = self.procedure_cache.borrow_mut();
        let entry = cache.get_entry_mut(proc.start_rva);
        match &entry.lines {
            Some(lines) => Ok(lines.clone()),
            None => {
                let lines = Rc::new(self.compute_procedure_lines(proc, line_program)?);
                entry.lines = Some(lines.clone());
                Ok(lines)
            }
        }
    }

    fn compute_procedure_lines(
        &self,
        proc: &Procedure,
        line_program: &LineProgram,
    ) -> Result<Vec<CachedLineInfo>> {
        let lines_for_proc = line_program.lines_at_offset(proc.offset);
        let mut iterator = lines_for_proc.map(|line_info| {
            let rva = line_info.offset.to_rva(&self.address_map).unwrap().0;
            Ok((rva, line_info))
        });
        let mut lines = Vec::new();
        let mut next_item = iterator.next()?;
        while let Some((start_rva, line_info)) = next_item {
            next_item = iterator.next()?;
            lines.push(CachedLineInfo {
                start_rva,
                file_index: line_info.file_index,
                line_start: line_info.line_start,
            });
        }
        Ok(lines)
    }

    fn get_procedure_inline_ranges(
        &self,
        module_info: &ModuleInfo,
        proc: &Procedure,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
    ) -> Result<Rc<Vec<InlineRange>>> {
        let mut cache = self.procedure_cache.borrow_mut();
        let entry = cache.get_entry_mut(proc.start_rva);
        match &entry.inline_ranges {
            Some(inline_ranges) => Ok(inline_ranges.clone()),
            None => {
                let inline_ranges =
                    Rc::new(self.compute_procedure_inline_ranges(module_info, proc, inlinees)?);
                entry.inline_ranges = Some(inline_ranges.clone());
                Ok(inline_ranges)
            }
        }
    }

    fn compute_procedure_inline_ranges(
        &self,
        module_info: &ModuleInfo,
        proc: &Procedure,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
    ) -> Result<Vec<InlineRange>> {
        let mut lines = Vec::new();
        let mut symbols_iter = module_info.symbols_at(proc.symbol_index)?;
        let _proc_sym = symbols_iter.next()?;
        while let Some(symbol) = symbols_iter.next()? {
            if symbol.index() >= proc.end_symbol_index {
                break;
            }
            match symbol.parse() {
                Ok(SymbolData::Procedure(p)) => {
                    // This is a nested procedure. Skip it.
                    symbols_iter.skip_to(p.end)?;
                }
                Ok(SymbolData::InlineSite(site)) => {
                    self.process_inlinee_symbols(
                        &mut symbols_iter,
                        inlinees,
                        proc.offset,
                        site,
                        0,
                        &mut lines,
                    )?;
                }
                _ => {}
            }
        }

        lines.sort_by(|r1, r2| {
            if r1.call_depth < r2.call_depth {
                Ordering::Less
            } else if r1.call_depth > r2.call_depth {
                Ordering::Greater
            } else if r1.start_rva < r2.start_rva {
                Ordering::Less
            } else if r1.start_rva > r2.start_rva {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });

        Ok(lines)
    }

    fn process_inlinee_symbols(
        &self,
        symbols_iter: &mut SymbolIter,
        inlinees: &BTreeMap<IdIndex, Inlinee>,
        proc_offset: PdbInternalSectionOffset,
        site: InlineSiteSymbol,
        call_depth: u16,
        lines: &mut Vec<InlineRange>,
    ) -> Result<RangeSet<u32>> {
        let mut name = String::new();
        let _ = self.type_formatter.write_id(&mut name, site.inlinee);

        let mut ranges = RangeSet::empty();
        let mut file_index = None;
        if let Some(inlinee) = inlinees.get(&site.inlinee) {
            let mut iter = inlinee.lines(proc_offset, &site);
            while let Ok(Some(line_info)) = iter.next() {
                let length = match line_info.length {
                    Some(0) | None => {
                        continue;
                    }
                    Some(l) => l,
                };
                let start_rva = line_info.offset.to_rva(&self.address_map).unwrap().0;
                let end_rva = start_rva + length;
                lines.push(InlineRange {
                    start_rva,
                    end_rva,
                    call_depth,
                    inlinee: site.inlinee,
                    file_index: Some(line_info.file_index),
                    line_start: Some(line_info.line_start),
                });
                ranges |= RangeSet::from(start_rva..end_rva);
                if file_index.is_none() {
                    file_index = Some(line_info.file_index);
                }
            }
        }

        let mut callee_ranges = RangeSet::empty();
        while let Some(symbol) = symbols_iter.next()? {
            if symbol.index() >= site.end {
                break;
            }
            match symbol.parse() {
                Ok(SymbolData::Procedure(p)) => {
                    // This is a nested procedure. Skip it.
                    symbols_iter.skip_to(p.end)?;
                }
                Ok(SymbolData::InlineSite(site)) => {
                    callee_ranges |= self.process_inlinee_symbols(
                        symbols_iter,
                        inlinees,
                        proc_offset,
                        site,
                        call_depth + 1,
                        lines,
                    )?;
                }
                _ => {}
            }
        }

        if !ranges.is_superset(&callee_ranges) {
            // Workaround bad debug info.
            let missing_ranges: RangeSet<u32> = &callee_ranges - &ranges;
            for range in missing_ranges.iter() {
                let (start_rva, end_rva) = match range {
                    (Bound::Included(s), Bound::Excluded(e)) => (*s, *e),
                    other => {
                        panic!("Unexpected range bounds {:?}", other);
                    }
                };
                lines.push(InlineRange {
                    start_rva,
                    end_rva,
                    call_depth,
                    inlinee: site.inlinee,
                    file_index,
                    line_start: None,
                });
            }
            ranges |= missing_ranges;
        }

        Ok(ranges)
    }

    fn resolve_filename(
        &self,
        line_program: &LineProgram,
        file_index: FileIndex,
    ) -> Option<Cow<'a, str>> {
        line_program
            .get_file_info(file_index)
            .ok()
            .and_then(|file_info| file_info.name.to_string_lossy(&self.string_table).ok())
    }
}

#[derive(Default)]
struct ProcedureCache(BTreeMap<u32, ExtendedProcedureInfo>);

impl ProcedureCache {
    fn get_entry_mut(&mut self, procedure_start_rva: u32) -> &mut ExtendedProcedureInfo {
        self.0
            .entry(procedure_start_rva)
            .or_insert_with(|| ExtendedProcedureInfo {
                name: None,
                lines: None,
                inline_ranges: None,
            })
    }
}

#[derive(Clone)]
struct Procedure<'a> {
    start_rva: u32,
    end_rva: u32,
    module_index: u16,
    symbol_index: SymbolIndex,
    end_symbol_index: SymbolIndex,
    offset: PdbInternalSectionOffset,
    name: RawString<'a>,
    type_index: TypeIndex,
}

struct ExtendedProcedureInfo {
    name: Option<Rc<String>>,
    lines: Option<Rc<Vec<CachedLineInfo>>>,
    inline_ranges: Option<Rc<Vec<InlineRange>>>,
}

struct ExtendedModuleInfo<'a> {
    inlinees: BTreeMap<IdIndex, Inlinee<'a>>,
    line_program: LineProgram<'a>,
}

#[derive(Clone)]
struct CachedLineInfo {
    pub start_rva: u32,
    pub file_index: FileIndex,
    pub line_start: u32,
}

#[derive(Clone, Debug)]
struct InlineRange {
    pub start_rva: u32,
    pub end_rva: u32,
    pub call_depth: u16,
    pub inlinee: IdIndex,
    pub file_index: Option<FileIndex>,
    pub line_start: Option<u32>,
}
