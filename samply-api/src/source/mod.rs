use samply_symbols::{
    BinaryImage, ExternalFileAddressRef, ExternalFileRef, FileLoadError, FileLocation, FileTypes,
    FrameDebugInfo, FramesLookupResult, LibraryInfo, LookupAddress, SymbolMap,
};

use crate::api_file_path::to_api_file_path;
use crate::query_state::{ApiQueryState, ApiStep};
use crate::{to_debug_id, Error, QueryApiJsonResult};

pub mod request_json;
pub mod response_json;

#[derive(thiserror::Error, Debug)]
pub enum SourceError {
    #[error("Could not obtain symbols for the requested library: {0}")]
    NoSymbols(#[from] samply_symbols::Error),

    #[error("Don't have any debug info for the requested library")]
    NoDebugInfo,

    #[error("The requested path is not present in the symbolication frames")]
    InvalidPath,

    #[error("An error occurred when reading the file: {0}")]
    FileLoadError(#[from] FileLoadError),
}

/// Sans-IO state machine implementation of `/source/v1`.
///
/// External file references (`FramesLookupResult::External`) are chased via
/// [`ApiStep::NeedFile`] requests, so the macOS OSO-stab and ELF dwo
/// workflows are fully supported.
pub struct SourceApiQueryState<H: FileTypes> {
    state: SourceState<H>,
}

enum SourceState<H: FileTypes> {
    AwaitingSymbolMap {
        request: request_json::Request,
        library_info: LibraryInfo,
    },
    /// Chasing one or more `FramesLookupResult::External` references for the
    /// single requested address. Each `provide_file` call advances the chain
    /// until we reach an `Available` (or until the chain dead-ends).
    AwaitingExternalFile {
        request: request_json::Request,
        symbol_map: SymbolMap<H>,
        external: ExternalFileAddressRef,
        location: H::FL,
    },
    AwaitingSourceFile {
        requested_file: String,
        debug_file_location: H::FL,
        source_file_path_raw: String,
    },
    Done(Result<response_json::Response, SourceError>),
    Poisoned,
}

impl<H: FileTypes> SourceApiQueryState<H> {
    pub fn from_request_json(request_json: &str) -> Result<Self, Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        Ok(Self::new(request))
    }

    pub fn new(request: request_json::Request) -> Self {
        let debug_id = match to_debug_id(&request.debug_id) {
            Ok(id) => id,
            Err(e) => {
                return Self {
                    state: SourceState::Done(Err(SourceError::NoSymbols(e))),
                };
            }
        };
        let library_info = LibraryInfo {
            debug_name: Some(request.debug_name.clone()),
            debug_id: Some(debug_id),
            ..Default::default()
        };
        Self {
            state: SourceState::AwaitingSymbolMap {
                request,
                library_info,
            },
        }
    }

    /// After we've resolved a `FramesLookupResult::Available` (or the chain
    /// dead-ended), look for the requested file in the resulting frames and
    /// transition to `AwaitingSourceFile` (or fail with the appropriate
    /// error).
    fn finalize_with_frames(
        &mut self,
        request: request_json::Request,
        symbol_map: SymbolMap<H>,
        frames: Option<Vec<FrameDebugInfo>>,
    ) {
        let Some(frames) = frames else {
            self.state = SourceState::Done(Err(SourceError::NoDebugInfo));
            return;
        };
        let debug_file_location = symbol_map.debug_file_location().clone();
        let source_file_path = frames
            .into_iter()
            .filter_map(|frame| frame.file_path)
            .map(|path| symbol_map.resolve_source_file_path(path))
            .find(|file_path| to_api_file_path(file_path) == request.file);
        let Some(source_file_path) = source_file_path else {
            self.state = SourceState::Done(Err(SourceError::InvalidPath));
            return;
        };
        let source_file_path_raw = source_file_path.raw_path().to_owned();
        self.state = SourceState::AwaitingSourceFile {
            requested_file: request.file,
            debug_file_location,
            source_file_path_raw,
        };
    }

    /// Given an `ExternalFileAddressRef`, compute its `FileLocation` (or
    /// finish the lookup with `NoDebugInfo` if no location can be resolved).
    fn enqueue_external(
        &mut self,
        request: request_json::Request,
        symbol_map: SymbolMap<H>,
        external: ExternalFileAddressRef,
    ) {
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
                self.state = SourceState::AwaitingExternalFile {
                    request,
                    symbol_map,
                    external,
                    location,
                };
            }
            None => {
                // We can't fetch this file. Try one last lookup with no new
                // contents (it might already be cached); otherwise treat as
                // no debug info.
                let next = symbol_map.try_lookup_external_with_file_contents(&external, None);
                self.process_external_result(request, symbol_map, next);
            }
        }
    }

    fn process_external_result(
        &mut self,
        request: request_json::Request,
        symbol_map: SymbolMap<H>,
        result: Option<FramesLookupResult>,
    ) {
        match result {
            Some(FramesLookupResult::Available(frames)) => {
                self.finalize_with_frames(request, symbol_map, Some(frames));
            }
            Some(FramesLookupResult::External(new_external)) => {
                self.enqueue_external(request, symbol_map, new_external);
            }
            None => {
                self.finalize_with_frames(request, symbol_map, None);
            }
        }
    }
}

