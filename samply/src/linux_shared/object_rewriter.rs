use std::error::Error;

use object::read::elf::{FileHeader, SectionHeader};
use object::{elf, Endianness, U16, U32, U64};

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
            if s.sh_type(endian) & elf::SHT_NOBITS == 0 {
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

    // Write the ELF header.
    {
        // We copy all information from the original header, except the header size + version,
        // the section table offset, and the program header information (set to offset 0 + num 0).
        // This code would be simpler if the elf::FileHeader trait had setters, and not just getters.
        let e_ident = *in_header.e_ident();
        let e_type = in_header.e_type(endian);
        let e_machine = in_header.e_machine(endian);
        let e_version = elf::EV_CURRENT.into();
        let e_entry = in_header.e_entry(endian).into();
        let e_phoff = 0; // No program headers
        let e_shoff = section_table_offset as u64; // Adjusted section table offset
        let e_flags = in_header.e_flags(endian);
        let e_ehsize = in_header.e_ehsize(endian);
        let e_phentsize = in_header.e_phentsize(endian);
        let e_phnum = 0; // No program headers
        let e_shentsize = std::mem::size_of::<Elf::SectionHeader>() as u16;
        let e_shnum = in_header.e_shnum(endian);
        let e_shstrndx = in_header.e_shstrndx(endian);
        if is_64 {
            let out_header = elf::FileHeader64 {
                e_ident,
                e_type: U16::new(endian, e_type),
                e_machine: U16::new(endian, e_machine),
                e_version: U32::new(endian, e_version),
                e_entry: U64::new(endian, e_entry),
                e_phoff: U64::new(endian, e_phoff),
                e_shoff: U64::new(endian, e_shoff),
                e_flags: U32::new(endian, e_flags),
                e_ehsize: U16::new(endian, e_ehsize),
                e_phentsize: U16::new(endian, e_phentsize),
                e_phnum: U16::new(endian, e_phnum),
                e_shentsize: U16::new(endian, e_shentsize),
                e_shnum: U16::new(endian, e_shnum),
                e_shstrndx: U16::new(endian, e_shstrndx),
            };
            out_data.write_pod(&out_header);
        } else {
            let out_header = elf::FileHeader32 {
                e_ident,
                e_type: U16::new(endian, e_type),
                e_machine: U16::new(endian, e_machine),
                e_version: U32::new(endian, e_version),
                e_entry: U32::new(endian, e_entry as u32),
                e_phoff: U32::new(endian, e_phoff as u32),
                e_shoff: U32::new(endian, e_shoff as u32),
                e_flags: U32::new(endian, e_flags),
                e_ehsize: U16::new(endian, e_ehsize),
                e_phentsize: U16::new(endian, e_phentsize),
                e_phnum: U16::new(endian, e_phnum),
                e_shentsize: U16::new(endian, e_shentsize),
                e_shnum: U16::new(endian, e_shnum),
                e_shstrndx: U16::new(endian, e_shstrndx),
            };
            out_data.write_pod(&out_header);
        }
    }

    // Do not write any program headers.

    // Write the section data.
    for (section, offset) in in_sections.iter().zip(section_offsets.iter()) {
        if section.sh_type(endian) & elf::SHT_NOBITS == 0 {
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
        // This would be simpler if the elf::SectionHeader trait had setters, and not just getters.
        let sh_name = in_section.sh_name(endian);
        let sh_type = in_section.sh_type(endian);
        let sh_flags = in_section.sh_flags(endian).into();
        let sh_addr = in_section.sh_addr(endian).into();

        // Fix section offset
        let sh_offset = if sh_type == elf::SHT_NULL {
            0
        } else {
            *offset as u64
        };

        let sh_size = in_section.sh_size(endian).into();
        let sh_link = in_section.sh_link(endian);
        let sh_info = in_section.sh_info(endian);
        let sh_addralign = in_section.sh_addralign(endian).into();
        let sh_entsize = in_section.sh_entsize(endian).into();
        if is_64 {
            let out_section = elf::SectionHeader64 {
                sh_name: U32::new(endian, sh_name),
                sh_type: U32::new(endian, sh_type),
                sh_flags: U64::new(endian, sh_flags),
                sh_addr: U64::new(endian, sh_addr),
                sh_offset: U64::new(endian, sh_offset),
                sh_size: U64::new(endian, sh_size),
                sh_link: U32::new(endian, sh_link),
                sh_info: U32::new(endian, sh_info),
                sh_addralign: U64::new(endian, sh_addralign),
                sh_entsize: U64::new(endian, sh_entsize),
            };
            out_data.write_pod(&out_section);
        } else {
            let out_section = elf::SectionHeader32 {
                sh_name: U32::new(endian, sh_name),
                sh_type: U32::new(endian, sh_type),
                sh_flags: U32::new(endian, sh_flags as u32),
                sh_addr: U32::new(endian, sh_addr as u32),
                sh_offset: U32::new(endian, sh_offset as u32),
                sh_size: U32::new(endian, sh_size as u32),
                sh_link: U32::new(endian, sh_link),
                sh_info: U32::new(endian, sh_info),
                sh_addralign: U32::new(endian, sh_addralign as u32),
                sh_entsize: U32::new(endian, sh_entsize as u32),
            };
            out_data.write_pod(&out_section);
        }
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
