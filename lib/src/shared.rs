use object::read::ReadRef;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::{cell::Cell, future::Future};
use std::{collections::BTreeMap, fmt::Debug};
use std::{marker::PhantomData, ops::Deref};

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
    Normal(PathBuf),
    InDyldCache {
        dyld_cache_path: PathBuf,
        dylib_path: String,
    },
}

/// This is the trait that consumers need to implement so that they can call
/// the main entry points of this crate. This crate contains no direct file
/// access - all access to the file system is via this trait, and its associated
/// trait `FileContents`.
pub trait FileAndPathHelper {
    type F: FileContents;

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
        breakpad_id: &str,
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
        _breakpad_id: &str,
        pdb_path_as_stored_in_binary: &std::ffi::CStr,
        _binary_path: &Path,
    ) -> FileAndPathHelperResult<Vec<PathBuf>> {
        let s = std::str::from_utf8(pdb_path_as_stored_in_binary.to_bytes())?;
        Ok(vec![s.into()])
    }

    /// This method is the entry point for file access during symbolication.
    /// The implementer needs to return an object which implements the `FileContents` trait.
    /// This method is asynchronous, but once it returns, the file data needs to be
    /// available synchronously because the `FileContents` methods are synchronous.
    /// If there is no file at the requested path, an error should be returned (or in any
    /// other error case).
    fn open_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>>>>;
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
    fn read_bytes_at_until(&self, offset: u64, delimiter: u8) -> FileAndPathHelperResult<&[u8]>;
}

pub struct AddressDebugInfo {
    pub frames: Vec<InlineStackFrame>,
}

pub struct InlineStackFrame {
    pub function: Option<String>,
    pub file_path: Option<String>, // maybe PathBuf?
    pub line_number: Option<u32>,
}

pub enum SymbolicationResultKind {
    AllSymbols,
    SymbolsForAddresses { with_debug_info: bool },
}

impl SymbolicationResultKind {
    pub fn wants_debug_info_for_addresses(&self) -> bool {
        match self {
            Self::AllSymbols => false,
            Self::SymbolsForAddresses { with_debug_info } => *with_debug_info,
        }
    }
}

/// A trait which allows many "get_symbolication_result" functions to share code between
/// the implementation that constructs a full symbol table and the implementation that
/// constructs a JSON response with data per looked-up address.
pub trait SymbolicationResult {
    /// The kind of data which this result wants to carry.
    fn result_kind() -> SymbolicationResultKind;

    /// Create a `SymbolicationResult` object based on a full symbol map.
    /// Can be called regardless of `result_kind()`.
    fn from_full_map<S>(map: BTreeMap<u32, S>, addresses: &[u32]) -> Self
    where
        S: Deref<Target = str>;

    /// Create a `SymbolicationResult` object based on a set of addresses.
    /// Only called if `result_kind()` is `SymbolicationResultKind::SymbolsForAddresses`.
    /// The data for each address will be supplied by subsequent calls to `add_address_symbol`
    /// and potentially `add_address_debug_info`.
    fn for_addresses(addresses: &[u32]) -> Self;

    /// Called to supply the symbol name for a symbol.
    /// Only called if `result_kind()` is `SymbolicationResultKind::SymbolsForAddresses`, and
    /// only on objects constructed by a call to `for_addresses`.
    /// `address` is the address that the consumer wants to look up, and may fall anywhere
    /// inside a function. `symbol_address` is the closest (<= address) symbol address.
    fn add_address_symbol(&mut self, address: u32, symbol_address: u32, symbol_name: &str);

    /// Called to supply debug info for the address.
    /// Only called if `result_kind()` is `SymbolicationResultKind::SymbolsForAddresses { with_debug_info: true }`.
    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo);

    /// Supplies the total number of symbols in this binary.
    /// Only called if `result_kind()` is `SymbolicationResultKind::SymbolsForAddresses`, and
    /// only on objects constructed by a call to `for_addresses`.
    fn set_total_symbol_count(&mut self, total_symbol_count: u32);
}

