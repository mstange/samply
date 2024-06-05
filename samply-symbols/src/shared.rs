#[cfg(feature = "partial_read_stats")]
use std::cell::RefCell;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::marker::PhantomData;
use std::ops::{Deref, Range};
use std::str::FromStr;
use std::sync::Arc;

#[cfg(feature = "partial_read_stats")]
use bitvec::{bitvec, prelude::BitVec};
use debugid::DebugId;
use object::read::ReadRef;
use object::FileFlags;
use uuid::Uuid;

use crate::mapped_path::MappedPath;
use crate::symbol_map::SymbolMapTrait;

pub type FileAndPathHelperError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type FileAndPathHelperResult<T> = std::result::Result<T, FileAndPathHelperError>;

// Define a OptionallySendFuture trait. This exists for the following reasons:
//  - The "+ Send" in the return types of the FileAndPathHelper trait methods
//    trickles down all the way to the root async functions exposed by this crate.
//  - We have two consumers: One that requires Send on the futures returned by those
//    root functions, and one that cannot return Send futures from the trait methods.
//    The former is hyper/tokio (in profiler-symbol-server), the latter is the wasm/js
//    implementation: JsFutures are not Send.
// So we provide a cargo feature to allow the consumer to select whether they want Send or not.
//
// Please tell me that there is a better way.

#[cfg(not(feature = "send_futures"))]
pub trait OptionallySendFuture: Future {}

#[cfg(not(feature = "send_futures"))]
impl<T> OptionallySendFuture for T where T: Future {}

#[cfg(feature = "send_futures")]
pub trait OptionallySendFuture: Future + Send {}

#[cfg(feature = "send_futures")]
impl<T> OptionallySendFuture for T where T: Future + Send {}

#[derive(Debug)]
pub enum CandidatePathInfo<FL: FileLocation> {
    SingleFile(FL),
    InDyldCache {
        dyld_cache_path: FL,
        dylib_path: String,
    },
}

/// An address that can be looked up in a `SymbolMap`.
///
/// You'll usually want to use `LookupAddress::Relative`, i.e. addresses that
/// are relative to some "image base address". This form works with all types
/// of symbol maps across all platforms.
///
/// When testing, be aware that many binaries are laid out in such a way that
/// all three representations of addresses are the same: The image base address
/// is often zero and the sections are often laid out so that each section's
/// address matches its file offset. So if you misrepresent an address in
/// the wrong form, you might not notice it because it still works until you
/// encounter a more complex binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LookupAddress {
    /// A relative address is relative to the image base address.
    ///
    /// What this means depends on the format of the binary:
    ///
    /// - On Windows, a "relative address" is the same as a RVA ("relative virtual
    ///   address") in the PE file.
    /// - On macOS, a "relative address" is relative to the start of the `__TEXT`
    ///   segment.
    /// - On Linux / ELF, a "relative address" is relative to the address of the
    ///   first LOAD command in the program header table. In other words, it's
    ///   relative to the start of the first segment.
    /// - For Jitdump files, the "relative address" space is a conceptual space
    ///   in which the code from all `JIT_CODE_LOAD` records is laid out
    ///   sequentially, starting at 0.
    ///   So the relative address of an instruction inside a `JIT_CODE_LOAD` record
    ///   is the sum of the `code_size` fields of all previous `JIT_CODE_LOAD`
    ///   records plus the offset of the instruction within the code of this
    ///   `JIT_CODE_LOAD` record.
    ///
    /// See [`relative_address_base`] for more information.
    Relative(u32),
    /// A "stated virtual memory address", i.e. a virtual memory address as
    /// written down in the binary. In mach-O and ELF, this is the space that
    /// section addresses and symbol addresses are in. It's the type of address
    /// you'd pass to the Linux `addr2line` tool.
    ///
    /// This type of lookup address is not supported by symbol maps for PDB
    /// files or Breakpad files.
    Svma(u64),
    /// A raw file offset to the point in the binary file where the bytes of the
    /// instruction are stored for which symbols should be looked up.
    ///
    /// On Linux, if you have an "AVMA" (absolute virtual memory address) and
    /// the `/proc/<pid>/maps` for the process, this is probably the easiest
    /// form of address to compute, because the process maps give you the file offsets.
    ///
    /// However, if you do this, be aware that the file offset often is not
    /// the same as an SVMA, so expect wrong results if you end up using it in
    /// places where SVMAs are expected - it might work fine with some binaries
    /// and then break with others.
    ///
    /// File offsets are not supported by symbol maps for PDB files or Breakpad files.
    FileOffset(u64),
}

/// In case the loaded binary contains multiple architectures, this specifies
/// how to resolve the ambiguity. This is only needed on macOS.
#[derive(Debug, Clone)]
pub enum MultiArchDisambiguator {
    /// Disambiguate by CPU architecture (exact match).
    ///
    /// This string is a name for what mach-O calls the "CPU type" and "CPU subtype".
    /// Examples are `x86_64`, `x86_64h`, `arm64`, `arm64e`.
    ///
    /// These strings are returned by the mach function `macho_arch_name_for_cpu_type`.
    Arch(String),

