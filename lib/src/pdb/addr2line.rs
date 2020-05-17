use super::type_dumper::{ParentScope, TypeDumper};
use pdb::{FallibleIterator, Result, SymbolData, PDB};
use std::collections::BTreeMap;

pub struct Frame<'s> {
    pub function: Option<String>,
    pub location: Option<Location<'s>>,
}

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
                return self.find_frames_from_procedure(
                    address,
                    &module_info,
                    symbol_index,
                    proc,
                    procedure_rva_range,
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
    ) -> Result<Vec<Frame<'b>>>
    where
        's: 'b,
        'a: 'b,
    {
        let function = self
            .type_dumper
            .dump_function(&proc.name.to_string(), proc.type_index, None)
            .ok();

        let line_program = module_info.line_program()?;

        let location = self
            .find_line_info_containing_address(
                line_program.lines_at_offset(proc.offset),
                address,
                Some(procedure_rva_range.end),
            )
            .map(|line_info| self.line_info_to_location(line_info, &line_program));

        // Ordered outside to inside, until just before the end of this function.
        let mut frames = vec![Frame { function, location }];

        let inlinees: BTreeMap<_, _> = module_info
            .inlinees()?
            .map(|i| Ok((i.index(), i)))
            .collect()?;

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
                    if let Some(frame) = self.frame_for_inline_symbol(
                        site,
                        address,
                        &inlinees,
                        proc.offset,
                        &line_program,
                    ) {
                        frames.push(frame);
                    }
                }
                _ => {}
            }
        }

        // Now order from inside to outside.
        frames.reverse();

        Ok(frames)
    }

    fn frame_for_inline_symbol<'b>(
        &self,
        site: pdb::InlineSiteSymbol,
        address: u32,
        inlinees: &BTreeMap<pdb::IdIndex, pdb::Inlinee>,
        proc_offset: pdb::PdbInternalSectionOffset,
        line_program: &pdb::LineProgram,
    ) -> Option<Frame<'b>>
    where
        's: 'b,
        'a: 'b,
    {
        if let Some(inlinee) = inlinees.get(&site.inlinee) {
            if let Some(line_info) = self.find_line_info_containing_address(
                inlinee.lines(proc_offset, &site),
                address,
                None,
            ) {
                let location = self.line_info_to_location(line_info, line_program);

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
                return Some(Frame {
                    function,
                    location: Some(location),
                });
            }
        }
        None
    }

    fn find_line_info_containing_address<LineIterator>(
        &self,
        iterator: LineIterator,
        address: u32,
        outer_end_rva: Option<u32>,
    ) -> Option<pdb::LineInfo>
    where
        LineIterator: FallibleIterator<Item = pdb::LineInfo, Error = pdb::Error>,
    {
        let mut lines = iterator.peekable();
        while let Some(line_info) = lines.next().ok()? {
            let start_rva = line_info
                .offset
                .to_rva(&self.address_map)
                .expect("invalid rva")
                .0;
            if address < start_rva {
                continue;
            }
            let end_rva = match (line_info.length, outer_end_rva) {
                (Some(length), _) => Some(start_rva + length),
                (None, Some(fallback_end)) => {
                    let next_line_info_rva = lines
                        .peek()
                        .ok()?
                        .map(|i| i.offset.to_rva(&self.address_map).expect("invalid rva").0);
                    Some(next_line_info_rva.unwrap_or(fallback_end))
                }
                (None, None) => None,
            };
            match end_rva {
                Some(end_rva) if address < end_rva => return Some(line_info),
                _ => {}
            }
        }
        None
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
