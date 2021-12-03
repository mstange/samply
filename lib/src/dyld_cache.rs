use core::slice;
use std::result;

use object::read::ReadRef;
use object::{macho, Endian, Endianness, Pod, U32, U64};

// This file is basically a copy of the corresponding file in the object crate,
// but it has changes for compatibility with macOS 12.
// See https://github.com/gimli-rs/object/issues/358 for upstreaming these changes.

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    #[error("{0}")]
    Generic(&'static str),

    #[error("object read error: {0}")]
    ObjectRead(#[source] object::read::Error),

    #[error("object read error: {0} ({1})")]
    ObjectReadWithContext(&'static str, #[source] object::read::Error),
}

impl From<object::read::Error> for Error {
    fn from(e: object::read::Error) -> Self {
        Error::ObjectRead(e)
    }
}

/// The result type used within the read module.
pub type Result<T> = result::Result<T, Error>;

trait ReadError<T> {
    fn read_error(self, error: &'static str) -> Result<T>;
}

impl<T> ReadError<T> for result::Result<T, ()> {
    fn read_error(self, error: &'static str) -> Result<T> {
        self.map_err(|()| Error::Generic(error))
    }
}

impl<T> ReadError<T> for result::Result<T, object::read::Error> {
    fn read_error(self, context: &'static str) -> Result<T> {
        self.map_err(|error| Error::ObjectReadWithContext(context, error))
    }
}

/// A parsed representation of the dyld shared cache.
#[derive(Debug)]
pub struct DyldCache<'data, E = Endianness, R = &'data [u8]>
where
    E: Endian,
    R: ReadRef<'data>,
{
    endian: E,
    data: R,
    mappings: &'data [macho::DyldCacheMappingInfo<E>],
    images: &'data [macho::DyldCacheImageInfo<E>],
}

impl<'data, E, R> DyldCache<'data, E, R>
where
    E: Endian,
    R: ReadRef<'data>,
{
    /// Parse the raw dyld shared cache data.
    pub fn parse(data: R) -> Result<Self> {
        let header = macho::DyldCacheHeader::parse(data)?;
        let (_arch, endian) = header.parse_magic()?;
        let mappings = header.mappings(endian, data)?;
        let mut images = header.images(endian, data)?;
        if images.is_empty() && header.mapping_offset.get(endian) >= 456 {
            // On macOS 12+, deal with the split chunks format. Each chunk contains the
            // entire list of images, but only a limited set of mappings.
            let header = DyldCacheHeaderMonterey::parse(data)?;
            images = header.images_across_all_chunks(endian, data)?
        };
        Ok(DyldCache {
            endian,
            data,
            mappings,
            images,
        })
    }

    /// Iterate over the images in this cache.
    pub fn images<'cache>(&'cache self) -> DyldCacheImageIterator<'data, 'cache, E, R> {
        DyldCacheImageIterator {
            cache: self,
            iter: self.images.iter(),
        }
    }
}

/// An iterator over all the images (dylibs) in the dyld shared cache.
#[derive(Debug)]
pub struct DyldCacheImageIterator<'data, 'cache, E = Endianness, R = &'data [u8]>
where
    E: Endian,
    R: ReadRef<'data>,
{
    cache: &'cache DyldCache<'data, E, R>,
    iter: slice::Iter<'data, macho::DyldCacheImageInfo<E>>,
}

impl<'data, 'cache, E, R> Iterator for DyldCacheImageIterator<'data, 'cache, E, R>
where
    E: Endian,
    R: ReadRef<'data>,
{
    type Item = DyldCacheImage<'data, E, R>;

    fn next(&mut self) -> Option<DyldCacheImage<'data, E, R>> {
        let image_info = self.iter.next()?;
        Some(DyldCacheImage {
            endian: self.cache.endian,
            data: self.cache.data,
            mappings: self.cache.mappings,
            image_info,
        })
    }
}

/// One image (dylib) from inside the dyld shared cache.
#[derive(Debug)]
pub struct DyldCacheImage<'data, E = Endianness, R = &'data [u8]>
where
    E: Endian,
    R: ReadRef<'data>,
{
    endian: E,
    data: R,
    mappings: &'data [macho::DyldCacheMappingInfo<E>],
    image_info: &'data macho::DyldCacheImageInfo<E>,
}

impl<'data, E, R> DyldCacheImage<'data, E, R>
where
    E: Endian,
    R: ReadRef<'data>,
{
    /// The file system path of this image.
    pub fn path(&self) -> Result<&'data str> {
        let path = self.image_info.path(self.endian, self.data)?;
        // The path should always be ascii, so from_utf8 should alway succeed.
        let path = core::str::from_utf8(path)
            .map_err(|_| Error::Generic("Path string not valid utf-8"))?;
        Ok(path)
    }

    /// The offset in the dyld cache file where this image starts.
    pub fn file_offset(&self) -> Result<u64> {
        Ok(self.image_info.file_offset(self.endian, self.mappings)?)
    }
}

/// The dyld cache header, containing only the fields which are present
/// in all versions of dyld caches (dyld-95.3 and up).
/// Many more fields exist in later dyld versions, but we currently do
/// not need to parse those.
/// Corresponds to struct dyld_cache_header from dyld_cache_format.h.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct DyldCacheHeaderMonterey<E: Endian> {
    /// e.g. "dyld_v0    i386"
    pub magic: [u8; 16],
    /// file offset to first dyld_cache_mapping_info
    pub mapping_offset: U32<E>,
    /// number of dyld_cache_mapping_info entries
    pub mapping_count: U32<E>,
    /// file offset to first dyld_cache_image_info
    pub images_offset: U32<E>,
    /// number of dyld_cache_image_info entries
    pub images_count: U32<E>,
    /// base address of dyld when cache was built
    pub dyld_base_address: U64<E>,
    /// Random fields in between that we don't care about
    reserved: [u8; 408],
    /// file offset to first dyld_cache_image_info
    pub images_across_all_chunks_offset: U32<E>,
    /// number of dyld_cache_image_info entries
    pub images_across_all_chunks_count: U32<E>,
}

unsafe impl<E: Endian> Pod for DyldCacheHeaderMonterey<E> {}

impl<E: Endian> DyldCacheHeaderMonterey<E> {
    /// Read the dyld cache header.
    pub fn parse<'data, R: ReadRef<'data>>(data: R) -> Result<&'data Self> {
        data.read_at::<DyldCacheHeaderMonterey<E>>(0)
            .read_error("Could not read DyldCacheHeaderMonterey")
    }

    /// Return the image information table.
    pub fn images_across_all_chunks<'data, R: ReadRef<'data>>(
        &self,
        endian: E,
        data: R,
    ) -> Result<&'data [macho::DyldCacheImageInfo<E>]> {
        data.read_slice_at::<macho::DyldCacheImageInfo<E>>(
            self.images_across_all_chunks_offset.get(endian).into(),
            self.images_across_all_chunks_count.get(endian) as usize,
        )
        .read_error("Invalid dyld cache image size or alignment")
    }
}
