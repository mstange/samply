use debugid::DebugId;
use object::{Endianness, FileKind};

use crate::{
    debug_id_for_object,
    macho::{DyldCacheFileData, MachOFatArchiveMemberData},
    shared::{FileContentsWrapper, RangeReadRef},
    Error, FileContents,
};

pub struct BinaryImage<F: FileContents + 'static> {
    file_kind: FileKind,
    inner: BinaryImageInner<F>,
    debug_id: Option<DebugId>,
}

impl<F: FileContents + 'static> BinaryImage<F> {
    pub(crate) fn new(inner: BinaryImageInner<F>, file_kind: FileKind) -> Result<Self, Error> {
        let obj = inner.make_object(file_kind)?;
        let debug_id = debug_id_for_object(&obj);
        Ok(Self {
            file_kind,
            inner,
            debug_id,
        })
    }

    pub fn debug_id(&self) -> Option<DebugId> {
        self.debug_id
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

impl<F: FileContents> BinaryImageInner<F> {
    pub fn make_object(
        &self,
        file_kind: FileKind,
    ) -> Result<object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>>, Error> {
        match self {
            BinaryImageInner::Normal(file) => object::File::parse(file.full_range())
                .map_err(|e| Error::ObjectParseError(file_kind, e)),
            BinaryImageInner::MemberOfFatArchive(MachOFatArchiveMemberData {
                file_data,
                start_offset,
                range_size,
            }) => {
                let range = file_data.range(*start_offset, *range_size);
                object::File::parse(range).map_err(|e| Error::ObjectParseError(file_kind, e))
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

                image.parse_object().map_err(Error::MachOHeaderParseError)
            }
        }
    }
}