    /// Disambiguate by CPU architecture (best match).
    ///
    /// The Vec contains the first choice, followed by acceptable fallback choices.
    /// Examples are `["arm64e", "arm64"]` or `["x86_64h", "x86_64"]`.
    /// This is used in cases where you have lost information about the architecture
    /// you're interested in and just want to hope to get the right one.
    ///
    /// The strings are names for what mach-O calls the "CPU type" and "CPU subtype".
    /// Examples are `x86_64`, `x86_64h`, `arm64`, `arm64e`.
    ///
    /// These strings are returned by the mach function `macho_arch_name_for_cpu_type`.
    BestMatch(Vec<String>),

    /// Disambiguate by CPU architecture and find the best match for the architecture
    /// that is currently executing this code. This is a heuristic, and should only
    /// be used in cases where you have lost information about the architecture you're
    /// interested in.
    BestMatchForNative,

    /// Disambiguate by `DebugId`.
    DebugId(DebugId),
}

/// An enum carrying an identifier for a binary. This is stores the same information
/// as a [`debugid::CodeId`], but without projecting it down to a string.
///
/// All types need to be treated rather differently, see their respective documentation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodeId {
    /// The code ID for a Windows PE file. When combined with the binary name,
    /// the code ID lets you obtain binaries from symbol servers. It is not useful
    /// on its own, it has to be paired with the binary name.
    ///
    /// On Windows, a binary's code ID is distinct from its debug ID (= pdb GUID + age).
    /// If you have a binary file, you can get both the code ID and the debug ID
    /// from it. If you only have a PDB file, you usually *cannot* get the code ID of
    /// the corresponding binary from it.
    PeCodeId(PeCodeId),

    /// The code ID for a macOS / iOS binary (mach-O). This is just the mach-O UUID.
    /// The mach-O UUID is shared between both the binary file and the debug file (dSYM),
    /// and it can be used on its own to find dSYMs using Spotlight.
    ///
    /// The debug ID and the code ID contain the same information; the debug ID
    /// is literally just the UUID plus a zero at the end.
    MachoUuid(Uuid),

    /// The code ID for a Linux ELF file. This is the "ELF build ID" (also called "GNU build ID").
    /// The build ID is usually 20 bytes, commonly written out as 40 hex chars.
    ///
    /// It can be used to find debug files on the local file system or to download
    /// binaries or debug files from a `debuginfod` symbol server. it does not have to be
    /// paired with the binary name.
    ///
    /// An ELF binary's code ID is more useful than its debug ID: The debug ID is truncated
    /// to 16 bytes (32 hex characters), whereas the code ID is the full ELF build ID.
    ElfBuildId(ElfBuildId),
}

impl FromStr for CodeId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() <= 17 {
            // 8 bytes timestamp + 1 to 8 bytes of image size
            Ok(CodeId::PeCodeId(PeCodeId::from_str(s)?))
        } else if s.len() == 32 && is_uppercase_hex(s) {
            // mach-O UUID
            Ok(CodeId::MachoUuid(Uuid::from_str(s).map_err(|_| ())?))
        } else {
            // ELF build ID. These are usually 40 hex characters (= 20 bytes).
            Ok(CodeId::ElfBuildId(ElfBuildId::from_str(s)?))
        }
    }
}

fn is_uppercase_hex(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_uppercase()))
}

impl std::fmt::Display for CodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeId::PeCodeId(pe) => std::fmt::Display::fmt(pe, f),
            CodeId::MachoUuid(uuid) => f.write_fmt(format_args!("{:X}", uuid.simple())),
            CodeId::ElfBuildId(elf) => std::fmt::Display::fmt(elf, f),
        }
    }
}

/// The code ID for a Windows PE file.
///
/// When combined with the binary name, the `PeCodeId` lets you obtain binaries from
/// symbol servers. It is not useful on its own, it has to be paired with the binary name.
///
/// A Windows binary's `PeCodeId` is distinct from its debug ID (= pdb GUID + age).
/// If you have a binary file, you can get both the `PeCodeId` and the debug ID
/// from it. If you only have a PDB file, you usually *cannot* get the `PeCodeId` of
/// the corresponding binary from it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeCodeId {
    pub timestamp: u32,
    pub image_size: u32,
}

impl FromStr for PeCodeId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < 9 || s.len() > 16 {
            return Err(());
        }
        let timestamp = u32::from_str_radix(&s[..8], 16).map_err(|_| ())?;
        let image_size = u32::from_str_radix(&s[8..], 16).map_err(|_| ())?;
        Ok(Self {
            timestamp,
            image_size,
        })
    }
}

impl std::fmt::Display for PeCodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:08X}{:x}", self.timestamp, self.image_size))
    }
}

/// The build ID for an ELF file (also called "GNU build ID").
///
/// The build ID can be used to find debug files on the local file system or to download
/// binaries or debug files from a `debuginfod` symbol server. it does not have to be
/// paired with the binary name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElfBuildId(pub Vec<u8>);