impl<H: FileTypes> ApiQueryState<H> for SourceApiQueryState<H> {
    fn poll(&self) -> ApiStep<H> {
        match &self.state {
            SourceState::AwaitingSymbolMap { library_info, .. } => {
                ApiStep::NeedSymbolMap(library_info.clone())
            }
            SourceState::AwaitingExternalFile { location, .. } => ApiStep::NeedFile {
                location: location.clone(),
                required: false,
            },
            SourceState::AwaitingSourceFile {
                debug_file_location,
                source_file_path_raw,
                ..
            } => ApiStep::NeedSourceFile {
                debug_file: debug_file_location.clone(),
                source_file_path: source_file_path_raw.clone(),
            },
            SourceState::Done(_) => ApiStep::Done,
            SourceState::Poisoned => unreachable!("invalid SourceApiQueryState state"),
        }
    }

    fn provide_symbol_map(&mut self, result: Result<SymbolMap<H>, samply_symbols::Error>) {
        let state = std::mem::replace(&mut self.state, SourceState::Poisoned);
        let SourceState::AwaitingSymbolMap { request, .. } = state else {
            panic!("provide_symbol_map called when not awaiting a symbol map");
        };
        let symbol_map = match result {
            Ok(sm) => sm,
            Err(e) => {
                self.state = SourceState::Done(Err(SourceError::NoSymbols(e)));
                return;
            }
        };
        let address_info = symbol_map.lookup_sync(LookupAddress::Relative(request.module_offset));
        let frames_result = address_info.and_then(|ai| ai.frames);
        match frames_result {
            Some(FramesLookupResult::Available(frames)) => {
                self.finalize_with_frames(request, symbol_map, Some(frames));
            }
            Some(FramesLookupResult::External(external)) => {
                self.enqueue_external(request, symbol_map, external);
            }
            None => {
                self.state = SourceState::Done(Err(SourceError::NoDebugInfo));
            }
        }
    }

    fn provide_source_file(&mut self, result: Result<String, samply_symbols::Error>) {
        let state = std::mem::replace(&mut self.state, SourceState::Poisoned);
        let SourceState::AwaitingSourceFile { requested_file, .. } = state else {
            panic!("provide_source_file called when not awaiting a source file");
        };
        self.state = SourceState::Done(match result {
            Ok(source) => Ok(response_json::Response {
                symbols_last_modified: None,
                source_last_modified: None,
                file: requested_file,
                source,
            }),
            Err(e) => Err(SourceError::NoSymbols(e)),
        });
    }

    fn provide_binary(&mut self, _result: Result<BinaryImage<H::F>, samply_symbols::Error>) {
        panic!("source query never asks for a binary");
    }

    fn provide_file(&mut self, result: Result<H::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, SourceState::Poisoned);
        let SourceState::AwaitingExternalFile {
            request,
            symbol_map,
            external,
            ..
        } = state
        else {
            panic!("provide_file called when not awaiting an external file");
        };
        let file_contents = result.ok();
        let next = symbol_map.try_lookup_external_with_file_contents(&external, file_contents);
        self.process_external_result(request, symbol_map, next);
    }

    fn finish(self: Box<Self>) -> QueryApiJsonResult<H> {
        match self.state {
            SourceState::Done(Ok(response)) => QueryApiJsonResult::SourceResponse(response),
            SourceState::Done(Err(e)) => QueryApiJsonResult::Err(Error::Source(e)),
            _ => panic!("SourceApiQueryState::finish called before reaching Done"),
        }
    }
}
