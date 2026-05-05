use linux_perf_data::jitdump::JitDumpReader;
use object::FileKind;

use crate::binary_image::{BinaryImage, BinaryImageInner};
use crate::error::Error;
use crate::jitdump::{self, JitDumpIndex};
use crate::macho;
use crate::sans_io::dyld_cache_load::DyldCacheLoad;
use crate::sans_io::{LoadStep, NeedsFiles};
use crate::shared::{
    FileContentsCursor, FileContentsWrapper, FileLoadError, FileTypes, MultiArchDisambiguator,
};

/// State machine for `SymbolManager::load_binary*`. Loads a single primary file
/// (or a dyld cache + subcaches) and produces a [`BinaryImage`].
pub struct LoadBinary<H: FileTypes> {
    state: LoadBinaryState<H>,
}

enum LoadBinaryState<H: FileTypes> {
    AwaitingPrimary {
        file_location: H::FL,
        name: Option<String>,
        path: Option<String>,
        disambiguator: Option<MultiArchDisambiguator>,
        pending: H::FL,
    },
    DyldCache {
        sm: DyldCacheLoad<H>,
        dylib_path: String,
    },
    Done(Result<BinaryImage<H::F>, Error>),
    Poisoned,
}

impl<H: FileTypes> LoadBinary<H> {
    /// Construct a state machine for loading a binary from a single file path.
    pub fn new(
        file_location: H::FL,
        name: Option<String>,
        path: Option<String>,
        disambiguator: Option<MultiArchDisambiguator>,
    ) -> Self {
        let pending = file_location.clone();
        Self {
            state: LoadBinaryState::AwaitingPrimary {
                file_location,
                name,
                path,
                disambiguator,
                pending,
            },
        }
    }

    /// Construct a state machine for loading a binary that lives inside a dyld
    /// shared cache.
    pub fn for_dyld_cache(dyld_cache_path: H::FL, dylib_path: String) -> Self {
        let sm = DyldCacheLoad::<H>::new(dyld_cache_path, dylib_path.clone());
        Self {
            state: LoadBinaryState::DyldCache { sm, dylib_path },
        }
    }

    pub fn finish(self) -> Result<BinaryImage<H::F>, Error> {
        match self.state {
            LoadBinaryState::Done(result) => result,
            _ => panic!("LoadBinary::finish called before reaching Done"),
        }
    }
}

impl<H: FileTypes> NeedsFiles<H> for LoadBinary<H> {
    fn poll(&self) -> LoadStep<'_, H::FL> {
        match &self.state {
            LoadBinaryState::AwaitingPrimary { pending, .. } => LoadStep::NeedFile {
                location: pending,
                required: true,
            },
            LoadBinaryState::DyldCache { sm, .. } => sm.poll(),
            LoadBinaryState::Done(_) => LoadStep::Done,
            LoadBinaryState::Poisoned => unreachable!("invalid LoadBinary state"),
        }
    }

    fn provide(&mut self, result: Result<H::F, FileLoadError>) {
        let state = std::mem::replace(&mut self.state, LoadBinaryState::Poisoned);
        match state {
            LoadBinaryState::AwaitingPrimary {
                file_location,
                name,
                path,
                disambiguator,
                pending,
            } => {
                let file = match result {
                    Ok(f) => f,
                    Err(e) => {
                        self.state = LoadBinaryState::Done(Err(Error::HelperErrorDuringOpenFile(
                            pending.to_string(),
                            e,
                        )));
                        return;
                    }
                };
                let _ = file_location;
                self.state = LoadBinaryState::Done(build_binary_image::<H>(
                    FileContentsWrapper::new(file),
                    name,
                    path,
                    disambiguator,
                ));
            }
            LoadBinaryState::DyldCache { mut sm, dylib_path } => {
                sm.provide(result);
                match sm.poll() {
                    LoadStep::Done => {
                        self.state = LoadBinaryState::Done(sm.finish().and_then(|file_data| {
                            let inner = BinaryImageInner::MemberOfDyldSharedCache(file_data);
                            let name = match dylib_path.rfind('/') {
                                Some(idx) => dylib_path[idx + 1..].to_owned(),
                                None => dylib_path.clone(),
                            };
                            BinaryImage::new(inner, Some(name), Some(dylib_path))
                        }));
                    }
                    _ => {
                        self.state = LoadBinaryState::DyldCache { sm, dylib_path };
                    }
                }
            }
            LoadBinaryState::Done(_) | LoadBinaryState::Poisoned => {
                panic!("LoadBinary::provide called when not awaiting a file")
            }
        }
    }
}

fn build_binary_image<H: FileTypes>(
    file_contents: FileContentsWrapper<H::F>,
    name: Option<String>,
    path: Option<String>,
    disambiguator: Option<MultiArchDisambiguator>,
) -> Result<BinaryImage<H::F>, Error> {
    let file_kind = match FileKind::parse(&file_contents) {
        Ok(file_kind) => file_kind,
        Err(_) if jitdump::is_jitdump_file(&file_contents) => {
            let cursor = FileContentsCursor::new(&file_contents);
            let reader = JitDumpReader::new(cursor)?;
            let index = JitDumpIndex::from_reader(reader).map_err(Error::JitDumpFileReading)?;
            let inner = BinaryImageInner::JitDump(file_contents, index);
            return BinaryImage::new(inner, name, path);
        }
        Err(_) => {
            return Err(Error::InvalidInputError("Unrecognized file"));
        }
    };
    let inner = match file_kind {
        FileKind::Elf32
        | FileKind::Elf64
        | FileKind::MachO32
        | FileKind::MachO64
        | FileKind::Pe32
        | FileKind::Pe64 => BinaryImageInner::Normal(file_contents, file_kind),
        FileKind::MachOFat32 | FileKind::MachOFat64 => {
            let member = macho::get_fat_archive_member(&file_contents, file_kind, disambiguator)?;
            let (offset, size) = member.offset_and_size;
            let arch = member.arch;
            let data = macho::MachOFatArchiveMemberData::new(file_contents, offset, size, arch);
            BinaryImageInner::MemberOfFatArchive(data, file_kind)
        }
        _ => {
            return Err(Error::InvalidInputError(
                "Input was Archive, Coff or Wasm format, which are unsupported for now",
            ))
        }
    };
    BinaryImage::new(inner, name, path)
}