impl ElfBuildId {
    /// Create a new `ElfBuildId` from a slice of bytes (commonly a sha1 hash
    /// generated by the linker, i.e. 20 bytes).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(bytes.to_owned())
    }
}

impl FromStr for ElfBuildId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let byte_count = s.len() / 2;
        let mut bytes = Vec::with_capacity(byte_count);
        for i in 0..byte_count {
            let hex_byte = &s[i * 2..i * 2 + 2];
            let b = u8::from_str_radix(hex_byte, 16).map_err(|_| ())?;
            bytes.push(b);
        }
        Ok(Self(bytes))
    }
}

impl std::fmt::Display for ElfBuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            f.write_fmt(format_args!("{byte:02x}"))?;
        }
        Ok(())
    }
}

/// Information about a library ("binary" / "module" / "DSO") which allows finding
/// symbol files for it. The information can be partial.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibraryInfo {
    pub debug_name: Option<String>,
    pub debug_id: Option<DebugId>,
    pub debug_path: Option<String>,
    pub name: Option<String>,
    pub code_id: Option<CodeId>,
    pub path: Option<String>,
    pub arch: Option<String>,
}

impl LibraryInfo {
    /// Fill all `None` fields on this object with the corresponding fields from `other`.
    ///
    /// This should only be called if some minimal matching has been established, for
    /// example if the `code_id` matches or if the combination pair `debug_name, debug_id`
    /// matches.
    pub fn absorb(&mut self, other: &LibraryInfo) {
        if self.debug_name.is_none() && other.debug_name.is_some() {
            self.debug_name.clone_from(&other.debug_name);
        }
        if self.debug_id.is_none() && other.debug_id.is_some() {
            self.debug_id = other.debug_id;
        }
        if self.debug_path.is_none() && other.debug_path.is_some() {
            self.debug_path.clone_from(&other.debug_path);
        }
        if self.name.is_none() && other.name.is_some() {
            self.name.clone_from(&other.name);
        }
        if self.code_id.is_none() && other.code_id.is_some() {
            self.code_id.clone_from(&other.code_id);
        }
        if self.path.is_none() && other.path.is_some() {
            self.path.clone_from(&other.path);
        }
        if self.arch.is_none() && other.arch.is_some() {
            self.arch.clone_from(&other.arch);
        }
    }
}

/// This is the trait that consumers need to implement so that they can call
/// the main entry points of this crate. This crate contains no direct file
/// access - all access to the file system is via this trait, and its associated
/// trait `FileContents`.
pub trait FileAndPathHelper {
    type F: FileContents + 'static;
    type FL: FileLocation + 'static;

    /// Given a "debug name" and a "breakpad ID", return a list of file paths
    /// which may potentially have artifacts containing symbol data for the
    /// requested binary (executable or library).
    ///
    /// The symbolication methods will try these paths one by one, calling
    /// `load_file` for each until it succeeds and finds a file whose contents
    /// match the breakpad ID. Any remaining paths are discarded.
    ///
    /// # Arguments
    ///
    ///  - `debug_name`: On Windows, this is the filename of the associated PDB
    ///    file of the executable / DLL, for example "firefox.pdb" or "xul.pdb". On
    ///    non-Windows, this is the filename of the binary, for example "firefox"
    ///    or "XUL" or "libxul.so".
    ///  - `breakpad_id`: A string of 33 hex digits, serving as a hash of the
    ///    contents of the binary / library. On Windows, this is 32 digits "signature"
    ///    plus one digit of "pdbAge". On non-Windows, this is the binary's UUID
    ///    (ELF id or mach-o UUID) plus a "0" digit at the end (replacing the pdbAge).
    ///
    fn get_candidate_paths_for_debug_file(
        &self,
        info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<Self::FL>>>;

    /// TODO
    fn get_candidate_paths_for_binary(
        &self,
        info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<Self::FL>>>;

    /// TODO
    fn get_dyld_shared_cache_paths(
        &self,
        arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<Self::FL>>;

    /// TODO
    fn get_candidate_paths_for_gnu_debug_link_dest(
        &self,
        _original_file_location: &Self::FL,
        _debug_link_name: &str,
    ) -> FileAndPathHelperResult<Vec<Self::FL>> {
        Ok(Vec::new())
    }

    /// TODO
    fn get_candidate_paths_for_supplementary_debug_file(
        &self,
        _original_file_path: &Self::FL,
        _supplementary_file_path: &str,
        _supplementary_file_build_id: &ElfBuildId,
    ) -> FileAndPathHelperResult<Vec<Self::FL>> {
        Ok(Vec::new())
    }

    /// This method is the entry point for file access during symbolication.
    /// The implementer needs to return an object which implements the `FileContents` trait.
    /// This method is asynchronous, but once it returns, the file data needs to be
    /// available synchronously because the `FileContents` methods are synchronous.
    /// If there is no file at the requested path, an error should be returned (or in any
    /// other error case).
    fn load_file(
        &self,
        location: Self::FL,
    ) -> std::pin::Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + '_>>;

    /// Ask the helper to return a SymbolMap if it happens to have one available already.
    fn get_symbol_map_for_library(
        &self,
        _info: &LibraryInfo,
    ) -> Option<(Self::FL, Arc<dyn SymbolMapTrait + Send + Sync>)> {
        None
    }
}

/// Provides synchronous access to the raw bytes of a file.
/// This trait needs to be implemented by the consumer of this crate.
pub trait FileContents: Send + Sync {
    /// Must return the length, in bytes, of this file.
    fn len(&self) -> u64;

    /// Whether the file is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Must return a slice of the file contents, or an error.
    /// The slice's lifetime must be valid for the entire lifetime of this
    /// `FileContents` object. This restriction may be a bit cumbersome to satisfy;
    /// it's a restriction that's inherited from the `object` crate's `ReadRef` trait.
    fn read_bytes_at(&self, offset: u64, size: u64) -> FileAndPathHelperResult<&[u8]>;

    /// TODO: document
    fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]>;

    /// Append `size` bytes to `buffer`, starting to read at `offset` in the file.
    /// If successful, `buffer` must have had its len increased exactly by `size`,
    /// otherwise the caller may panic.
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()>;
}

/// The debug information (function name, file path, line number) for a single frame
/// at the looked-up address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDebugInfo {
    /// The function name for this frame, if known.
    pub function: Option<String>,
    /// The [`SourceFilePath`] for this frame, if known.
    pub file_path: Option<SourceFilePath>,
    /// The line number for this frame, if known.
    pub line_number: Option<u32>,
}

/// A trait which abstracts away the token that's passed to the [`FileAndPathHelper::load_file`]
/// trait method.
///
/// This is usually something like a `PathBuf`, but it can also be more complicated. For example,
/// in `wholesym` this is an enum which can refer to a local file or to a file from a symbol
/// server.
pub trait FileLocation: Clone + Display {
    /// Called on a Dyld shared cache location to create a location for a subcache.
    /// Subcaches are separate files with filenames such as `dyld_shared_cache_arm64e.01`.
    ///
    /// The suffix begins with a period.
    fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self>;

