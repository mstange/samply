use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::{cell::Cell, future::Future};
use std::{collections::BTreeMap, fmt::Debug};

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
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Vec<PathBuf>>>>>;

    fn get_candidate_paths_for_pdb(
        &self,
        _debug_name: &str,
        _breakpad_id: &str,
        pdb_path_as_stored_in_binary: &std::ffi::CStr,
        _binary_path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Vec<PathBuf>>>>> {
        async fn single_value_path_vec(
            path: std::ffi::CString,
        ) -> FileAndPathHelperResult<Vec<PathBuf>> {
            Ok(vec![path.into_string()?.into()])
        }
        Box::pin(single_value_path_vec(
            pdb_path_as_stored_in_binary.to_owned(),
        ))
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

pub fn object_to_map<'a: 'b, 'b, T>(object_file: &'b T) -> BTreeMap<u32, &'a str>
where
    T: object::Object<'a, 'b>,
{
    use object::{ObjectSymbol, SymbolKind};
    object_file
        .dynamic_symbols()
        .chain(object_file.symbols())
        .filter(|symbol| symbol.kind() == SymbolKind::Text)
        .filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| (symbol.address() as u32, name))
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

impl<'data, T: FileContents> object::ReadRef<'data> for &'data FileContentsWrapper<T> {
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
