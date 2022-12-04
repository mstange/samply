use std::marker::PhantomData;

use crate::path_mapper::PathMapper;
use crate::shared::InlineStackFrame;
use crate::{demangle, Error};
use addr2line::fallible_iterator;
use addr2line::gimli;
use elsa::sync::FrozenVec;
use fallible_iterator::FallibleIterator;
use gimli::{EndianSlice, Reader, RunTimeEndian, SectionId};
use object::read::ReadRef;
use object::{CompressedFileRange, CompressionFormat};

pub fn get_frames<R: Reader>(
    address: u64,
    context: Option<&addr2line::Context<R>>,
    path_mapper: &mut PathMapper<()>,
) -> Option<Vec<InlineStackFrame>> {
    context
        .and_then(|context| context.find_frames(address).ok())
        .and_then(|frame_iter| {
            frame_iter
                .map(|f| Ok(convert_stack_frame(f, &mut *path_mapper)))
                .collect::<Vec<InlineStackFrame>>()
                .ok()
        })
        .filter(|frames| !frames.is_empty())
}

pub fn convert_stack_frame<R: gimli::Reader>(
    frame: addr2line::Frame<R>,
    path_mapper: &mut PathMapper<()>,
) -> InlineStackFrame {
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
    let file_path = match &frame.location {
        Some(location) => location.file.map(|file| path_mapper.map_path(file)),
        None => None,
    };

    InlineStackFrame {
        function,
        file_path,
        line_number: frame.location.and_then(|l| l.line).map(|l| l as u32),
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

pub fn try_get_section_data<'data, 'file, O, T>(
    data: T,
    file: &'file O,
    section_id: SectionId,
) -> Option<SingleSectionData<'data, T>>
where
    'data: 'file,
    O: object::Object<'data, 'file>,
    T: ReadRef<'data>,
{
    use object::ObjectSection;
    let section_name = section_id.name();
    let (section, used_manual_zdebug_path) =
        if let Some(section) = file.section_by_name(section_name) {
            (section, false)
        } else {
            // Also detect old-style compressed section which start with .zdebug / __zdebug
            // in case object did not detect them.
            assert!(section_name.as_bytes().starts_with(b".debug_"));
            let mut name = Vec::with_capacity(section_name.len() + 1);
            name.extend_from_slice(b".zdebug_");
            name.extend_from_slice(&section_name.as_bytes()[7..]);
            let section = file.section_by_name_bytes(&name)?;
            (section, true)
        };

    // Handle sections which are not compressed.
    let mut file_range = section.compressed_file_range().ok()?;
    if file_range.format == CompressionFormat::None
        && used_manual_zdebug_path
        && file_range.uncompressed_size > 12
    {
        let first_twelve = data.read_bytes_at(file_range.offset, 12).ok()?;
        if first_twelve.starts_with(b"ZLIB\0\0\0\0") {
            // Object's built-in compressed section handling didn't detect this as a
            // compressed section. This happens on old Go binaries which use compressed
            // sections like __zdebug_ranges, which is generally uncommon on macOS, so
            // object's mach-O parser doesn't handle them.
            // But we want to handle them.
            // Go fixed this in https://github.com/golang/go/issues/50796 .
            let b = first_twelve.get(8..12)?;
            let uncompressed_size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            file_range = CompressedFileRange {
                format: CompressionFormat::Zlib,
                offset: file_range.offset + 12,
                compressed_size: file_range.uncompressed_size - 12,
                uncompressed_size: u64::from(uncompressed_size),
            };
        }
    }

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

    fn sect<'s, 'data, 'file, O, R>(
        &'s self,
        data: R,
        obj: &'file O,
        section_id: SectionId,
        endian: RunTimeEndian,
    ) -> EndianSlice<'s, RunTimeEndian>
    where
        'data: 'file,
        'data: 's,
        O: object::Object<'data, 'file>,
        R: ReadRef<'data>,
    {
        let slice: &[u8] = match try_get_section_data(data, obj, section_id) {
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

    pub fn make_context<'data, 'file, 's, O, R>(
        &'s self,
        data: R,
        obj: &'file O,
    ) -> Result<addr2line::Context<EndianSlice<'s, RunTimeEndian>>, Error>
    where
        'data: 'file,
        'data: 's,
        O: object::Object<'data, 'file>,
        R: ReadRef<'data>,
    {
        let e = if obj.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        let context = addr2line::Context::from_sections(
            self.sect(data, obj, SectionId::DebugAbbrev, e).into(),
            self.sect(data, obj, SectionId::DebugAddr, e).into(),
            self.sect(data, obj, SectionId::DebugAranges, e).into(),
            self.sect(data, obj, SectionId::DebugInfo, e).into(),
            self.sect(data, obj, SectionId::DebugLine, e).into(),
            self.sect(data, obj, SectionId::DebugLineStr, e).into(),
            self.sect(data, obj, SectionId::DebugRanges, e).into(),
            self.sect(data, obj, SectionId::DebugRngLists, e).into(),
            self.sect(data, obj, SectionId::DebugStr, e).into(),
            self.sect(data, obj, SectionId::DebugStrOffsets, e).into(),
            EndianSlice::new(&[], e),
        )
        .map_err(Error::Addr2lineContextCreationError)?;
        Ok(context)
    }
}