    /// Called on the location of a debug file in order to create a location for an
    /// external object file, based on an absolute path found in the "object map" of
    /// the original file.
    fn location_for_external_object_file(&self, object_file: &str) -> Option<Self>;

    /// Callod on the location of a PE binary in order to create a location for
    /// a corresponding PDB file, based on an absolute PDB path found in the binary.
    fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self>;

    /// Called on the location of a debug file in order to create a location for
    /// a source file. `source_file_path` is the path to the source file as written
    /// down in the debug file. This is usually an absolute path.
    ///
    /// Only one case with a relative path has been observed to date: In this case the
    /// "debug file" was a synthetic .so file which was generated by `perf inject --jit`
    /// based on a JITDUMP file which included relative paths. You could argue
    /// that the application which emitted relative paths into the JITDUMP file was
    /// creating bad data and should have written out absolute paths. However, the `perf`
    /// infrastructure worked fine on this file, because the relative paths happened to
    /// be relative to the working directory, and because perf / objdump were resolving
    /// those relative paths relative to the current working directory.
    fn location_for_source_file(&self, source_file_path: &str) -> Option<Self>;

    /// Called on the location of a Breakpad sym file, to get a location for its
    /// corresponding symindex file.
    fn location_for_breakpad_symindex(&self) -> Option<Self>;

    fn location_for_dwo(&self, comp_dir: &str, path: &str) -> Option<Self>;

    fn location_for_dwp(&self) -> Option<Self>;
}

/// The path of a source file, as found in the debug info.
///
/// This contains both the raw path and an optional "mapped path". The raw path can
/// refer to a file on this machine or on a different machine (i.e. the original
/// build machine). The mapped path is something like a permalink which potentially
/// allows obtaining the source file from a source server or a public hosted repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFilePath {
    /// The raw path to the source file, as written down in the debug file. This is
    /// usually an absolute path.
    raw_path: String,

    /// A variant of the path which may allow obtaining the source code for this file
    /// from the web.
    mapped_path: Option<MappedPath>,
}

impl SourceFilePath {
    /// Create a new `SourceFilePath`.
    pub fn new(raw_path: String, mapped_path: Option<MappedPath>) -> Self {
        Self {
            raw_path,
            mapped_path,
        }
    }

    /// Create a `SourceFilePath` from a path in a Breakpad .sym file. Such files can
    /// contain the "special path" serialization of a mapped path, but they can
    /// also contain absolute paths.
    pub fn from_breakpad_path(raw_path: String) -> Self {
        let mapped_path = MappedPath::from_special_path_str(&raw_path);
        Self {
            raw_path,
            mapped_path,
        }
    }

    /// A short, display-friendly version of this path.
    pub fn display_path(&self) -> String {
        match self.mapped_path() {
            Some(mapped_path) => mapped_path.display_path(),
            None => self.raw_path.clone(),
        }
    }

