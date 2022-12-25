use debugid::{CodeId, DebugId};
use std::ops::Range;

/// A library ("binary" / "module" / "DSO") which is loaded into a process.
/// This can be the main executable file or a dynamic library, or any other
/// mapping of executable memory.
///
/// Library information makes after-the-fact symbolication possible: The
/// profile JSON contains raw code addresses, and then the symbols for these
/// addresses get resolved later.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryInfo {
    /// The "actual virtual memory address", in the address space of the process,
    /// where this library's base address is located. The base address is the
    /// address which "relative addresses" are relative to.
    ///
    /// For ELF binaries, the base address is equal to the "image bias", i.e. the
    /// offset that is added to the virtual memory addresses as stated in the
    /// library file (SVMAs, "stated virtual memory addresses"). In other words,
    /// the base AVMA corresponds to SVMA zero.
    ///
    /// For mach-O binaries, the base address is the start of the `__TEXT` segment.
    ///
    /// For Windows binaries, the base address is the image load address.
    pub base_avma: u64,
    /// The address range that this mapping occupies in the virtual memory
    /// address space of the process. AVMA = "actual virtual memory address"
    pub avma_range: Range<u64>,
    /// The name of this library that should be displayed in the profiler.
    /// Usually this is the filename of the binary, but it could also be any other
    /// name, such as "\[kernel.kallsyms\]" or "\[vdso\]".
    pub name: String,
    /// The debug name of this library which should be used when looking up symbols.
    /// On Windows this is the filename of the PDB file, on other platforms it's
    /// usually the same as the filename of the binary.
    pub debug_name: String,
    /// The absolute path to the binary file.
    pub path: String,
    /// The absolute path to the debug file. On Linux and macOS this is the same as
    /// the path to the binary file. On Windows this is the path to the PDB file.
    pub debug_path: String,
    /// The debug ID of the library. This lets symbolication confirm that it's
    /// getting symbols for the right file, and it can sometimes allow obtaining a
    /// symbol file from a symbol server.
    pub debug_id: DebugId,
    /// The code ID of the library. This lets symbolication confirm that it's
    /// getting symbols for the right file, and it can sometimes allow obtaining a
    /// symbol file from a symbol server.
    pub code_id: Option<CodeId>,
    /// An optional string with the CPU arch of this library, for example "x86_64",
    /// "arm64", or "arm64e". Historically, this was used on macOS to find the
    /// correct sub-binary in a fat binary. But we now use the debug_id for that
    /// purpose. But it could still be used to find the right dyld shared cache for
    /// system libraries on macOS.
    pub arch: Option<String>,
}
