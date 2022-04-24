use debugid::DebugId;
use object::read::ReadRef;
use object::SymbolKind;
use std::borrow::Cow;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::future::Future;
use std::ops::Range;
use std::path::PathBuf;
use std::{marker::PhantomData, ops::Deref};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileLocation {
    Path(PathBuf),
    Custom(String),
}

impl FileLocation {
    pub fn to_string_lossy(&self) -> String {
        match self {
            FileLocation::Path(path) => path.to_string_lossy().to_string(),
            FileLocation::Custom(string) => string.clone(),
        }
    }
}

/// This is the trait that consumers need to implement so that they can call
/// the main entry points of this crate. This crate contains no direct file
/// access - all access to the file system is via this trait, and its associated
/// trait `FileContents`.
pub trait FileAndPathHelper<'h> {
    type F: FileContents;
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
    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        debug_id: &DebugId,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>>;

    /// This method can usually be ignored and does not need to be implemented; its default
    /// implementation is usually what you want.
    ///
    /// This is called in the following case: Let's say you're trying to look up symbols
    /// for "example.pdb". The implementer of this trait might not know the location of
    /// a suitable "example.pdb", but they might know the location of a relevant "example.exe".
    /// They can return the path to the "example.exe" from `get_candidate_paths_for_binary_or_pdb`.
    /// Symbolication will look at the exe file, and find a PDB reference inside it, with an
    /// absolute path to a PDB file. Then this method will be called, allowing the trait
    /// implementer to add more PDB candidate paths based on the PDB path from the exe.
    ///
    /// I'm actually not sure when that ability would ever be useful. Maybe this method
    /// should just be removed again.
    fn get_candidate_paths_for_pdb(
        &self,
        _debug_name: &str,
        _debug_id: &DebugId,
        pdb_path_as_stored_in_binary: &std::ffi::CStr,
        _binary_file_location: &FileLocation,
    ) -> FileAndPathHelperResult<Vec<FileLocation>> {
        let s = std::str::from_utf8(pdb_path_as_stored_in_binary.to_bytes())?;
        Ok(vec![FileLocation::Path(s.into())])
    }

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

#[derive(Debug, Clone)]
pub struct AddressDebugInfo {
    /// Must be non-empty. Ordered from inside to outside.
    /// The last frame is the outer function, the other frames are inlined functions.
    pub frames: Vec<InlineStackFrame>,
}

#[derive(Debug, Clone)]
pub struct InlineStackFrame {
    pub function: Option<String>,
    pub file_path: Option<FilePath>, // maybe PathBuf?
    pub line_number: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum FilePath {
    Normal(String),
    Mapped { raw: String, mapped: String },
}

impl FilePath {
    pub fn mapped_path(&self) -> &str {
        match self {
            FilePath::Normal(s) => s,
            FilePath::Mapped { mapped, .. } => mapped,
        }
    }

    pub fn into_mapped_path(self) -> String {
        match self {
            FilePath::Normal(s) => s,
            FilePath::Mapped { mapped, .. } => mapped,
        }
    }

    pub fn raw_path(&self) -> &str {
        match self {
            FilePath::Normal(s) => s,
            FilePath::Mapped { raw, .. } => raw,
        }
    }

