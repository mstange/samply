use debugid::DebugId;
use object::read::ReadRef;
use object::FileFlags;
use std::borrow::Cow;
use std::fmt::Debug;
use std::future::Future;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{marker::PhantomData, ops::Deref};
use uuid::Uuid;

#[cfg(feature = "partial_read_stats")]
use bitvec::{bitvec, prelude::BitVec};
#[cfg(feature = "partial_read_stats")]
use std::cell::RefCell;

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

pub enum CandidatePathInfo {
    SingleFile(FileLocation),
    InDyldCache {
        dyld_cache_path: PathBuf,
        dylib_path: String,
    },
}

/// In case the loaded binary contains multiple architectures, this specifies
/// how to resolve the ambiguity. This is only needed on macOS.
#[derive(Debug, Clone)]
pub enum MultiArchDisambiguator {
    /// Disambiguate by CPU architecture.
    ///
    /// This string is a mach-O string for what mach-O calls the "CPU type" and "CPU subtype".
    /// Examples are `x86_64`, `x86_64h`, `arm64`, `arm64e`.
    ///
    /// These strings are returned by the mach function `macho_arch_name_for_cpu_type`.
    Arch(String),

    /// Disambiguate by `DebugId`.
    DebugId(DebugId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileLocation {
    /// A path to a local file. Symbol files at local paths are allowed to refer
    /// to source files on the local file system. Those source file references
    /// will be returned as local `FilePath`s from the symbolication API.
    Path(PathBuf),

    /// A special string that identifies some way of obtaining the file. The string
    /// gets interpreted by the implementation of `FileAndPathHelper::open_file`.
    /// Files from this location type cannot refer to source files on the local
    /// file system; any source file references in them are returned as
    /// `FilePath::NonLocal`.
    Custom(String),
}

impl FileLocation {
    pub fn to_string_lossy(&self) -> String {
        match self {
            FileLocation::Path(path) => path.to_string_lossy().to_string(),
            FileLocation::Custom(string) => string.clone(),
        }
    }

    pub fn to_base_path(&self) -> BasePath {
        match self {
            FileLocation::Path(file_path) => {
                let base_path = file_path.parent().unwrap_or(file_path);
                BasePath::CanReferToLocalFiles(base_path.to_owned())
            }
            FileLocation::Custom(_) => BasePath::NoLocalSourceFileAccess,
        }
    }
}

/// THIS IS NOT [`debugid::CodeId`].
///
/// This is an enum which tells you which type of CodeId it is, because all
/// types need to be treated rather differently.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodeId {
    /// On Windows, the code ID needs to be combined with the binary name, and
    /// the pair (name, code_id) lets you obtain binaries from symbol servers.
    /// The code ID is distinct from the debug_id (= pdb GUID + age). If you
    /// have a binary file, you can get both the code ID and the debug ID from
    /// it. If you only have a PDB file, you can usually not get the code ID of
    /// the corresponding binary from it.
    PeCodeId(PeCodeId),

    /// On macOS / iOS, the UUID is shared between both the binary file and
    /// the debug file (dSYM), and it can be used to find dSYMs using Spotlight.
    /// The debug ID and the code ID contain the same information; the debug ID
    /// is literally just the UUID plus a zero at the end.
    MachoUuid(Uuid),

    /// On Linux, the ELF build ID is strictly superior to the debug ID, because
    /// it is usually a superset of information: The build ID is usually 20 bytes
    /// (40 hex chars), whereas the debug ID is truncated to 16 bytes (32 hex chars)
    /// and byte-swapped depending on the endianness of the binary.
    ///
    /// The ELF build ID is shared between the binary file and the debug info file.
    /// With debuginfod you can obtain binaries and debug info files from a server
    /// based on just the build ID (no binary name needed).
    ElfBuildId(ElfBuildId),
}

impl FromStr for CodeId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() <= 17 {
            // 8 bytes timestamp + 1 to 8 bytes of image size
            Ok(CodeId::PeCodeId(PeCodeId::from_str(s)?))
        } else if s.len() == 32 {
            // mach-O UUID
            Ok(CodeId::MachoUuid(Uuid::from_str(s).map_err(|_| ())?))
        } else if s.len() >= 34 {
            // ELF build ID. These are usually 40 hex characters (= 20 bytes).
            Ok(CodeId::ElfBuildId(ElfBuildId::from_str(s)?))
        } else {
            Err(())
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElfBuildId(pub Vec<u8>);

impl ElfBuildId {
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
            self.debug_name = other.debug_name.clone();
        }
        if self.debug_id.is_none() && other.debug_id.is_some() {
            self.debug_id = other.debug_id;
        }
        if self.debug_path.is_none() && other.debug_path.is_some() {
            self.debug_path = other.debug_path.clone();
        }
        if self.name.is_none() && other.name.is_some() {
            self.name = other.name.clone();
        }
        if self.code_id.is_none() && other.code_id.is_some() {
            self.code_id = other.code_id.clone();
        }
        if self.path.is_none() && other.path.is_some() {
            self.path = other.path.clone();
        }
        if self.arch.is_none() && other.arch.is_some() {
            self.arch = other.arch.clone();
        }
    }
}

