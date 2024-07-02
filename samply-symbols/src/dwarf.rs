use std::marker::PhantomData;

use addr2line::{fallible_iterator, gimli};
use elsa::sync::FrozenVec;
use fallible_iterator::FallibleIterator;
use gimli::{DwarfPackage, EndianSlice, Reader, RunTimeEndian, SectionId};
use object::read::ReadRef;
use object::CompressionFormat;

use crate::path_mapper::PathMapper;
use crate::shared::FrameDebugInfo;
use crate::{demangle, Error, SourceFilePath};

pub fn get_frames<R: Reader>(
    address: u64,
    context: Option<&addr2line::Context<R>>,
    path_mapper: &mut PathMapper<()>,
) -> Option<Vec<FrameDebugInfo>> {
    let frame_iter = context?.find_frames(address).skip_all_loads().ok()?;
    convert_frames(frame_iter, path_mapper)
}

pub fn convert_frames<'a, R: gimli::Reader>(
    frame_iter: impl FallibleIterator<Item = addr2line::Frame<'a, R>>,
    path_mapper: &mut PathMapper<()>,
) -> Option<Vec<FrameDebugInfo>> {
    let frames: Vec<_> = frame_iter
        .map(|f| Ok(convert_stack_frame(f, &mut *path_mapper)))
        .collect()
        .ok()?;

    if frames.is_empty() {
        None
    } else {
        Some(frames)
    }
}

pub fn convert_stack_frame<R: gimli::Reader>(
    frame: addr2line::Frame<R>,
    path_mapper: &mut PathMapper<()>,
) -> FrameDebugInfo {
    let function = match frame.function {
        Some(function_name) => {
            if let Ok(name) = function_name.raw_name() {
                Some(demangle::demangle_any(&name))
            } else {
                None
            }
        }
        None => None,
    };
    let file_path = frame.location.as_ref().and_then(|l| l.file).map(|file| {
        let mapped_path = path_mapper.map_path(file);
        SourceFilePath::new(file.into(), mapped_path)
    });

    FrameDebugInfo {
        function,
        file_path,
        line_number: frame.location.and_then(|l| l.line),
    }
}

pub enum SingleSectionData<'data, T: ReadRef<'data>> {
    View {
        data: T,
        offset: u64,
        size: u64,
        _phantom: PhantomData<&'data ()>,
    },
    Owned(Vec<u8>),
}

pub fn try_get_section_data<'data, O, T>(
    data: T,
    file: &O,
    section_id: SectionId,
    is_for_dwo_dwp: bool,
) -> Option<SingleSectionData<'data, T>>
where
    O: object::Object<'data>,
    T: ReadRef<'data>,
{
    use object::ObjectSection;
    let section_name = if is_for_dwo_dwp {
        section_id.dwo_name()?
    } else {
        section_id.name()
    };
    let section = file.section_by_name(section_name)?;
    let file_range = section.compressed_file_range().ok()?;
    match file_range.format {
        CompressionFormat::None => Some(SingleSectionData::View {
            data,
            offset: file_range.offset,
            size: file_range.uncompressed_size,
            _phantom: PhantomData,
        }),
        CompressionFormat::Zlib => {
            let compressed_bytes = data
                .read_bytes_at(file_range.offset, file_range.compressed_size)
                .ok()?;

            let mut decompressed = Vec::with_capacity(file_range.uncompressed_size as usize);
            let mut decompress = flate2::Decompress::new(true);
            decompress
                .decompress_vec(
                    compressed_bytes,
                    &mut decompressed,
                    flate2::FlushDecompress::Finish,
                )
                .ok()?;

            return Some(SingleSectionData::Owned(decompressed));
        }
        _ => None,
    }
}

/// Holds on to section data so that we can create an addr2line::Context for that
/// that data. This avoids one copy compared to what addr2line::Context::new does
/// by default, saving 1.5 seconds on libxul. (For comparison, dumping all symbols
/// from libxul takes 200ms in total.)
/// See addr2line::Context::new for details.
pub struct Addr2lineContextData {
    uncompressed_section_data: FrozenVec<Vec<u8>>,
}

