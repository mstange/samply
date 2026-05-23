use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use samply_symbols::{
    AccessPatternHint, BinaryImage, ExternalFileAddressRef, ExternalFileRef, FileLoadError,
    FileLocation, FileTypes, FramesLookupResult, LibraryInfo, LookupAddress, SymbolMap,
};

use crate::error::Error;
use crate::query_state::{ApiQueryState, ApiStep};
use crate::symbolicate::looked_up_addresses::{AddressResult, AddressResults};
use crate::symbolicate::response_json::{LibSymbols, Response};
use crate::{to_debug_id, ModuleLoadOutcome, ModuleStat, QueryApiJsonResult, SymbolicateStats};

pub mod looked_up_addresses;
pub mod request_json;
pub mod response_json;

use request_json::Lib;

/// Sans-IO state-machine implementation of `/symbolicate/v5`.
///
/// Addresses whose debug info lives in an external file
/// (`FramesLookupResult::External`) are chased via [`ApiStep::NeedFile`] so
/// macOS OSO-stab and ELF dwo workflows are supported.
pub struct SymbolicateApiQueryState<FT: FileTypes> {
    state: SymbolicateState<FT>,
}

struct SymbolicateContext<FT: FileTypes> {
    request: request_json::Request,
    pending_libs: VecDeque<(Lib, Vec<u32>)>,
    results: HashMap<Lib, Result<LibSymbols<FT>, samply_symbols::Error>>,
    module_stats: Vec<ModuleStat>,
    jobs_count: usize,
    stacks_count: usize,
    frames_count: usize,
}

enum SymbolicateState<FT: FileTypes> {
    AwaitingSymbolMap {
        ctx: SymbolicateContext<FT>,
        current_lib: Lib,
        current_addresses: Vec<u32>,
        library_info: LibraryInfo,
    },
    /// Symbol map loaded; chasing external file references for addresses
    /// that returned `FramesLookupResult::External` from `lookup_sync`.
    AwaitingExternalFile {
        ctx: SymbolicateContext<FT>,
        current_lib: Lib,
        symbol_map: SymbolMap<FT>,
        address_results: AddressResults,
        current_address: u32,
        current_external: ExternalFileAddressRef,
        location: FT::FL,
        pending_externals: VecDeque<(u32, ExternalFileAddressRef)>,
    },
    Done(Result<response_json::Response<FT>, Error>),
    Poisoned,
}

