use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol, SymbolKind};

use std::path::Path;

use super::object_rewriter;

pub fn jit_function_name<'data>(obj: &object::File<'data>) -> Option<&'data str> {
    let mut text_symbols = obj.symbols().filter(|s| s.kind() == SymbolKind::Text);
    let symbol = text_symbols.next()?;
    symbol.name().ok()
}

/// Correct unusable .so files from certain versions of perf which create ELF program
/// headers but fail to adjust addresses by the program header size.
///
/// For these bad files, we create a new, fixed, file, so that the mapping correctly
/// refers to the location of its .text section.
///
/// Background:
///
/// If you use `perf record` on applications which output JITDUMP information, such as
/// the V8 shell, you usually run `perf inject --jit` on your `perf.data` file afterwards.
/// This command interprets the JITDUMP file, and creates files of the form `jitted-12345-12.so`
/// which contain the JIT code from the JITDUMP file. There is one file per function.
/// These files are ELF files which look just enough like regular ELF files so that the
/// regular perf functionality works with them - unwinding, symbols, line numbers, assembly.
///
/// Before September 2022, these files did not contain any ELF program headers ("segments"),
/// they only contained sections. The files have 0x40 bytes of ELF header, followed by a
/// .text section at offset and address 0x40, followed by a few other sections, followed by
/// the section table.
///
/// This was changed by commit <https://github.com/torvalds/linux/commit/babd04386b1df8c364cdaa39ac0e54349502e1e5>,
/// "perf jit: Include program header in ELF files", in September 2022.
/// There was a bug in this commit, which was fixed by commit <https://github.com/torvalds/linux/commit/89b15d00527b7>,
/// "perf inject: Fix GEN_ELF_TEXT_OFFSET for jit", in October 2022.
///
/// Unfortunately, the first commit made it into a number of perf releases:
/// 4.19.215+, 5.4.215+, 5.10.145+, 5.15.71+, 5.19.12+, probably 6.0.16, and 6.1.2
///
/// The bug in the first commit means that, if you load the jit-ified perf.data file using
/// `samply load` and use the `jitted-12345-12.so` files as-is, the opened profile will
/// contain no useful information about JIT functions.
///
/// The broken files have a PT_LOAD command with file offset 0 and address 0, and a
/// .text section with file offset 0x80 and address 0x40. Furthermore, the synthesized
/// mmap record points at offset 0x40. The function name symbol is at address 0x40.
///
/// This means:
///  - The mapping does not encompass the entire .text section.
///  - We cannot calculate the image's base address in process memory because the .text
///    section has a different file-offset-to-address translation (-0x40) than the
///    PT_LOAD command (0x0). Neither of the translation amounts would give us a
///    base address that works correctly with the rest of the system: Following the
///    section might give us correct symbols but bad assembly, and the other way round.
///
/// We load these .so files twice: First, during profile conversion, and then later again
/// during symbolication and also when assembly code is looked up. We need to have a
/// consistent file on the file system which works with all these consumers.
///
/// So this function creates a fixed file and adjusts all fall paths to point to the fixed
/// file. The fixed file has its program header removed, so that the original symbol address
/// and mmap record are correct for the file offset and address 0x40.
/// We could also choose to keep the program header, but then we would need to adjust a
/// lot more: the mmap record, the symbol addresses, and the debug info.
pub fn correct_bad_perf_jit_so_file(
    file: &std::fs::File,
    path: &str,
) -> Option<(std::fs::File, String)> {
    if !path.contains("/jitted-") || !path.ends_with(".so") {
        return None;
    }

    let mmap = unsafe { memmap2::MmapOptions::new().map(file) }.ok()?;
    let obj = object::read::File::parse(&mmap[..]).ok()?;
    if obj.format() != object::BinaryFormat::Elf {
        return None;
    }

    // The bad files have exactly one segment, with offset 0x0 and address 0x0.
    let segment = obj.segments().next()?;
    if segment.address() != 0 || segment.file_range().0 != 0 {
        return None;
    }

    // The bad files have a .text section with offset 0x80 and address 0x40 (on x86_64).
    let text_section = obj.section_by_name(".text")?;
    if text_section.file_range()?.0 == text_section.address() {
        return None;
    }

    // All right, we have one of the broken files!

    // Let's make it right.
    let fixed_data = if obj.is_64() {
        object_rewriter::drop_phdr::<object::elf::FileHeader64<object::Endianness>>(&mmap[..])
            .ok()?
    } else {
        object_rewriter::drop_phdr::<object::elf::FileHeader32<object::Endianness>>(&mmap[..])
            .ok()?
    };
    let mut fixed_path = path.strip_suffix(".so").unwrap().to_string();
    fixed_path.push_str("-fixed.so");

    std::fs::write(&fixed_path, fixed_data).ok()?;

    // Open the fixed file for reading, and return it.
    let fixed_file = std::fs::File::open(&fixed_path).ok()?;

    Some((fixed_file, fixed_path))
}

pub fn get_path_if_jitdump(path: &[u8]) -> Option<&Path> {
    let path = Path::new(std::str::from_utf8(path).ok()?);
    let filename = path.file_name()?.to_str()?;
    if filename.starts_with("jit-") && filename.ends_with(".dump") {
        Some(path)
    } else {
        None
    }
}
