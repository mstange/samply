use crate::error::Error;
use crate::external_file::ExternalFileSymbolMap;
use crate::sans_io::{LoadStep, NeedsFiles};
use crate::shared::{FileContents, FileLoadError, FileLocation, FileTypes};
use crate::source_file_path::SourceFilePath;

/// State machine that fetches a single source file and decodes it into a `String`.
///
/// Mirrors `SymbolManager::load_source_file` without performing any I/O.
pub struct LoadSourceFile<H: FileTypes> {
    state: LoadSourceFileState<H>,
}

enum LoadSourceFileState<H: FileTypes> {
    NeedFile { location: H::FL },
    Done(Result<String, Error>),
    Poisoned,
}

impl<H: FileTypes> LoadSourceFile<H> {
    pub fn new(
        debug_file_location: &H::FL,
        source_file_path: &SourceFilePath<'_>,
    ) -> Result<Self, Error> {
        let location = debug_file_location
            .location_for_source_file(source_file_path.raw_path())
            .ok_or(Error::FileLocationRefusedSourceFileLocation)?;
        Ok(Self {
            state: LoadSourceFileState::NeedFile { location },
        })
    }

    pub fn finish(self) -> Result<String, Error> {
        match self.state {
            LoadSourceFileState::Done(result) => result,
            _ => panic!("LoadSourceFile::finish called before reaching Done"),
        }
    }
}

impl<H: FileTypes> NeedsFiles<H> for LoadSourceFile<H> {
    fn poll(&self) -> LoadStep<'_, H::FL> {
        match &self.state {
            LoadSourceFileState::NeedFile { location } => LoadStep::NeedFile {
                location,
                required: true,
            },
            LoadSourceFileState::Done(_) => LoadStep::Done,
            LoadSourceFileState::Poisoned => unreachable!("invalid LoadSourceFile state"),
        }
    }

    fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let location = match std::mem::replace(&mut self.state, LoadSourceFileState::Poisoned) {
            LoadSourceFileState::NeedFile { location } => location,
            _ => panic!("LoadSourceFile::provide called when not awaiting a file"),
        };
        let decoded = match result {
            Ok(file) => decode_source_file(&location, file),
            Err(e) => Err(Error::HelperErrorDuringOpenFile(location.to_string(), e)),
        };
        self.state = LoadSourceFileState::Done(decoded);
    }
}

fn decode_source_file<FL: FileLocation, F: FileContents>(
    location: &FL,
    file: F,
) -> Result<String, Error> {
    let len = file.len();
    let bytes = file
        .read_bytes_at(0, len)
        .map_err(|e| Error::HelperErrorDuringFileReading(location.to_string(), e))?;
    Ok(String::from_utf8_lossy(bytes).to_string())
}

/// State machine that fetches a single external object file and parses it into
/// an `ExternalFileSymbolMap`.
///
/// Mirrors `SymbolManager::load_external_file` without performing any I/O.
pub struct LoadExternalFile<H: FileTypes> {
    external_file_path: String,
    state: LoadExternalFileState<H>,
}

enum LoadExternalFileState<H: FileTypes> {
    NeedFile { location: H::FL },
    Done(Result<ExternalFileSymbolMap<H::F>, Error>),
    Poisoned,
}

impl<H: FileTypes> LoadExternalFile<H> {
    pub fn new(debug_file_location: &H::FL, external_file_path: &str) -> Result<Self, Error> {
        let location = debug_file_location
            .location_for_external_object_file(external_file_path)
            .ok_or(Error::FileLocationRefusedExternalObjectLocation)?;
        Ok(Self {
            external_file_path: external_file_path.to_owned(),
            state: LoadExternalFileState::NeedFile { location },
        })
    }

    pub fn finish(self) -> Result<ExternalFileSymbolMap<H::F>, Error> {
        match self.state {
            LoadExternalFileState::Done(result) => result,
            _ => panic!("LoadExternalFile::finish called before reaching Done"),
        }
    }
}