/// This is the trait that consumers need to implement so that they can call
/// the main entry points of this crate. This crate contains no direct file
/// access - all access to the file system is via this trait, and its associated
/// trait `FileContents`.
pub trait FileAndPathHelper<'h> {
    type F: FileContents + 'static;

    type OpenFileFuture: OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h;

    /// Given a "debug name" and a "breakpad ID", return a list of file paths
    /// which may potentially have artifacts containing symbol data for the
    /// requested binary (executable or library).
    ///
    /// The symbolication methods will try these paths one by one, calling
    /// `open_file` for each until it succeeds and finds a file whose contents
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
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>>;

    /// TODO
    fn get_candidate_paths_for_binary(
        &self,
        info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>>;

    /// TODO
    fn get_dyld_shared_cache_paths(
        &self,
        arch: Option<&str>,
    ) -> FileAndPathHelperResult<Vec<PathBuf>>;

    /// This method is the entry point for file access during symbolication.
    /// The implementer needs to return an object which implements the `FileContents` trait.
    /// This method is asynchronous, but once it returns, the file data needs to be
    /// available synchronously because the `FileContents` methods are synchronous.
    /// If there is no file at the requested path, an error should be returned (or in any
    /// other error case).
    fn open_file(&'h self, location: &FileLocation) -> Self::OpenFileFuture;
}

/// Provides synchronous access to the raw bytes of a file.
/// This trait needs to be implemented by the consumer of this crate.
#[cfg(not(feature = "send_futures"))]
pub trait FileContents {
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

#[cfg(feature = "send_futures")]
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

#[derive(Debug, Clone)]
pub struct AddressDebugInfo {
    /// Must be non-empty. Ordered from inside to outside.
    /// The last frame is the outer function, the other frames are inlined functions.
    pub frames: Vec<InlineStackFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineStackFrame {
    pub function: Option<String>,
    pub file_path: Option<FilePath>, // maybe PathBuf?
    pub line_number: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum BasePath {
    /// Indicates that the symbol file did not originate on this machine.
    /// Any `FilePath`s created from this base path will be non-local.
    NoLocalSourceFileAccess,

    /// Indicates that the symbol file is a local file. Any `FilePath`
    /// created from this base path will be local. If the symbol file
    /// contains relative paths, those relative paths will be turned into
    /// absolute local paths by appending them to this base path.
    CanReferToLocalFiles(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePath {
    /// A local symbol file refers to a local path. No path mapping was applied.
    Local(PathBuf),

    /// A local symbol file refers to a local path but also has a mapped variant
    /// of that path which we prefer to return from the symbolication API.
    ///
    /// Examples:
    ///
    ///   - Local ELF file with DWARF info which refers to a path in a Rust
    ///     dependency. We have a local source file with the dependency's code
    ///     (whose location is in `local`) but in the API result we return a
    ///     special path of the type cargo:...:...
    ///   - Local pdb file with a srcsrv stream which maps local paths to github
    ///     URLs. We have a local file at the raw path but in the API result we
    ///     return a special path of the type git:...:...
    ///   - Local ELF file with DWARF info which specifies a **relative** path,
    ///     resolved relative to the location of that ELF file. We store the
    ///     resolved absolute path in `local` and the relative path in `mapped`.
    LocalMapped { local: PathBuf, mapped: String },

    /// A non-local symbol file refers to a path which may or may not have been
    /// mapped. If it was mapped, we discard the original raw path.
    ///
    /// Non-local symbol files aren't allowed to refer to files on this file
    /// system, so we don't need to know the pre-mapping path.
    ///
    /// Examples:
    ///
    ///   - A pdb file was downloaded from a symbol server and refers to a source
    ///     file with an absolute path which was valid on the original build
    ///     machine where this pdb file was produced. We store that absolute
    ///     path but we don't want to open a file at that path on this machine
    ///     because the pdb file came from somewhere else.
    ///   - Same as the previous example, but with a srcsrv stream which maps
    ///     the absolute path to a github URL. We map the path to a special path
    ///     of the type git:...:... and store only the mapped path.
    NonLocal(String),
}

impl FilePath {
    pub fn mapped_path(&self) -> Cow<str> {
        match self {
            FilePath::Local(local) => local.to_string_lossy(),
            FilePath::LocalMapped { mapped, .. } => mapped.into(),
            FilePath::NonLocal(s) => s.into(),
        }
    }

    pub fn into_mapped_path(self) -> String {
        match self {
            FilePath::Local(local) => local.to_string_lossy().into(),
            FilePath::LocalMapped { mapped, .. } => mapped,
            FilePath::NonLocal(s) => s,
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            FilePath::Local(local) => Some(local),
            FilePath::LocalMapped { local, .. } => Some(local),
            FilePath::NonLocal(_) => None,
        }
    }

    pub fn into_local_path(self) -> Option<PathBuf> {
        match self {
            FilePath::Local(local) => Some(local),
            FilePath::LocalMapped { local, .. } => Some(local),
            FilePath::NonLocal(_) => None,
        }
    }
}

/// In calls to SymbolMap::lookup, the requested addresses are in "relative address" form.
/// This is in contrast to the u64 "vmaddr" form which is used by section
/// addresses, symbol addresses and DWARF pc offset information.
///
/// Relative addresses are u32 offsets which are relative to some "base address".
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
pub fn relative_address_base<'data: 'file, 'file>(
    object_file: &'file impl object::Object<'data, 'file>,
) -> u64 {
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
    /// Information about the frames at the looked up address, from the debug info.
    pub frames: FramesLookupResult,
}

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
    Available(Vec<InlineStackFrame>),

    /// Debug info for this address was not found in the symbol map, but can
    /// potentially be found in a different file, with the help of
    /// `SymbolManager::lookup_external`.
    ///
    /// This case can currently only be hit on macOS: On macOS, linking multiple
    /// `.o` files together into a library or an executable does not copy the
    /// DWARF information into the linked output. Instead, the linker stores the
    /// paths to those original `.o` files, using 'OSO' stabs entries, and debug
    /// info must be obtained from those original files.
    External(ExternalFileAddressRef),

    /// No debug info is available.
    Unavailable,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalFileAddressRef {
    pub file_ref: ExternalFileRef,
    pub address_in_file: ExternalFileAddressInFileRef,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalFileRef {
    pub file_name: String,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalFileAddressInFileRef {
    pub name_in_archive: Option<String>,
    pub symbol_name: Vec<u8>,
    pub offset_from_symbol: u32,
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
    partial_read_stats: RefCell<FileReadStats>,
}

impl<T: FileContents> FileContentsWrapper<T> {
    pub fn new(file_contents: T) -> Self {
        let len = file_contents.len();
        Self {
            file_contents,
            len,
            #[cfg(feature = "partial_read_stats")]
            partial_read_stats: RefCell::new(FileReadStats::new(len)),
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
            .borrow_mut()
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
            .borrow_mut()
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
            .borrow_mut()
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
        eprintln!("{}", self.partial_read_stats.borrow());
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
