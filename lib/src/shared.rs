use object::read::ReadRef;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::{cell::Cell, future::Future};
use std::{collections::BTreeMap, fmt::Debug};
use std::{marker::PhantomData, ops::Deref};

pub type FileAndPathHelperError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type FileAndPathHelperResult<T> = std::result::Result<T, FileAndPathHelperError>;

pub trait FileContents {
    fn len(&self) -> u64;
    fn read_bytes_at<'a>(&'a self, offset: u64, size: u64) -> FileAndPathHelperResult<&'a [u8]>;
}

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

pub trait FileAndPathHelper {
    type F: FileContents;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> FileAndPathHelperResult<Vec<PathBuf>>;

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

    fn open_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>>>>;
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

pub trait SymbolicationResult {
    fn from_full_map<S>(map: BTreeMap<u32, S>, addresses: &[u32]) -> Self
    where
        S: Deref<Target = str>;

    fn for_addresses(addresses: &[u32]) -> Self;

    fn result_kind() -> SymbolicationResultKind;

    fn add_address_symbol(&mut self, address: u32, symbol_address: u32, symbol_name: &str);
    fn add_address_debug_info(&mut self, address: u32, info: AddressDebugInfo);
    fn set_total_symbol_count(&mut self, total_symbol_count: u32);
}

#[derive(Clone)]
pub struct SymbolicationQuery<'a> {
    pub debug_name: &'a str,
    pub breakpad_id: &'a str,
    pub path: &'a Path,
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
    pub fn read_bytes_at<'a>(
        &'a self,
        offset: u64,
        size: u64,
    ) -> FileAndPathHelperResult<&'a [u8]> {
        self.bytes_read.set(self.bytes_read.get() + size);
        self.file_contents.read_bytes_at(offset, size)
    }

    pub fn read_entire_data(&self) -> FileAndPathHelperResult<&[u8]> {
        self.read_bytes_at(0, self.len())
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.get()
    }

    pub fn full_range<'a>(&'a self) -> RangeReadRef<'a, &'a Self> {
        RangeReadRef::new(self, 0, self.len)
    }

    pub fn range<'a>(&'a self, start: u64, size: u64) -> RangeReadRef<'a, &'a Self> {
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
}
