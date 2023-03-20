use debugid::DebugId;
use object::{
    read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile, PeFile32, PeFile64},
    FileKind, Object, ReadRef,
};

use crate::{
    debug_id_for_object,
    debugid_util::code_id_for_object,
    macho::{DyldCacheFileData, MachOData, MachOFatArchiveMemberData, ObjectAndMachOData},
    shared::{FileContentsWrapper, LibraryInfo, PeCodeId, RangeReadRef},
    CodeId, Error, FileContents,
};

pub struct BinaryImage<F: FileContents + 'static> {
    inner: BinaryImageInner<F>,
    info: LibraryInfo,
}

impl<F: FileContents + 'static> BinaryImage<F> {
    pub(crate) fn new(
        inner: BinaryImageInner<F>,
        name: Option<String>,
        path: Option<String>,
    ) -> Result<Self, Error> {
        let info = inner.make_library_info(name, path)?;
        Ok(Self { inner, info })
    }

    pub fn library_info(&self) -> LibraryInfo {
        self.info.clone()
    }

    pub fn debug_name(&self) -> Option<&str> {
        self.info.debug_name.as_deref()
    }

    pub fn debug_id(&self) -> Option<DebugId> {
        self.info.debug_id
    }

    pub fn debug_path(&self) -> Option<&str> {
        self.info.debug_path.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.info.name.as_deref()
    }

    pub fn code_id(&self) -> Option<CodeId> {
        self.info.code_id.clone()
    }

    pub fn path(&self) -> Option<&str> {
        self.info.path.as_deref()
    }

    pub fn arch(&self) -> Option<&str> {
        self.info.arch.as_deref()
    }

    pub fn make_object(&self) -> object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>> {
        self.inner
            .make_object()
            .expect("We already parsed this before, why is it not parsing now?")
    }
}

pub enum BinaryImageInner<F: FileContents + 'static> {
    Normal(FileContentsWrapper<F>, FileKind),
    MemberOfFatArchive(MachOFatArchiveMemberData<F>, FileKind),
    MemberOfDyldSharedCache(DyldCacheFileData<F>),
}

impl<F: FileContents> BinaryImageInner<F> {
    fn make_library_info(
        &self,
        name: Option<String>,
        path: Option<String>,
    ) -> Result<LibraryInfo, Error> {
        let (debug_id, code_id, debug_path, debug_name, arch) = match self {
            BinaryImageInner::Normal(file, file_kind) => {
                let data = file.full_range();
                let object = object::File::parse(data)
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                let debug_id = debug_id_for_object(&object);
                match file_kind {
                    FileKind::Pe32 | FileKind::Pe64 => {
                        let (code_id, debug_path, debug_name) =
                            if let Ok(pe) = PeFile64::parse(file) {
                                pe_info(&pe).into_tuple()
                            } else if let Ok(pe) = PeFile32::parse(file) {
                                pe_info(&pe).into_tuple()
                            } else {
                                (None, None, None)
                            };
                        let arch =
                            object_arch_to_string(object.architecture()).map(ToOwned::to_owned);
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                    FileKind::MachO32 | FileKind::MachO64 => {
                        let macho_data = MachOData::new(file, 0, *file_kind == FileKind::MachO64);
                        let code_id = code_id_for_object(&object);
                        let arch = macho_data.get_arch().map(ToOwned::to_owned);
                        let (debug_path, debug_name) = (path.clone(), name.clone());
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                    _ => {
                        let code_id = code_id_for_object(&object);
                        let (debug_path, debug_name) = (path.clone(), name.clone());
                        let arch =
                            object_arch_to_string(object.architecture()).map(ToOwned::to_owned);
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                }
            }
            BinaryImageInner::MemberOfFatArchive(member, file_kind) => {
                let data = member.data();
                let object = object::File::parse(data)
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                let debug_id = debug_id_for_object(&object);
                let code_id = code_id_for_object(&object);
                let (debug_path, debug_name) = (path.clone(), name.clone());
                let arch = member.arch.clone();
                (debug_id, code_id, debug_path, debug_name, arch)
            }
            BinaryImageInner::MemberOfDyldSharedCache(dyld_cache_file_data) => {
                let ObjectAndMachOData { object, macho_data } =
                    dyld_cache_file_data.make_object()?;
                let debug_id = debug_id_for_object(&object);
                let code_id = code_id_for_object(&object);
                let (debug_path, debug_name) = (path.clone(), name.clone());
                let arch = macho_data.get_arch().map(ToOwned::to_owned);
                (debug_id, code_id, debug_path, debug_name, arch)
            }
        };
        let info = LibraryInfo {
            debug_id,
            debug_name,
            debug_path,
            name,
            code_id,
            path,
            arch,
        };
        Ok(info)
    }

    fn make_object(
        &self,
    ) -> Result<object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>>, Error> {
        match self {
            BinaryImageInner::Normal(file, file_kind) => {
                let obj = object::File::parse(file.full_range())
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                Ok(obj)
            }
            BinaryImageInner::MemberOfFatArchive(member, file_kind) => {
                let obj = object::File::parse(member.data())
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                Ok(obj)
            }
            BinaryImageInner::MemberOfDyldSharedCache(dyld_cache_file_data) => {
                let obj_and_macho_data = dyld_cache_file_data.make_object()?;
                Ok(obj_and_macho_data.object)
            }
        }
    }
}

struct PeInfo {
    code_id: CodeId,
    pdb_path: Option<String>,
    pdb_name: Option<String>,
}

impl PeInfo {
    pub fn into_tuple(self) -> (Option<CodeId>, Option<String>, Option<String>) {
        (Some(self.code_id), self.pdb_path, self.pdb_name)
    }
}

fn pe_info<'a, Pe: ImageNtHeaders, R: ReadRef<'a>>(pe: &PeFile<'a, Pe, R>) -> PeInfo {
    // The code identifier consists of the `time_date_stamp` field id the COFF header, followed by
    // the `size_of_image` field in the optional header. If the optional PE header is not present,
    // this identifier is `None`.
    let header = pe.nt_headers();
    let timestamp = header
        .file_header()
        .time_date_stamp
        .get(object::LittleEndian);
    let image_size = header.optional_header().size_of_image();
    let code_id = CodeId::PeCodeId(PeCodeId {
        timestamp,
        image_size,
    });

    let pdb_path: Option<String> = pe.pdb_info().ok().and_then(|pdb_info| {
        let pdb_path = std::str::from_utf8(pdb_info?.path()).ok()?;
        Some(pdb_path.to_string())
    });

    let pdb_name = pdb_path
        .as_deref()
        .map(|pdb_path| match pdb_path.rsplit_once(['/', '\\']) {
            Some((_base, file_name)) => file_name.to_string(),
            None => pdb_path.to_string(),
        });

    PeInfo {
        code_id,
        pdb_path,
        pdb_name,
    }
}

fn object_arch_to_string(arch: object::Architecture) -> Option<&'static str> {
    let s = match arch {
        object::Architecture::Aarch64 => "arm64",
        object::Architecture::Arm => "arm",
        object::Architecture::I386 => "x86",
        object::Architecture::X86_64 => "x86_64",
        _ => return None,
    };
    Some(s)
}