    pub fn into_raw_path(self) -> String {
        match self {
            FilePath::Normal(s) => s,
            FilePath::Mapped { raw, .. } => raw,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SymbolicationResultKind<'a> {
    AllSymbols,
    SymbolsForAddresses {
        addresses: &'a [u32],
        with_debug_info: bool,
    },
}

impl<'a> SymbolicationResultKind<'a> {
    pub fn wants_debug_info_for_addresses(&self) -> bool {
        match self {
            Self::AllSymbols => false,
            Self::SymbolsForAddresses {
                with_debug_info, ..
            } => *with_debug_info,
        }
    }
}

/// A trait which allows many "get_symbolication_result" functions to share code between
/// the implementation that constructs a full symbol table and the implementation that
/// constructs a JSON response with data per looked-up address.
pub trait SymbolicationResult {
    /// Create a `SymbolicationResult` object based on a full symbol map.
    /// Only called if `result_kind` is `SymbolicationResultKind::AllSymbols`.
    fn from_full_map<S>(map: Vec<(u32, S)>) -> Self
    where
        S: Deref<Target = str>;

    /// Create a `SymbolicationResult` object based on a set of addresses.
    /// Only called if `result_kind` is `SymbolicationResultKind::SymbolsForAddresses`.
    /// The data for each address will be supplied by subsequent calls to `add_address_symbol`
    /// and potentially `add_address_debug_info`.
    fn for_addresses(addresses: &[u32]) -> Self;

    /// Called to supply the symbol name for a symbol.
    /// Only called if `result_kind` is `SymbolicationResultKind::SymbolsForAddresses`, and
    /// only on objects constructed by a call to `for_addresses`.
    /// `address` is the address that the consumer wants to look up, and may fall anywhere
    /// inside a function. `symbol_address` is the closest (<= address) symbol address.
    fn add_address_symbol(
        &mut self,
        address: u32,
        symbol_address: u32,
        symbol_name: &str,
        function_size: Option<u32>,
    );

    /// Called to supply debug info for the address.
    /// Only called if `result_kind` is `SymbolicationResultKind::SymbolsForAddresses { with_debug_info: true }`.
    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo);

    /// Supplies the total number of symbols in this binary.
    /// Only called if `result_kind` is `SymbolicationResultKind::SymbolsForAddresses`, and
    /// only on objects constructed by a call to `for_addresses`.
    fn set_total_symbol_count(&mut self, total_symbol_count: u32);
}

/// A struct that wraps a number of parameters for various "get_symbolication_result" functions.
#[derive(Clone)]
pub struct SymbolicationQuery<'a> {
    /// The debug name of the binary whose symbols need to be looked up.
    pub debug_name: &'a str,
    /// The debug ID of the binary whose symbols need to be looked up.
    pub debug_id: DebugId,
    /// The kind of data which this query wants have returned.
    pub result_kind: SymbolicationResultKind<'a>,
}

/// In the symbolication query, the requested addresses are in "relative address" form.
/// This is in contrast to the u64 "vmaddr" form which is used by section
/// addresses, symbol addresses and DWARF pc offset information.
///
/// Relative addresses are u32 offsets which are relative to some "base address".
///
/// This function computes that base address. It is defined as follows:
///
///  - For Windows binaries, the base address is the "image base address".
///  - For mach-O binaries, the base address is the vmaddr of the __TEXT segment.
///  - For kernel ELF binaries ("vmlinux"), we define the base address as the
///    vmaddr of the .text section. This may not have a precedent, but it's an
///    address which is readily available in the Linux `perf` case which motivated
///    this special treatment.
///  - For other ELF binaries, the base address is zero.
///
/// In many cases, this base address is simply zero:
///
///  - Non-kernel ELF images are treated as having a base address of zero.
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
///  - The .text section of a vmlinux image can be at a very high address such
///    as 0xffffffff81000000.
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

    use object::ObjectSection;
    if let Some(text_section) = object_file.section_by_name_bytes(b".text") {
        // Detect kernel images.
        // TODO: There is probably a better way to detect this.
        if text_section.address() >= 0xffffffff80000000 {
            // This is a kernel image (vmlinux). Relative addresses are relative to the
            // text section.
            // (This decision is up for discussion. I chose this option because perf.data
            // has synthetic MMAP events for a "[kernel.kallsyms]_text" image, so this
            // choice makes things simple and allows relative addresses to fit in a u32.)
            return text_section.address();
        }
    }

    // For PE binaries, relative_address_base() returns the image base address.
    object_file.relative_address_base()
}

/// Return a Vec that contains address -> symbol name entries.
/// The address is relative to the address of the __TEXT segment (if present).
/// We discard the symbol "size"; the address is where the symbol starts.
pub fn object_to_map<'a: 'b, 'b, T>(
    object_file: &'b T,
    function_start_addresses: Option<&[u32]>,
) -> Vec<(u32, String)>
where
    T: object::Object<'a, 'b>,
{
    use object::ObjectSymbol;
    let image_base = relative_address_base(object_file);

    let mut map: Vec<(u32, String)> = Vec::new();

    if let Some(function_start_addresses) = function_start_addresses {
        // Begin with fallback function start addresses, with synthesized symbols of the form fun_abcdef.
        // We add these first so that they'll act as the ultimate fallback.
        // These synhesized symbols make it so that, for libraries which only contain symbols
        // for a small subset of their functions, we will show placeholder function names
        // rather than plain incorrect function names.
        map.extend(
            function_start_addresses
                .iter()
                .map(|address| (*address, format!("fun_{:x}", address))),
        );
    }

    // Add any symbols found in this library.
    map.extend(
        object_file
            .dynamic_symbols()
            .chain(object_file.symbols())
            .filter(|symbol| symbol.kind() == SymbolKind::Text)
            .filter_map(|symbol| {
                symbol
                    .name()
                    .ok()
                    .map(|name| ((symbol.address() - image_base) as u32, name.to_string()))
            }),
    );

    // For PE files, add the exports.
    if let Ok(exports) = object_file.exports() {
        for export in exports {
            if let Ok(name) = std::str::from_utf8(export.name()) {
                map.push(((export.address() - image_base) as u32, name.to_string()));
            }
        }
    }
    map
}