impl<FT: FileTypes> SymbolicateApiQueryState<FT> {
    pub fn from_request_json(request_json: &str) -> Result<Self, Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        Self::new(request)
    }

    pub fn new(request: request_json::Request) -> Result<Self, Error> {
        let requested_addresses = gather_requested_addresses(&request)?;
        let jobs_count = request.jobs().count();
        let stacks_count = request.jobs().map(|j| j.stacks.len()).sum();
        let frames_count = request
            .jobs()
            .map(|j| j.stacks.iter().map(|s| s.0.len()).sum::<usize>())
            .sum();
        let pending_libs: VecDeque<_> = requested_addresses.into_iter().collect();
        let ctx = SymbolicateContext {
            request,
            pending_libs,
            results: HashMap::new(),
            module_stats: Vec::new(),
            jobs_count,
            stacks_count,
            frames_count,
        };
        let mut me = Self {
            state: SymbolicateState::Poisoned,
        };
        me.advance(ctx);
        Ok(me)
    }

    fn advance(&mut self, mut ctx: SymbolicateContext<FT>) {
        while let Some((lib, addresses)) = ctx.pending_libs.pop_front() {
            match to_debug_id(lib.breakpad_id.as_str()) {
                Ok(debug_id) => {
                    let library_info = LibraryInfo {
                        debug_name: Some(lib.debug_name.to_string()),
                        debug_id: Some(debug_id),
                        ..Default::default()
                    };
                    self.state = SymbolicateState::AwaitingSymbolMap {
                        ctx,
                        current_lib: lib,
                        current_addresses: addresses,
                        library_info,
                    };
                    return;
                }
                Err(e) => {
                    record_failure(&mut ctx, &lib, &e);
                    ctx.results.insert(lib, Err(e));
                }
            }
        }
        // No more libs — assemble the response.
        let SymbolicateContext {
            request,
            results,
            module_stats,
            jobs_count,
            stacks_count,
            frames_count,
            ..
        } = ctx;
        self.state = SymbolicateState::Done(Ok(Response {
            request,
            symbols_per_lib: results,
            stats: SymbolicateStats {
                jobs_count,
                stacks_count,
                frames_count,
                module_stats,
            },
        }));
    }

    /// Process all addresses for the current lib synchronously, queuing up
    /// any addresses whose frames live in an external file. After this, the
    /// state machine either has more externals to chase or is ready to move
    /// on to the next lib.
    fn process_lib_addresses(
        &mut self,
        ctx: SymbolicateContext<FT>,
        current_lib: Lib,
        current_addresses: &[u32],
        symbol_map: SymbolMap<FT>,
    ) {
        let mut address_results: AddressResults =
            current_addresses.iter().map(|&addr| (addr, None)).collect();
        symbol_map.set_access_pattern_hint(AccessPatternHint::SequentialLookup);

        let mut external_addresses: Vec<(u32, ExternalFileAddressRef)> = Vec::new();
        for (&address, address_result) in &mut address_results {
            let Some(address_info) = symbol_map.lookup_sync(LookupAddress::Relative(address))
            else {
                continue;
            };
            *address_result = Some(AddressResult::new(address_info.symbol));
            match address_info.frames {
                Some(FramesLookupResult::Available(frames)) => {
                    address_result.as_mut().unwrap().set_debug_info(frames);
                }
                Some(FramesLookupResult::External(ext)) => {
                    external_addresses.push((address, ext));
                }
                None => {}
            }
        }

        // The symbol map only caches the most recent external file, so group
        // addresses by their `ExternalFileAddressRef` to maximize cache hits.
        external_addresses.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
        let pending_externals: VecDeque<(u32, ExternalFileAddressRef)> = external_addresses.into();

        self.start_next_external(
            ctx,
            current_lib,
            symbol_map,
            address_results,
            pending_externals,
        );
    }

    /// Pop the next external lookup off the queue and either set up a
    /// `NeedFile` request, or — if no candidate file location can be derived
    /// — try a "use cached" fallback synchronously and proceed.
    fn start_next_external(
        &mut self,
        ctx: SymbolicateContext<FT>,
        current_lib: Lib,
        symbol_map: SymbolMap<FT>,
        mut address_results: AddressResults,
        mut pending_externals: VecDeque<(u32, ExternalFileAddressRef)>,
    ) {
        while let Some((address, external)) = pending_externals.pop_front() {
            let location = match &external.file_ref {
                ExternalFileRef::MachoExternalObject { file_path } => symbol_map
                    .debug_file_location()
                    .location_for_external_object_file(file_path),
                ExternalFileRef::ElfExternalDwo { comp_dir, path } => symbol_map
                    .debug_file_location()
                    .location_for_dwo(comp_dir, path),
            };
            match location {
                Some(location) => {
                    self.state = SymbolicateState::AwaitingExternalFile {
                        ctx,
                        current_lib,
                        symbol_map,
                        address_results,
                        current_address: address,
                        current_external: external,
                        location,
                        pending_externals,
                    };
                    return;
                }
                None => {
                    // No location to fetch — fall back to whatever we have
                    // already cached for this external file.
                    let next = symbol_map.try_lookup_external_with_file_contents(&external, None);
                    consume_external_result(
                        next,
                        address,
                        &mut address_results,
                        &mut pending_externals,
                    );
                }
            }
        }
        // No more externals for this lib — finalize and advance.
        self.finalize_lib(ctx, current_lib, symbol_map, address_results);
    }

    fn finalize_lib(
        &mut self,
        mut ctx: SymbolicateContext<FT>,
        current_lib: Lib,
        symbol_map: SymbolMap<FT>,
        address_results: AddressResults,
    ) {
        let lib_symbols = LibSymbols {
            address_results,
            symbol_map: Arc::new(symbol_map),
        };
        ctx.module_stats.push(ModuleStat {
            debug_name: current_lib.debug_name.to_string(),
            breakpad_id: current_lib.breakpad_id.to_string(),
            outcome: ModuleLoadOutcome::Loaded,
        });
        ctx.results.insert(current_lib, Ok(lib_symbols));
        self.advance(ctx);
    }
}

/// Apply a `FramesLookupResult` to `address_results` for the given address.
/// If the result is itself another `External`, push it to the front of
/// `pending_externals` so the caller's loop picks it up next.
fn consume_external_result(
    result: Option<FramesLookupResult>,
    address: u32,
    address_results: &mut AddressResults,
    pending_externals: &mut VecDeque<(u32, ExternalFileAddressRef)>,
) {
    match result {
        Some(FramesLookupResult::Available(frames)) => {
            if let Some(Some(addr_result)) = address_results.get_mut(&address) {
                addr_result.set_debug_info(frames);
            }
        }
        Some(FramesLookupResult::External(new_external)) => {
            pending_externals.push_front((address, new_external));
        }
        None => {}
    }
}

