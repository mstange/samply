use std::path::{Path, PathBuf};

use debugid::{CodeId, DebugId};
use object::{
    read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile, PeFile32, PeFile64},
    Endianness, FileKind, Object, ReadRef,
};

use crate::{
    debug_id_for_object,
    debugid_util::code_id_for_object,
    macho::{DyldCacheFileData, MachOFatArchiveMemberData},
    shared::{FileContentsWrapper, LibraryInfo, RangeReadRef},
    Error, FileContents,
};

pub struct BinaryImage<F: FileContents + 'static> {
    file_kind: FileKind,
    inner: BinaryImageInner<F>,
    info: LibraryInfo,
}

impl<F: FileContents + 'static> BinaryImage<F> {
    pub(crate) fn new(
        inner: BinaryImageInner<F>,
        path: Option<&Path>,
        file_kind: FileKind,
    ) -> Result<Self, Error> {
        let obj_and_data = inner.make_object_and_data(file_kind)?;
        let path = path.map(|p| p.canonicalize().ok().unwrap_or_else(|| p.into()));
        let name = path
            .as_deref()
            .and_then(|p| Some(p.file_name()?.to_str()?.to_owned()));
        let path = path.and_then(|p| Some(p.to_str()?.to_owned()));
        let debug_id = debug_id_for_object(&obj_and_data.obj);
        let (code_id, debug_path, debug_name) = if let FileKind::Pe32 | FileKind::Pe64 = file_kind {
            if let Ok(pe) = PeFile64::parse(obj_and_data.data) {
                pe_info(&pe).into_tuple()
            } else if let Ok(pe) = PeFile32::parse(obj_and_data.data) {
                pe_info(&pe).into_tuple()
            } else {
                (None, None, None)
            }
        } else {
            let code_id = code_id_for_object(&obj_and_data.obj);
            (code_id, path.clone(), name.clone())
        };
        let info = LibraryInfo {
            debug_id,
            debug_name,
            debug_path,
            name,
            code_id,
            path,
            arch: None,
        };
        Ok(Self {
            file_kind,
            inner,
            info,
        })
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
            .make_object(self.file_kind)
            .expect("We already parsed this before, why is it not parsing now?")
    }
}

pub enum BinaryImageInner<F: FileContents + 'static> {
    Normal(FileContentsWrapper<F>),
    MemberOfFatArchive(MachOFatArchiveMemberData<F>),
    MemberOfDyldSharedCache(DyldCacheFileData<F>),
}

struct ObjectAndData<'a, F: FileContents> {
    obj: object::File<'a, RangeReadRef<'a, &'a FileContentsWrapper<F>>>,
    data: RangeReadRef<'a, &'a FileContentsWrapper<F>>,
}

impl<F: FileContents> BinaryImageInner<F> {
    pub fn make_object(
        &self,
        file_kind: FileKind,
    ) -> Result<object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>>, Error> {
        let ObjectAndData { obj, .. } = self.make_object_and_data(file_kind)?;
        Ok(obj)
    }

    fn make_object_and_data(&self, file_kind: FileKind) -> Result<ObjectAndData<'_, F>, Error> {
        match self {
            BinaryImageInner::Normal(file) => {
                let data = file.full_range();
                let obj =
                    object::File::parse(data).map_err(|e| Error::ObjectParseError(file_kind, e))?;
                Ok(ObjectAndData { obj, data })
            }
            BinaryImageInner::MemberOfFatArchive(member) => {
                let data = member.data();
                let obj =
                    object::File::parse(data).map_err(|e| Error::ObjectParseError(file_kind, e))?;
                Ok(ObjectAndData { obj, data })
            }
            BinaryImageInner::MemberOfDyldSharedCache(DyldCacheFileData {
                root_file_data,
                subcache_file_data,
                dylib_path,
            }) => {
                let rootcache_range = root_file_data.full_range();
                let subcache_ranges: Vec<_> = subcache_file_data
                    .iter()
                    .map(FileContentsWrapper::full_range)
                    .collect();
                let cache = object::read::macho::DyldCache::<Endianness, _>::parse(
                    rootcache_range,
                    &subcache_ranges,
                )
                .map_err(Error::DyldCacheParseError)?;

                let image = match cache.images().find(|image| image.path() == Ok(dylib_path)) {
                    Some(image) => image,
                    None => {
                        return Err(Error::NoMatchingDyldCacheImagePath(dylib_path.to_string()))
                    }
                };
                let (data, _header_offset) = image
                    .image_data_and_offset()
                    .map_err(Error::DyldCacheParseError)?;

                let obj = image.parse_object().map_err(Error::MachOHeaderParseError)?;
                Ok(ObjectAndData { obj, data })
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
    let size_of_image = header.optional_header().size_of_image();
    let code_id = CodeId::new(format!("{:08X}{:x}", timestamp, size_of_image));

    let pdb_path: Option<PathBuf> = match pe.pdb_info() {
        Ok(Some(pdb_info)) => {
            let pdb_path = std::str::from_utf8(pdb_info.path()).ok();
            pdb_path.map(Into::into)
        }
        _ => None,
    };
    let pdb_name = pdb_path
        .as_deref()
        .and_then(|p| Some(p.file_name()?.to_str()?.to_owned()));
    let pdb_path = pdb_path.and_then(|p| Some(p.to_str()?.to_owned()));

    PeInfo {
        code_id,
        pdb_path,
        pdb_name,
    }
}