enum FullSymbolListEntry<'a, Symbol: object::ObjectSymbol<'a>> {
    Synthesized,
    Symbol(Symbol),
    Export(object::Export<'a>),
    EndAddress,
}

impl<'a, Symbol: object::ObjectSymbol<'a>> FullSymbolListEntry<'a, Symbol> {
    fn name(&self, addr: u32) -> Result<Cow<str>, ()> {
        match self {
            FullSymbolListEntry::Synthesized => Ok(format!("fun_{:x}", addr).into()),
            FullSymbolListEntry::Symbol(symbol) => match symbol.name() {
                Ok(name) => Ok(Cow::Borrowed(name)),
                Err(_) => Err(()),
            },
            FullSymbolListEntry::Export(export) => match std::str::from_utf8(export.name()) {
                Ok(name) => Ok(Cow::Borrowed(name)),
                Err(_) => Err(()),
            },
            FullSymbolListEntry::EndAddress => Err(()),
        }
    }
}

/// Return a Vec that contains address -> symbol name entries.
/// The address is relative to the address of the __TEXT segment (if present).
/// We discard the symbol "size"; the address is where the symbol starts.
pub fn get_symbolication_result_for_addresses_from_object<'a: 'b, 'b, T, R>(
    addresses: &[u32],
    object_file: &'b T,
    function_start_addresses: Option<&[u32]>,
    function_end_addresses: Option<&[u32]>,
) -> R
where
    T: object::Object<'a, 'b>,
    R: SymbolicationResult,
{
    let mut entries: Vec<_> = Vec::new();

    let base_address = relative_address_base(object_file);

    // Add entries in the order "best to worst".

    // 1. Normal symbols
    // 2. Dynamic symbols (only used by ELF files, I think)
    use object::{ObjectSection, ObjectSymbol};
    entries.extend(
        object_file
            .symbols()
            .chain(object_file.dynamic_symbols())
            .filter(|symbol| symbol.kind() == SymbolKind::Text)
            .map(|symbol| {
                (
                    (symbol.address() - base_address) as u32,
                    FullSymbolListEntry::Symbol(symbol),
                )
            }),
    );

    // 3. Exports (only used by exe / dll objects)
    if let Ok(exports) = object_file.exports() {
        for export in exports {
            entries.push((
                (export.address() - base_address) as u32,
                FullSymbolListEntry::Export(export),
            ));
        }
    }

    // 4. Placeholder symbols based on function start addresses
    if let Some(function_start_addresses) = function_start_addresses {
        // Use function start addresses with synthesized symbols of the form fun_abcdef
        // as the ultimate fallback.
        // These synhesized symbols make it so that, for libraries which only contain symbols
        // for a small subset of their functions, we will show placeholder function names
        // rather than plain incorrect function names.
        entries.extend(
            function_start_addresses
                .iter()
                .map(|address| (*address, FullSymbolListEntry::Synthesized)),
        );
    }

    // 5. End addresses from section ends
    // These entries serve to "terminate" the last function of each section,
    // so that addresses in the following section are not considered
    // to be part of the last function of that previous section.
    entries.extend(object_file.sections().filter_map(|section| {
        let vma_end_address = section.address().checked_add(section.size())?;
        let end_address = vma_end_address.checked_sub(base_address)?;
        let end_address = u32::try_from(end_address).ok()?;
        Some((end_address, FullSymbolListEntry::EndAddress))
    }));

    // 6. End addresses for known functions ends
    // These addresses serve to "terminate" functions from function_start_addresses.
    // They come from .eh_frame or .pdata info, which has the function size.
    if let Some(function_end_addresses) = function_end_addresses {
        entries.extend(
            function_end_addresses
                .iter()
                .map(|address| (*address, FullSymbolListEntry::EndAddress)),
        );
    }

    // Done.
    // Now that all entries are added, sort and de-duplicate so that we only
    // have one entry per address.
    // If multiple entries for the same address are present, only the first
    // entry for that address is kept. (That's also why we use a stable sort
    // here.)
    // We have added entries in the order best to worst, so we keep the "best"
    // symbol for each address.
    entries.sort_by_key(|(address, _)| *address);
    entries.dedup_by_key(|(address, _)| *address);

    let mut symbolication_result = R::for_addresses(addresses);
    symbolication_result.set_total_symbol_count(entries.len() as u32);

    for &address in addresses {
        let index = match entries.binary_search_by_key(&address, |&(addr, _)| addr) {
            Err(0) => continue,
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let (start_addr, entry) = &entries[index];
        let next_entry = entries.get(index + 1);
        // Add the symbol.
        // If the found entry is an EndAddress entry, this means that `address` falls
        // in the dead space between known functions, and we consider it to be not found.
        if let (Ok(name), Some((end_addr, _))) = (entry.name(*start_addr), next_entry) {
            let function_size = end_addr - *start_addr;
            symbolication_result.add_address_symbol(
                address,
                *start_addr,
                &name,
                Some(function_size),
            );
        }
    }
    symbolication_result
}

/// Implementation for slices.
impl<T: Deref<Target = [u8]>> FileContents for T {
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
        Ok(self.len() as u64)
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