    /// The raw path to the source file, as written down in the debug file. This is
    /// usually an absolute path.
    ///
    /// Examples:
    ///
    ///  - `"/Users/mstange/code/samply/samply-symbols/src/shared.rs"`
    ///  - `"/Users/mstange/code/mozilla/widget/cocoa/nsNativeThemeCocoa.mm"`
    ///  - `"./csu/../csu/libc-start.c"`
    ///  - `"/rustc/69f9c33d71c871fc16ac445211281c6e7a340943/library/core/src/ptr/const_ptr.rs"`
    ///  - `r#"D:\agent\_work\2\s\src\vctools\crt\vcstartup\src\startup\exe_common.inl"#`
    ///
    /// If the debug file was produced by compiling code on this machine, then the path
    /// usually refers to a file on this machine. (An exception to this is debug info
    /// from the Rust stdlib, which has fake `/rustc/<rev>/...` paths even if the when
    /// compiling Rust code locally.)
    ///
    /// If the code was compiled on a different machine, then the raw path does not refer
    /// to a file on this machine.
    ///
    /// Sometimes this path is a relative path. One such case was observed when the
    /// "debug file" was a synthetic .so file which was generated by `perf inject --jit`
    /// based on a JITDUMP file which included relative paths. You could argue
    /// that the application which emitted relative paths into the JITDUMP file was
    /// creating bad data and should have written out absolute paths. However, the `perf`
    /// infrastructure worked fine on this file, because the relative paths happened to
    /// be relative to the working directory, and because perf / objdump were resolving
    /// those relative paths relative to the current working directory.
    pub fn raw_path(&self) -> &str {
        &self.raw_path
    }

    /// Returns the raw path while consuming this `SourceFilePath`.
    pub fn into_raw_path(self) -> String {
        self.raw_path
    }

    /// A variant of the path which may allow obtaining the source code for this file
    /// from the web.
    ///
    /// Examples:
    ///
    ///   - If the source file is from a Rust dependency from crates.io, we detect the
    ///     cargo cache directory in the raw path and create a mapped path of the form [`MappedPath::Cargo`].
    ///   - If the source file can be obtained from a github URL, and we know this either
    ///     from the `srcsrv` stream of a PDB file or because we recognize a path of the
    ///     form `/rustc/<rust-revision>/`, then we create a mapped path of the form [`MappedPath::Git`].
    pub fn mapped_path(&self) -> Option<&MappedPath> {
        self.mapped_path.as_ref()
    }

    /// Returns the mapped path while consuming this `SourceFilePath`.
    pub fn into_mapped_path(self) -> Option<MappedPath> {
        self.mapped_path
    }
}

/// The "relative address base" is the base address which [`LookupAddress::Relative`]
/// addresses are relative to. You start with an SVMA (a stated virtual memory address),
/// you subtract the relative address base, and out comes a relative address.
///
/// This function computes that base address. It is defined as follows:
///
///  - For Windows binaries, the base address is the "image base address".
///  - For mach-O binaries, the base address is the vmaddr of the __TEXT segment.
///  - For ELF binaries, the base address is the vmaddr of the *first* segment,
///    i.e. the vmaddr of the first "LOAD" ELF command.
///
/// In many cases, this base address is simply zero:
///
///  - ELF images of dynamic libraries (i.e. not executables) usually have a
///    base address of zero.
///  - Stand-alone mach-O dylibs usually have a base address of zero because their
///    __TEXT segment is at address zero.
///  - In PDBs, "RVAs" are relative addresses which are already relative to the
///    image base.
///
/// However, in the following cases, the base address is usually non-zero:
///
///  - The "image base address" of Windows binaries is usually non-zero.
///  - mach-O executable files (not dylibs) usually have their __TEXT segment at
///    address 0x100000000.
///  - mach-O libraries in the dyld shared cache have a __TEXT segment at some
///    non-zero address in the cache.
///  - ELF executables can have non-zero base addresses, e.g. 0x200000 or 0x400000.
///  - Kernel ELF binaries ("vmlinux") have a large base address such as
///    0xffffffff81000000. Moreover, the base address seems to coincide with the
///    vmaddr of the .text section, which is readily-available in perf.data files
///    (in a synthetic mapping called "[kernel.kallsyms]_text").
pub fn relative_address_base<'data>(object_file: &impl object::Object<'data>) -> u64 {
    use object::read::ObjectSegment;
    if let Some(text_segment) = object_file
        .segments()
        .find(|s| s.name() == Ok(Some("__TEXT")))
    {
        // This is a mach-O image. "Relative addresses" are relative to the
        // vmaddr of the __TEXT segment.
        return text_segment.address();
    }

    if let FileFlags::Elf { .. } = object_file.flags() {
        // This is an ELF image. "Relative addresses" are relative to the
        // vmaddr of the first segment (the first LOAD command).
        if let Some(first_segment) = object_file.segments().next() {
            return first_segment.address();
        }
    }

    // For PE binaries, relative_address_base() returns the image base address.
    object_file.relative_address_base()
}