impl<FT: FileTypes> ApiQueryState<FT> for SymbolicateApiQueryState<FT> {
    fn poll(&self) -> ApiStep<FT> {
        match &self.state {
            SymbolicateState::AwaitingSymbolMap { library_info, .. } => {
                ApiStep::NeedSymbolMap(library_info.clone())
            }
            SymbolicateState::AwaitingExternalFile { location, .. } => ApiStep::NeedFile {
                location: location.clone(),
                required: false,
            },
            SymbolicateState::Done(_) => ApiStep::Done,
            SymbolicateState::Poisoned => unreachable!("invalid SymbolicateApiQueryState state"),
        }
    }

    fn provide_symbol_map(&mut self, result: Result<SymbolMap<FT>, samply_symbols::Error>) {
        let state = std::mem::replace(&mut self.state, SymbolicateState::Poisoned);
        let SymbolicateState::AwaitingSymbolMap {
            mut ctx,
            current_lib,
            current_addresses,
            ..
        } = state
        else {
            panic!("provide_symbol_map called when not awaiting a symbol map");
        };
        match result {
            Ok(symbol_map) => {
                self.process_lib_addresses(ctx, current_lib, &current_addresses, symbol_map);
            }
            Err(e) => {
                ctx.module_stats.push(ModuleStat {
                    debug_name: current_lib.debug_name.to_string(),
                    breakpad_id: current_lib.breakpad_id.to_string(),
                    outcome: ModuleLoadOutcome::Failed {
                        error_name: e.enum_as_string(),
                    },
                });
                ctx.results.insert(current_lib, Err(e));
                self.advance(ctx);
            }
        }
    }

    fn provide_source_file(&mut self, _result: Result<String, samply_symbols::Error>) {
        panic!("symbolicate query never asks for a source file");
    }

    fn provide_binary(&mut self, _result: Result<BinaryImage<FT::F>, samply_symbols::Error>) {
        panic!("symbolicate query never asks for a binary");
    }

    fn provide_file(&mut self, result: Result<FT::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, SymbolicateState::Poisoned);
        let SymbolicateState::AwaitingExternalFile {
            ctx,
            current_lib,
            symbol_map,
            mut address_results,
            current_address,
            current_external,
            mut pending_externals,
            ..
        } = state
        else {
            panic!("provide_file called when not awaiting an external file");
        };
        let file_contents = result.ok();
        let next =
            symbol_map.try_lookup_external_with_file_contents(&current_external, file_contents);
        consume_external_result(
            next,
            current_address,
            &mut address_results,
            &mut pending_externals,
        );
        self.start_next_external(
            ctx,
            current_lib,
            symbol_map,
            address_results,
            pending_externals,
        );
    }

    fn finish(self: Box<Self>) -> QueryApiJsonResult<FT> {
        match self.state {
            SymbolicateState::Done(Ok(response)) => {
                QueryApiJsonResult::SymbolicateResponse(response)
            }
            SymbolicateState::Done(Err(e)) => QueryApiJsonResult::Err(e),
            _ => panic!("SymbolicateApiQueryState::finish called before reaching Done"),
        }
    }
}

fn record_failure<FT: FileTypes>(
    ctx: &mut SymbolicateContext<FT>,
    lib: &Lib,
    e: &samply_symbols::Error,
) {
    ctx.module_stats.push(ModuleStat {
        debug_name: lib.debug_name.to_string(),
        breakpad_id: lib.breakpad_id.to_string(),
        outcome: ModuleLoadOutcome::Failed {
            error_name: e.enum_as_string(),
        },
    });
}

fn gather_requested_addresses(
    request: &request_json::Request,
) -> Result<HashMap<Lib, Vec<u32>>, Error> {
    let mut requested_addresses: HashMap<Lib, Vec<u32>> = HashMap::new();
    for job in request.jobs() {
        let mut requested_addresses_by_module_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for stack in &job.stacks {
            for frame in &stack.0 {
                requested_addresses_by_module_index
                    .entry(frame.module_index)
                    .or_default()
                    .push(frame.address);
            }
        }
        for (module_index, addresses) in requested_addresses_by_module_index {
            let lib = job.memory_map.get(module_index as usize).ok_or(
                Error::ParseRequestErrorContents("Stack frame module index beyond the memoryMap"),
            )?;
            requested_addresses
                .entry((*lib).clone())
                .or_default()
                .extend(addresses);
        }
    }
    Ok(requested_addresses)
}
