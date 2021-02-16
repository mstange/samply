use crate::shared::{AddressDebugInfo, InlineStackFrame, SymbolicationResult};
use crate::symbolicate::demangle;
use addr2line::{fallible_iterator, gimli};
use fallible_iterator::FallibleIterator;
use gimli::{EndianSlice, SectionId};
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddressPair {
    pub original_address: u32,
    pub address_in_this_object: u64,
}

impl AddressPair {
    pub fn same(address: u32) -> Self {
        AddressPair {
            original_address: address,
            address_in_this_object: address as u64,
        }
    }
}

pub fn collect_dwarf_address_debug_data<'data: 'file, 'file, O, R>(
    object: &'file O,
    addresses: &[AddressPair],
    symbolication_result: &mut R,
) where
    O: object::Object<'data, 'file>,
    R: SymbolicationResult,
{
    if addresses.is_empty() {
        return;
    }

    let section_data = SectionDataNoCopy::from_object(object);
    if let Ok(context) = section_data.make_addr2line_context() {
        for AddressPair {
            original_address,
            address_in_this_object,
        } in addresses
        {
            if let Ok(frame_iter) = context.find_frames(*address_in_this_object as u64) {
                let frames: std::result::Result<Vec<_>, _> =
                    frame_iter.map(convert_stack_frame).collect();
                if let Ok(frames) = frames {
                    symbolication_result
                        .add_address_debug_info(*original_address, AddressDebugInfo { frames });
                }
            }
        }
    }
}

fn convert_stack_frame<R: gimli::Reader>(
    frame: addr2line::Frame<R>,
) -> std::result::Result<InlineStackFrame, gimli::read::Error> {
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
        Some(location) => {
            if let Some(file) = location.file {
                Some(file.to_owned())
            } else {
                None
            }
        }
        None => None,
    };

    Ok(InlineStackFrame {
        function,
        file_path,
        line_number: frame.location.and_then(|l| l.line).map(|l| l as u32),
    })
}

/// Holds on to section data so that we can create an addr2line::Context for that
/// that data. This avoids one copy compared to what addr2line::Context::new does
/// by default, saving 1.5 seconds on libxul. (For comparison, dumping all symbols
/// from libxul takes 200ms in total.)
/// See addr2line::Context::new for details.
pub struct SectionDataNoCopy<'a> {
    pub endian: gimli::RunTimeEndian,
    pub debug_abbrev_data: Cow<'a, [u8]>,
    pub debug_addr_data: Cow<'a, [u8]>,
    pub debug_info_data: Cow<'a, [u8]>,
    pub debug_line_data: Cow<'a, [u8]>,
    pub debug_line_str_data: Cow<'a, [u8]>,
    pub debug_ranges_data: Cow<'a, [u8]>,
    pub debug_rnglists_data: Cow<'a, [u8]>,
    pub debug_str_data: Cow<'a, [u8]>,
    pub debug_str_offsets_data: Cow<'a, [u8]>,
    pub default_section_data: Cow<'a, [u8]>,
}

impl<'data> SectionDataNoCopy<'data> {
    pub fn from_object<'file, O>(file: &'file O) -> Self
    where
        'data: 'file,
        O: object::Object<'data, 'file>,
    {
        let endian = if file.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };

        fn get_section_data<'data, 'file, O>(
            file: &'file O,
            section_name: &'static str,
        ) -> Cow<'data, [u8]>
        where
            'data: 'file,
            O: object::Object<'data, 'file>,
        {
            use object::ObjectSection;
            if let Some(section) = file.section_by_name(section_name) {
                if let Ok(data) = section.uncompressed_data() {
                    return data;
                }
            }
            Cow::Borrowed(&[])
        }

        let debug_abbrev_data = get_section_data(file, SectionId::DebugAbbrev.name());
        let debug_addr_data = get_section_data(file, SectionId::DebugAddr.name());
        let debug_info_data = get_section_data(file, SectionId::DebugInfo.name());
        let debug_line_data = get_section_data(file, SectionId::DebugLine.name());
        let debug_line_str_data = get_section_data(file, SectionId::DebugLineStr.name());
        let debug_ranges_data = get_section_data(file, SectionId::DebugRanges.name());
        let debug_rnglists_data = get_section_data(file, SectionId::DebugRngLists.name());
        let debug_str_data = get_section_data(file, SectionId::DebugStr.name());
        let debug_str_offsets_data = get_section_data(file, SectionId::DebugStrOffsets.name());

        Self {
            endian,
            debug_abbrev_data,
            debug_addr_data,
            debug_info_data,
            debug_line_data,
            debug_line_str_data,
            debug_ranges_data,
            debug_rnglists_data,
            debug_str_data,
            debug_str_offsets_data,
            default_section_data: Cow::Borrowed(&[]),
        }
    }

    pub fn make_addr2line_context(
        &self,
    ) -> std::result::Result<
        addr2line::Context<EndianSlice<gimli::RunTimeEndian>>,
        gimli::read::Error,
    > {
        let endian = self.endian;
        fn get<'a, S>(section_data: &'a [u8], endian: gimli::RunTimeEndian) -> S
        where
            S: From<EndianSlice<'a, gimli::RunTimeEndian>>,
        {
            EndianSlice::new(&*section_data, endian).into()
        }

        addr2line::Context::from_sections(
            get(&self.debug_abbrev_data, endian),
            get(&self.debug_addr_data, endian),
            get(&self.debug_info_data, endian),
            get(&self.debug_line_data, endian),
            get(&self.debug_line_str_data, endian),
            get(&self.debug_ranges_data, endian),
            get(&self.debug_rnglists_data, endian),
            get(&self.debug_str_data, endian),
            get(&self.debug_str_offsets_data, endian),
            get(&self.default_section_data, endian),
        )
    }
}