/// The symbol for a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolInfo {
    /// The function's address. This is a relative address.
    pub address: u32,
    /// The function size, in bytes. May have been approximated from neighboring symbols.
    pub size: Option<u32>,
    /// The function name, demangled.
    pub name: String,
}

/// The lookup result for an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInfo {
    /// Information about the symbol which contains the looked up address.
    pub symbol: SymbolInfo,
    /// Information about the frames at the looked up address, if found in the debug info.
    ///
    /// This Vec contains the file name and line number of the address.
    /// If the compiler inlined a function call at this address, then this Vec
    /// also contains the function name of the inlined function, along with the
    /// file and line information inside that function.
    ///
    /// The Vec begins with the callee-most ("innermost") inlinee, followed by
    /// its caller, and so on. The last element is always the outer function.
    pub frames: Option<Vec<FrameDebugInfo>>,
}

/// The lookup result from `lookup_sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAddressInfo {
    /// Information about the symbol which contains the looked up address.
    pub symbol: SymbolInfo,
    /// Information about the frames at the looked up address, from the debug info.
    pub frames: Option<FramesLookupResult>,
}

/// Contains address debug info (inlined functions, file names, line numbers) if
/// available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramesLookupResult {
    /// Debug info for this address was found in the symbol map.
    ///
    /// This Vec contains the file name and line number of the address.
    /// If the compiler inlined a function call at this address, then this Vec
    /// also contains the function name of the inlined function, along with the
    /// file and line information inside that function.
    ///
    /// The Vec begins with the callee-most ("innermost") inlinee, followed by
    /// its caller, and so on. The last element is always the outer function.
    Available(Vec<FrameDebugInfo>),

    /// Debug info for this address was not found in the symbol map, but can
    /// potentially be found in a different file, with the help of
    /// [`SymbolMap::lookup_external`](crate::SymbolMap::lookup_external).
    ///
    /// This case can currently only be hit on macOS: On macOS, linking multiple
    /// `.o` files together into a library or an executable does not copy the
    /// DWARF information into the linked output. Instead, the linker stores the
    /// paths to those original `.o` files, using 'OSO' stabs entries, and debug
    /// info must be obtained from those original files.
    External(ExternalFileAddressRef),
}

/// Information to find an external file and an address within that file, to be
/// passed to [`SymbolMap::lookup_external`](crate::SymbolMap::lookup_external) or
/// [`ExternalFileSymbolMap::lookup`](crate::ExternalFileSymbolMap::lookup).
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalFileAddressRef {
    /// Information needed to find the external file.
    pub file_ref: ExternalFileRef,
    /// Information needed to find the address within that external file.
    pub address_in_file: ExternalFileAddressInFileRef,
}

/// Information to find an external file with debug information.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExternalFileRef {
    MachoExternalObject {
        /// The path to the file, as specified in the linked binary's object map.
        file_path: String,
    },
    ElfExternalDwo {
        comp_dir: String,
        path: String,
    },
}

/// Information to find an address within an external file, for debug info lookup.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExternalFileAddressInFileRef {
    MachoOsoObject {
        /// The name of the function symbol, as bytes, for the function which contains the
        /// address we want to look up.
        symbol_name: Vec<u8>,
        /// The address to look up, as a relative offset from the function symbol address.
        offset_from_symbol: u32,
    },
    MachoOsoArchive {
        /// If the external file is an archive file (e.g. `libjs_static.a`, created with `ar`),
        /// then this is the name of the archive member (e.g. `Unified_cpp_js_src23.o`),
        /// otherwise `None`.
        name_in_archive: String,
        /// The name of the function symbol, as bytes, for the function which contains the
        /// address we want to look up.
        symbol_name: Vec<u8>,
        /// The address to look up, as a relative offset from the function symbol address.
        offset_from_symbol: u32,
    },
    ElfDwo {
        dwo_id: u64,
        svma: u64,
    },
}

/// Implementation for slices.
impl<T: Deref<Target = [u8]> + Send + Sync> FileContents for T {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn read_bytes_at(&self, offset: u64, size: u64) -> FileAndPathHelperResult<&[u8]> {
        <[u8]>::get(self, offset as usize..)
            .and_then(|s| s.get(..size as usize))
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "FileContents::read_bytes_at for &[u8] was called with out-of-range indexes",
                )
                .into()
            })
    }

    fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]> {
        if range.end < range.start {
            return Err("Invalid range in read_bytes_at_until".into());
        }
        let slice = self.read_bytes_at(range.start, range.end - range.start)?;
        if let Some(pos) = memchr::memchr(delimiter, slice) {
            Ok(&slice[..pos])
        } else {
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Delimiter not found",
            )))
        }
    }

    #[inline]
    fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()> {
        buffer.extend_from_slice(self.read_bytes_at(offset, size as u64)?);
        Ok(())
    }
}

#[cfg(feature = "partial_read_stats")]
const CHUNK_SIZE: u64 = 32 * 1024;