impl Addr2lineContextData {
    pub fn new() -> Self {
        Self {
            uncompressed_section_data: FrozenVec::new(),
        }
    }

    fn sect<'data, 'ctxdata, O, R>(
        &'ctxdata self,
        data: R,
        obj: &O,
        section_id: SectionId,
        endian: RunTimeEndian,
        is_for_dwo_dwp: bool,
    ) -> EndianSlice<'ctxdata, RunTimeEndian>
    where
        'data: 'ctxdata,
        O: object::Object<'data>,
        R: ReadRef<'data>,
    {
        let slice: &[u8] = match try_get_section_data(data, obj, section_id, is_for_dwo_dwp) {
            Some(SingleSectionData::Owned(section_data)) => {
                self.uncompressed_section_data.push_get(section_data)
            }
            Some(SingleSectionData::View {
                data, offset, size, ..
            }) => data.read_bytes_at(offset, size).unwrap_or(&[]),
            None => &[],
        };
        EndianSlice::new(slice, endian)
    }

    pub fn make_context<'data, 'ctxdata, O, R>(
        &'ctxdata self,
        data: R,
        obj: &O,
        sup_data: Option<R>,
        sup_obj: Option<&O>,
    ) -> Result<addr2line::Context<EndianSlice<'ctxdata, RunTimeEndian>>, Error>
    where
        'data: 'ctxdata,
        O: object::Object<'data>,
        R: ReadRef<'data>,
    {
        let e = if obj.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        let mut dwarf = gimli::Dwarf::load(|s| Ok(self.sect(data, obj, s, e, false)))
            .map_err(Error::Addr2lineContextCreationError)?;
        if let (Some(sup_obj), Some(sup_data)) = (sup_obj, sup_data) {
            dwarf
                .load_sup(|s| Ok(self.sect(sup_data, sup_obj, s, e, false)))
                .map_err(Error::Addr2lineContextCreationError)?;
        }
        let context =
            addr2line::Context::from_dwarf(dwarf).map_err(Error::Addr2lineContextCreationError)?;
        Ok(context)
    }

    pub fn make_package<'data, 'ctxdata, O, R>(
        &'ctxdata self,
        data: R,
        obj: &O,
        dwp_data: Option<R>,
        dwp_obj: Option<&O>,
    ) -> Result<Option<DwarfPackage<EndianSlice<'ctxdata, RunTimeEndian>>>, Error>
    where
        'data: 'ctxdata,
        O: object::Object<'data>,
        R: ReadRef<'data>,
    {
        let e = if obj.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        let mut package = None;
        if let (Some(dwp_obj), Some(dwp_data)) = (dwp_obj, dwp_data) {
            package = DwarfPackage::load::<_, gimli::Error>(
                |s| Ok(self.sect(dwp_data, dwp_obj, s, e, true)),
                EndianSlice::new(&[], e),
            )
            .ok();
        }
        if package.is_none() && obj.section_by_name(".debug_cu_index").is_some() {
            package = DwarfPackage::load::<_, gimli::Error>(
                |s| Ok(self.sect(data, obj, s, e, true)),
                EndianSlice::new(&[], e),
            )
            .ok();
        }
        Ok(package)
    }

    pub fn make_dwarf_for_dwo<'data, 'ctxdata, O, R>(
        &'ctxdata self,
        data: R,
        obj: &O,
    ) -> Result<addr2line::gimli::Dwarf<EndianSlice<'ctxdata, RunTimeEndian>>, Error>
    where
        'data: 'ctxdata,
        O: object::Object<'data>,
        R: ReadRef<'data>,
    {
        let e = if obj.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        let dwarf = gimli::Dwarf::load(|s| Ok(self.sect(data, obj, s, e, true)))
            .map_err(Error::Addr2lineContextCreationError)?;
        Ok(dwarf)
    }
}