impl<H: FileTypes> NeedsFiles<H> for LoadExternalFile<H> {
    fn poll(&self) -> LoadStep<'_, H::FL> {
        match &self.state {
            LoadExternalFileState::NeedFile { location } => LoadStep::NeedFile {
                location,
                required: true,
            },
            LoadExternalFileState::Done(_) => LoadStep::Done,
            LoadExternalFileState::Poisoned => unreachable!("invalid LoadExternalFile state"),
        }
    }

    fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let _location = match std::mem::replace(&mut self.state, LoadExternalFileState::Poisoned) {
            LoadExternalFileState::NeedFile { location } => location,
            _ => panic!("LoadExternalFile::provide called when not awaiting a file"),
        };
        let parsed = match result {
            Ok(file) => ExternalFileSymbolMap::new(&self.external_file_path, file),
            Err(e) => Err(Error::HelperErrorDuringOpenFile(
                self.external_file_path.clone(),
                e,
            )),
        };
        self.state = LoadExternalFileState::Done(parsed);
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::shared::FileTypes;

    /// A `FileTypes` that never actually does any I/O — its only
    /// purpose is to give `LoadSourceFile` something to be generic over so
    /// that the sans-IO loop can be driven from a synchronous test.
    struct UnusedHelper;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct StringPath(String);

    impl std::fmt::Display for StringPath {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl FileLocation for StringPath {
        fn location_for_dyld_subcache(&self, _: &str) -> Option<Self> {
            None
        }
        fn location_for_external_object_file(&self, p: &str) -> Option<Self> {
            Some(StringPath(p.to_owned()))
        }
        fn location_for_pdb_from_binary(&self, p: &str) -> Option<Self> {
            Some(StringPath(p.to_owned()))
        }
        fn location_for_source_file(&self, p: &str) -> Option<Self> {
            Some(StringPath(p.to_owned()))
        }
        fn location_for_breakpad_symindex(&self) -> Option<Self> {
            None
        }
        fn location_for_dwo(&self, _: &str, _: &str) -> Option<Self> {
            None
        }
        fn location_for_dwp(&self) -> Option<Self> {
            None
        }
    }

    impl FileTypes for UnusedHelper {
        type F = Vec<u8>;
        type FL = StringPath;
    }

    #[test]
    fn drives_synchronously_without_async_runtime() {
        let debug_loc = StringPath("/some/debug/file".to_owned());
        let src_path = SourceFilePath::RawPath(Cow::Borrowed("/source/file.rs"));
        let mut sm = LoadSourceFile::<UnusedHelper>::new(&debug_loc, &src_path).unwrap();

        // First poll: a NeedFile with the resolved location.
        match sm.poll() {
            LoadStep::NeedFile { location, required } => {
                assert!(required);
                assert_eq!(location.0, "/source/file.rs");
            }
            _ => panic!("expected NeedFile"),
        }

        // Synthesize file contents — no async involved.
        sm.provide(Ok(b"hello sans-io\n".to_vec()));

        // Second poll: Done.
        assert!(matches!(sm.poll(), LoadStep::Done));
        assert_eq!(sm.finish().unwrap(), "hello sans-io\n");
    }

    #[test]
    fn surfaces_helper_error_on_required_fetch() {
        let debug_loc = StringPath("/some/debug/file".to_owned());
        let src_path = SourceFilePath::RawPath(Cow::Borrowed("/source/missing.rs"));
        let mut sm = LoadSourceFile::<UnusedHelper>::new(&debug_loc, &src_path).unwrap();

        assert!(matches!(sm.poll(), LoadStep::NeedFile { .. }));
        sm.provide(Err("nope".into()));
        assert!(matches!(sm.poll(), LoadStep::Done));
        let err = sm.finish().unwrap_err();
        assert!(matches!(err, Error::HelperErrorDuringOpenFile(_, _)));
    }
}