#[cfg(feature = "partial_read_stats")]
struct FileReadStats {
    bytes_read: u64,
    unique_chunks_read: BitVec,
    read_call_count: u64,
}

#[cfg(feature = "partial_read_stats")]
impl FileReadStats {
    pub fn new(size_in_bytes: u64) -> Self {
        assert!(size_in_bytes > 0);
        let chunk_count = (size_in_bytes - 1) / CHUNK_SIZE + 1;
        FileReadStats {
            bytes_read: 0,
            unique_chunks_read: bitvec![0; chunk_count as usize],
            read_call_count: 0,
        }
    }

    pub fn record_read(&mut self, offset: u64, size: u64) {
        if size == 0 {
            return;
        }

        let start = offset;
        let end = offset + size;
        let chunk_index_start = start / CHUNK_SIZE;
        let chunk_index_end = (end - 1) / CHUNK_SIZE + 1;

        let chunkbits =
            &mut self.unique_chunks_read[chunk_index_start as usize..chunk_index_end as usize];
        if chunkbits.count_ones() != (chunk_index_end - chunk_index_start) as usize {
            if chunkbits[0] {
                self.bytes_read += chunk_index_end * CHUNK_SIZE - start;
            } else {
                self.bytes_read += (chunk_index_end - chunk_index_start) * CHUNK_SIZE;
            }
            self.read_call_count += 1;
        }
        chunkbits.set_all(true);
    }

    pub fn unique_bytes_read(&self) -> u64 {
        self.unique_chunks_read.count_ones() as u64 * CHUNK_SIZE
    }
}

#[cfg(feature = "partial_read_stats")]
impl std::fmt::Display for FileReadStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unique_bytes_read = self.unique_bytes_read();
        let repeated_bytes_read = self.bytes_read - unique_bytes_read;
        let redudancy_percentage = repeated_bytes_read * 100 / unique_bytes_read;
        write!(
            f,
            "{} total, {} unique, {}% redundancy, {} reads total",
            bytesize::ByteSize(self.bytes_read),
            bytesize::ByteSize(unique_bytes_read),
            redudancy_percentage,
            self.read_call_count
        )
    }
}

/// A wrapper for a FileContents object. The wrapper provides some convenience methods
/// and, most importantly, implements `ReadRef` for `&FileContentsWrapper`.
pub struct FileContentsWrapper<T: FileContents> {
    file_contents: T,
    len: u64,
    #[cfg(feature = "partial_read_stats")]
    partial_read_stats: std::sync::Mutex<FileReadStats>,
}

impl<T: FileContents> FileContentsWrapper<T> {
    pub fn new(file_contents: T) -> Self {
        let len = file_contents.len();
        Self {
            file_contents,
            len,
            #[cfg(feature = "partial_read_stats")]
            partial_read_stats: std::sync::Mutex::new(FileReadStats::new(len)),
        }
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn read_bytes_at(&self, offset: u64, size: u64) -> FileAndPathHelperResult<&[u8]> {
        #[cfg(feature = "partial_read_stats")]
        self.partial_read_stats
            .lock()
            .unwrap()
            .record_read(offset, size);

        self.file_contents.read_bytes_at(offset, size)
    }

    #[inline]
    pub fn read_bytes_at_until(
        &self,
        range: Range<u64>,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]> {
        #[cfg(feature = "partial_read_stats")]
        let start = range.start;

        let bytes = self.file_contents.read_bytes_at_until(range, delimiter)?;

        #[cfg(feature = "partial_read_stats")]
        self.partial_read_stats
            .lock()
            .unwrap()
            .record_read(start, (bytes.len() + 1) as u64);

        Ok(bytes)
    }

    /// Append `size` bytes to `buffer`, starting to read at `offset` in the file.
    /// If successful, `buffer` must have had its len increased exactly by `size`,
    /// otherwise the caller may panic.
    pub fn read_bytes_into(
        &self,
        buffer: &mut Vec<u8>,
        offset: u64,
        size: usize,
    ) -> FileAndPathHelperResult<()> {
        #[cfg(feature = "partial_read_stats")]
        self.partial_read_stats
            .lock()
            .unwrap()
            .record_read(offset, size as u64);

        self.file_contents.read_bytes_into(buffer, offset, size)
    }

    pub fn read_entire_data(&self) -> FileAndPathHelperResult<&[u8]> {
        self.read_bytes_at(0, self.len())
    }

    pub fn full_range(&self) -> RangeReadRef<'_, &Self> {
        RangeReadRef::new(self, 0, self.len)
    }

    pub fn range(&self, start: u64, size: u64) -> RangeReadRef<'_, &Self> {
        RangeReadRef::new(self, start, size)
    }
}

#[cfg(feature = "partial_read_stats")]
impl<T: FileContents> Drop for FileContentsWrapper<T> {
    fn drop(&mut self) {
        eprintln!("{}", self.partial_read_stats.lock());
    }
}

impl<T: FileContents> Debug for FileContentsWrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileContentsWrapper({} bytes)", self.len())
    }
}

