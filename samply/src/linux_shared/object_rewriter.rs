use std::error::Error;

use object::read::elf::{FileHeader, SectionHeader};
use object::{elf, Endianness};

/// Returns a `Vec<u8>` with ELF data of an equivalent image with no program
/// headers and fixed section offsets.
///
/// Example before (output of `llvm-readelf --segments` and `llvm-readelf -SW`):
///
/// ```plain
/// Program Headers:
///   Type           Offset   VirtAddr           PhysAddr           FileSiz  MemSiz   Flg Align
///   LOAD           0x000000 0x0000000000000000 0x0000000000000000 0x000374 0x000374 R E 0x8
///
/// Section Headers:
///   [Nr] Name              Type            Address          Off    Size   ES Flg Lk Inf Al
///   [ 0]                   NULL            0000000000000000 000000 000000 00      0   0  0
///   [ 1] .text             PROGBITS        0000000000000040 000080 000374 00  AX  0   0 16
///   [ 2] .eh_frame         PROGBITS        00000000000003b8 0003f8 000000 00   A  0   0  8
///   [ 3] .eh_frame_hdr     PROGBITS        00000000000003b8 0003f8 000014 00   A  0   0  4
///   [ 4] .shstrtab         STRTAB          0000000000000000 00040c 000072 00      0   0  1
///   [ 5] .symtab           SYMTAB          0000000000000000 000480 000030 18      6   0  8
///   [ 6] .strtab           STRTAB          0000000000000000 0004b0 000018 00      0   0  1
///   [ 7] .note.gnu.build-id NOTE           0000000000000000 0004c8 000024 00   A  0   0  4
/// ```
///
/// after:
///
/// ```plain
/// Program Headers:
///   Type           Offset   VirtAddr           PhysAddr           FileSiz  MemSiz   Flg Align
///
/// Section Headers:
///   [Nr] Name              Type            Address          Off    Size   ES Flg Lk Inf Al
///   [ 0]                   NULL            0000000000000000 000000 000000 00      0   0  0
///   [ 1] .text             PROGBITS        0000000000000040 000040 000374 00  AX  0   0 16
///   [ 2] .eh_frame         PROGBITS        00000000000003b8 0003b8 000000 00   A  0   0  8
///   [ 3] .eh_frame_hdr     PROGBITS        00000000000003b8 0003b8 000014 00   A  0   0  4
///   [ 4] .shstrtab         STRTAB          0000000000000000 0003cc 000072 00      0   0  1
///   [ 5] .symtab           SYMTAB          0000000000000000 000440 000030 18      6   0  8
///   [ 6] .strtab           STRTAB          0000000000000000 000470 000018 00      0   0  1
///   [ 7] .note.gnu.build-id NOTE           0000000000000000 000488 000024 00   A  0   0  4
/// ```
pub fn drop_phdr<Elf: FileHeader<Endian = Endianness>>(
    in_data: &[u8],
) -> Result<Vec<u8>, Box<dyn Error>> {
    // An ELF image has four parts: The elf header, an optional list of program headers, the section data, and the section table.
    // The section table is at the end, and contains offsets into the section data.
    // The code below drops the program headers and corrects the section offsets in the section table.

    let in_header = Elf::parse(in_data)?;
    let is_64 = in_header.is_type_64();
    let endian = in_header.endian()?;
    let elf_align = if is_64 { 8 } else { 4 };
    let in_sections = in_header.section_headers(endian, in_data)?;

    use object::write::WritableBuffer;
    let mut out_data = Vec::new();

    let mut reserved_len = 0;
    reserved_len += std::mem::size_of::<Elf>();

    let section_offsets: Vec<usize> = in_sections
        .iter()
        .map(|s: &Elf::SectionHeader| {
            if s.sh_type(endian) != elf::SHT_NOBITS {
                let addralign = s.sh_addralign(endian).into() as usize;
                if addralign != 0 {
                    reserved_len = align(reserved_len, addralign);
                }
                let offset = reserved_len;
                reserved_len += s.sh_size(endian).into() as usize;
                offset
            } else {
                reserved_len
            }
        })
        .collect();

    reserved_len = align(reserved_len, elf_align);
    let section_table_offset = reserved_len;
    reserved_len += section_offsets.len() * std::mem::size_of::<Elf::SectionHeader>();

    out_data.reserve(reserved_len);

    // We're done reserving, start writing.
    let encoder = object::write::elf::Encoder::new(endian, is_64, in_header.e_machine(endian));

    // Write the ELF header.
    {
        // We copy all information from the original header, except the header size + version,
        // the section table offset, and the program header information (set to offset 0 + num 0).
        let out_header = object::write::elf::FileHeader::from_raw(endian, in_header);
        let mut out_layout =
            object::write::elf::FileHeaderLayout::from_raw(endian, in_header, in_data)?;
        out_layout.e_phoff = 0; // No program headers
        out_layout.segment_num = 0; // No program headers
        out_layout.e_shoff = section_table_offset as u64; // Adjusted section table offset
        encoder.file_header(&mut out_data, &out_header, &out_layout)?;
    }

    // Do not write any program headers.

    // Write the section data.
    for (section, offset) in in_sections.iter().zip(section_offsets.iter()) {
        if section.sh_type(endian) != elf::SHT_NOBITS {
            write_align(&mut out_data, section.sh_addralign(endian).into() as usize);
            debug_assert_eq!(out_data.len(), *offset);
            out_data.write_bytes(section.data(endian, in_data)?);
        }
    }

    // Write the section table (i.e. the list of section headers).
    write_align(&mut out_data, elf_align);
    debug_assert_eq!(out_data.len(), section_table_offset);
    for (in_section, offset) in in_sections.iter().zip(section_offsets.iter()) {
        // We copy all information from the original section except the offset.
        let mut out_section = object::write::elf::SectionHeader::from_raw(endian, in_section);

        // Fix section offset
        out_section.sh_offset = if out_section.sh_type == elf::SHT_NULL {
            0
        } else {
            *offset as u64
        };

        encoder.section_header(&mut out_data, &out_section);
    }

    // We're done.
    debug_assert_eq!(reserved_len, out_data.len());

    Ok(out_data)
}

fn align(offset: usize, size: usize) -> usize {
    (offset + (size - 1)) & !(size - 1)
}

fn write_align(buffer: &mut Vec<u8>, size: usize) {
    let new_len = if size != 0 {
        align(buffer.len(), size)
    } else {
        buffer.len()
    };
    buffer.resize(new_len, 0);
}

#[cfg(test)]
mod test {
    #[test]
    #[ignore]
    fn rewrite_this_file_for_testing() {
        let input = std::fs::read("/Users/mstange/Downloads/jitted-123175-0.so").unwrap();
        let output =
            super::drop_phdr::<object::elf::FileHeader64<object::Endianness>>(&input).unwrap();
        std::fs::write("/Users/mstange/Downloads/jitted-123175-0-fixed.so", output).unwrap();
    }
}
