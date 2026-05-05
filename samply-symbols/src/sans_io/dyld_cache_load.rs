use crate::error::Error;
use crate::macho::DyldCacheFileData;
use crate::sans_io::{LoadStep, NeedsFiles};
use crate::shared::{FileContentsWrapper, FileLoadError, FileLocation, FileTypes};

/// State machine for loading a dyld shared cache and its numeric/`.symbols`
/// subcaches.
pub struct DyldCacheLoad<H: FileTypes> {
    state: DyldCacheLoadState<H>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SuffixAttempt {
    Plain,
    ZeroPadded,
}

enum DyldCacheLoadState<H: FileTypes> {
    /// Awaiting the root cache file. Required.
    AwaitingRoot {
        dyld_cache_path: H::FL,
        dylib_path: String,
        pending: H::FL,
    },
    /// Awaiting the bytes for `.{N}` or `.{NN}` numeric subcache.
    AwaitingNumericSubcache {
        root: FileContentsWrapper<H::F>,
        dyld_cache_path: H::FL,
        dylib_path: String,
        collected: Vec<FileContentsWrapper<H::F>>,
        index: usize,
        attempt: SuffixAttempt,
        pending: H::FL,
    },
    /// Awaiting the optional `.symbols` subcache.
    AwaitingSymbolsSubcache {
        root: FileContentsWrapper<H::F>,
        dyld_cache_path: H::FL,
        dylib_path: String,
        collected: Vec<FileContentsWrapper<H::F>>,
        pending: H::FL,
    },
    Done(Result<DyldCacheFileData<H::F>, Error>),
    Poisoned,
}

impl<H: FileTypes> DyldCacheLoad<H> {
    pub fn new(dyld_cache_path: H::FL, dylib_path: String) -> Self {
        let pending = dyld_cache_path.clone();
        Self {
            state: DyldCacheLoadState::AwaitingRoot {
                dyld_cache_path,
                dylib_path,
                pending,
            },
        }
    }

    pub fn finish(self) -> Result<DyldCacheFileData<H::F>, Error> {
        match self.state {
            DyldCacheLoadState::Done(result) => result,
            _ => panic!("DyldCacheLoad::finish called before reaching Done"),
        }
    }

    fn advance_to_next_subcache(
        &mut self,
        root: FileContentsWrapper<H::F>,
        dyld_cache_path: H::FL,
        dylib_path: String,
        collected: Vec<FileContentsWrapper<H::F>>,
        index: usize,
        mut attempt: SuffixAttempt,
    ) {
        loop {
            let suffix = match attempt {
                SuffixAttempt::Plain => format!(".{index}"),
                SuffixAttempt::ZeroPadded => format!(".{index:02}"),
            };
            match dyld_cache_path.location_for_dyld_subcache(&suffix) {
                Some(pending) => {
                    self.state = DyldCacheLoadState::AwaitingNumericSubcache {
                        root,
                        dyld_cache_path,
                        dylib_path,
                        collected,
                        index,
                        attempt,
                        pending,
                    };
                    return;
                }
                None => match attempt {
                    SuffixAttempt::Plain => {
                        attempt = SuffixAttempt::ZeroPadded;
                    }
                    SuffixAttempt::ZeroPadded => {
                        self.advance_to_symbols_or_finalize(
                            root,
                            dyld_cache_path,
                            dylib_path,
                            collected,
                        );
                        return;
                    }
                },
            }
        }
    }

    fn advance_to_symbols_or_finalize(
        &mut self,
        root: FileContentsWrapper<H::F>,
        dyld_cache_path: H::FL,
        dylib_path: String,
        collected: Vec<FileContentsWrapper<H::F>>,
    ) {
        match dyld_cache_path.location_for_dyld_subcache(".symbols") {
            Some(pending) => {
                self.state = DyldCacheLoadState::AwaitingSymbolsSubcache {
                    root,
                    dyld_cache_path,
                    dylib_path,
                    collected,
                    pending,
                };
            }
            None => {
                self.state = DyldCacheLoadState::Done(Ok(DyldCacheFileData::new(
                    root, collected, dylib_path,
                )));
                let _ = dyld_cache_path;
            }
        }
    }
}

impl<H: FileTypes> NeedsFiles<H> for DyldCacheLoad<H> {
    fn poll(&self) -> LoadStep<'_, H::FL> {
        match &self.state {
            DyldCacheLoadState::AwaitingRoot { pending, .. } => LoadStep::NeedFile {
                location: pending,
                required: true,
            },
            DyldCacheLoadState::AwaitingNumericSubcache { pending, .. }
            | DyldCacheLoadState::AwaitingSymbolsSubcache { pending, .. } => LoadStep::NeedFile {
                location: pending,
                required: false,
            },
            DyldCacheLoadState::Done(_) => LoadStep::Done,
            DyldCacheLoadState::Poisoned => unreachable!("invalid DyldCacheLoad state"),
        }
    }

    fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, DyldCacheLoadState::Poisoned);
        match state {
            DyldCacheLoadState::AwaitingRoot {
                dyld_cache_path,
                dylib_path,
                pending,
            } => match result {
                Ok(file) => {
                    let root = FileContentsWrapper::new(file);
                    self.advance_to_next_subcache(
                        root,
                        dyld_cache_path,
                        dylib_path,
                        Vec::new(),
                        1,
                        SuffixAttempt::Plain,
                    );
                }
                Err(e) => {
                    self.state = DyldCacheLoadState::Done(Err(Error::HelperErrorDuringOpenFile(
                        pending.to_string(),
                        e,
                    )));
                }
            },
            DyldCacheLoadState::AwaitingNumericSubcache {
                root,
                dyld_cache_path,
                dylib_path,
                mut collected,
                index,
                attempt,
                pending: _pending,
            } => match result {
                Ok(file) => {
                    collected.push(FileContentsWrapper::new(file));
                    self.advance_to_next_subcache(
                        root,
                        dyld_cache_path,
                        dylib_path,
                        collected,
                        index + 1,
                        SuffixAttempt::Plain,
                    );
                }
                Err(_) => match attempt {
                    SuffixAttempt::Plain => {
                        self.advance_to_next_subcache(
                            root,
                            dyld_cache_path,
                            dylib_path,
                            collected,
                            index,
                            SuffixAttempt::ZeroPadded,
                        );
                    }
                    SuffixAttempt::ZeroPadded => {
                        self.advance_to_symbols_or_finalize(
                            root,
                            dyld_cache_path,
                            dylib_path,
                            collected,
                        );
                    }
                },
            },
            DyldCacheLoadState::AwaitingSymbolsSubcache {
                root,
                dyld_cache_path,
                dylib_path,
                mut collected,
                pending: _pending,
            } => {
                if let Ok(file) = result {
                    collected.push(FileContentsWrapper::new(file));
                }
                self.state = DyldCacheLoadState::Done(Ok(DyldCacheFileData::new(
                    root, collected, dylib_path,
                )));
                let _ = dyld_cache_path; // intentionally dropped
            }
            DyldCacheLoadState::Done(_) | DyldCacheLoadState::Poisoned => {
                panic!("DyldCacheLoad::provide called when not awaiting a file")
            }
        }
    }
}