impl<'data, T: FileContents> ReadRef<'data> for &'data FileContentsWrapper<T> {
    #[inline]
    fn len(self) -> Result<u64, ()> {
        Ok(self.len())
    }

    #[inline]
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'data [u8], ()> {
        self.read_bytes_at(offset, size).map_err(|_| {
            // Note: We're discarding the error from the FileContents method here.
        })
    }

    #[inline]
    fn read_bytes_at_until(self, range: Range<u64>, delimiter: u8) -> Result<&'data [u8], ()> {
        self.read_bytes_at_until(range, delimiter).map_err(|_| {
            // Note: We're discarding the error from the FileContents method here.
        })
    }
}

#[test]
fn test_filecontents_readref_is_send_and_sync() {
    fn assert_is_send<T: Send>() {}
    fn assert_is_sync<T: Sync>() {}
    #[allow(unused)]
    fn wrapper<T: FileContents + Sync>() {
        assert_is_send::<&FileContentsWrapper<T>>();
        assert_is_sync::<&FileContentsWrapper<T>>();
    }
}

#[derive(Clone, Copy)]
pub struct RangeReadRef<'data, T: ReadRef<'data>> {
    original_readref: T,
    range_start: u64,
    range_size: u64,
    _phantom_data: PhantomData<&'data ()>,
}

impl<'data, T: ReadRef<'data>> RangeReadRef<'data, T> {
    pub fn new(original_readref: T, range_start: u64, range_size: u64) -> Self {
        Self {
            original_readref,
            range_start,
            range_size,
            _phantom_data: PhantomData,
        }
    }

    pub fn make_subrange(&self, start: u64, size: u64) -> Self {
        Self::new(self.original_readref, self.range_start + start, size)
    }

    pub fn original_readref(&self) -> T {
        self.original_readref
    }

    pub fn range_start(&self) -> u64 {
        self.range_start
    }

    pub fn range_size(&self) -> u64 {
        self.range_size
    }
}

impl<'data, T: ReadRef<'data>> ReadRef<'data> for RangeReadRef<'data, T> {
    #[inline]
    fn len(self) -> Result<u64, ()> {
        Ok(self.range_size)
    }

    #[inline]
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'data [u8], ()> {
        let shifted_offset = self.range_start.checked_add(offset).ok_or(())?;
        self.original_readref.read_bytes_at(shifted_offset, size)
    }

    #[inline]
    fn read_bytes_at_until(self, range: Range<u64>, delimiter: u8) -> Result<&'data [u8], ()> {
        if range.end < range.start {
            return Err(());
        }
        let shifted_start = self.range_start.checked_add(range.start).ok_or(())?;
        let shifted_end = self.range_start.checked_add(range.end).ok_or(())?;
        let range = shifted_start..shifted_end;
        self.original_readref.read_bytes_at_until(range, delimiter)
    }
}

pub struct FileContentsCursor<'a, T: FileContents> {
    /// Invariant: current_offset + remaining_len == total_len
    current_offset: u64,
    /// Invariant: current_offset + remaining_len == total_len
    remaining_len: u64,
    inner: &'a FileContentsWrapper<T>,
}

impl<'a, T: FileContents> FileContentsCursor<'a, T> {
    pub fn new(inner: &'a FileContentsWrapper<T>) -> Self {
        let remaining_len = inner.len();
        Self {
            current_offset: 0,
            remaining_len,
            inner,
        }
    }
}

impl<'a, T: FileContents> std::io::Read for FileContentsCursor<'a, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read_len = <[u8]>::len(buf).min(self.remaining_len as usize);
        // Make a silly copy
        let mut tmp_buf = Vec::with_capacity(read_len);
        self.inner
            .read_bytes_into(&mut tmp_buf, self.current_offset, read_len)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        buf[..read_len].copy_from_slice(&tmp_buf);
        self.current_offset += read_len as u64;
        self.remaining_len -= read_len as u64;
        Ok(read_len)
    }
}

impl<'a, T: FileContents> std::io::Seek for FileContentsCursor<'a, T> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        /// Returns (new_offset, new_remaining_len)
        fn inner(cur: u64, total_len: u64, pos: std::io::SeekFrom) -> Option<(u64, u64)> {
            let new_offset: u64 = match pos {
                std::io::SeekFrom::Start(pos) => pos,
                std::io::SeekFrom::End(pos) => {
                    (total_len as i64).checked_add(pos)?.try_into().ok()?
                }
                std::io::SeekFrom::Current(pos) => {
                    (cur as i64).checked_add(pos)?.try_into().ok()?
                }
            };
            let new_remaining = total_len.checked_sub(new_offset)?;
            Some((new_offset, new_remaining))
        }

        let cur = self.current_offset;
        let total_len = self.current_offset + self.remaining_len;
        match inner(cur, total_len, pos) {
            Some((cur, rem)) => {
                self.current_offset = cur;
                self.remaining_len = rem;
                Ok(cur)
            }
            None => Err(std::io::Error::new(std::io::ErrorKind::Other, "Bad Seek")),
        }
    }
}