/// A struct that wraps a number of parameters for various "get_symbolication_result" functions.
#[derive(Clone)]
pub struct SymbolicationQuery<'a> {
    /// The debug name of the binary whose symbols need to be looked up.
    pub debug_name: &'a str,
    /// The breakpad ID of the binary whose symbols need to be looked up.
    pub breakpad_id: &'a str,
    /// The set of addresses which need to be fed into the `SymbolicationResult`.
    /// Only used if the `SymbolicationResult`'s `result_kind()` is `SymbolsForAddresses`.
    pub addresses: &'a [u32],
}

/// Return a BTreeMap that contains address -> symbol name entries.
/// The address is relative to the address of the __TEXT segment (if present).
/// We discard the symbol "size"; the address is where the symbol starts.
pub fn object_to_map<'a: 'b, 'b, T>(object_file: &'b T) -> BTreeMap<u32, &'a str>
where
    T: object::Object<'a, 'b>,
{
    use object::read::ObjectSegment;
    use object::{ObjectSymbol, SymbolKind};
    let vmaddr_of_text_segment = object_file
        .segments()
        .find(|segment| segment.name() == Ok(Some("__TEXT")))
        .map(|segment| segment.address())
        .unwrap_or(0);

    object_file
        .dynamic_symbols()
        .chain(object_file.symbols())
        .filter(|symbol| symbol.kind() == SymbolKind::Text)
        .filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| ((symbol.address() - vmaddr_of_text_segment) as u32, name))
        })
        .collect()
}

/// A wrapper for a FileContents object. The wrapper provides some convenience methods
/// and, most importantly, implements `ReadRef` for `&FileContentsWrapper`.
pub struct FileContentsWrapper<T: FileContents> {
    file_contents: T,
    len: u64,
    bytes_read: Cell<u64>,
}

impl<T: FileContents> FileContentsWrapper<T> {
    pub fn new(file_contents: T) -> Self {
        let len = file_contents.len();
        Self {
            file_contents,
            len,
            bytes_read: Cell::new(0),
        }
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.len
    }

    #[inline]
    pub fn read_bytes_at(&self, offset: u64, size: u64) -> FileAndPathHelperResult<&[u8]> {
        self.bytes_read.set(self.bytes_read.get() + size);
        self.file_contents.read_bytes_at(offset, size)
    }

    #[inline]
    pub fn read_bytes_at_until(
        &self,
        offset: u64,
        delimiter: u8,
    ) -> FileAndPathHelperResult<&[u8]> {
        let bytes = self.file_contents.read_bytes_at_until(offset, delimiter)?;
        self.bytes_read
            .set(self.bytes_read.get() + bytes.len() as u64);
        Ok(bytes)
    }

    pub fn read_entire_data(&self) -> FileAndPathHelperResult<&[u8]> {
        self.read_bytes_at(0, self.len())
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.get()
    }

    pub fn full_range(&self) -> RangeReadRef<'_, &Self> {
        RangeReadRef::new(self, 0, self.len)
    }

    pub fn range(&self, start: u64, size: u64) -> RangeReadRef<'_, &Self> {
        RangeReadRef::new(self, start, size)
    }
}

// impl<T: FileContents> Drop for FileContentsWrapper<T> {
//     fn drop(&mut self)  {
//         eprintln!("Read {} of {} bytes.", self.bytes_read(), self.len());
//     }
// }

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
    fn read_bytes_at_until(self, offset: u64, delimiter: u8) -> Result<&'data [u8], ()> {
        self.read_bytes_at_until(offset, delimiter).map_err(|_| {
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
        self.original_readref
            .read_bytes_at(self.range_start + offset, size)
    }

    #[inline]
    fn read_bytes_at_until(self, offset: u64, delimiter: u8) -> Result<&'data [u8], ()> {
        self.original_readref
            .read_bytes_at_until(self.range_start + offset, delimiter)
    }
}
