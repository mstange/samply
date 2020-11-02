use addr2line::object;
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::BTreeMap;
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub trait OwnedFileData {
    fn get_data(&self) -> &[u8];
}

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

pub trait FileAndPathHelper {
    type FileContents: OwnedFileData;

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

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::FileContents>>>>;
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
    T: Object<'a, 'b>,
{
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
