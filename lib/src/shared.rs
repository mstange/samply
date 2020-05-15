use object::{Object, SymbolKind};
use std::collections::HashMap;
use std::ops::Deref;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub trait OwnedFileData {
    fn get_data(&self) -> &[u8];
}

pub type FileAndPathHelperError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type FileAndPathHelperResult<T> = std::result::Result<T, FileAndPathHelperError>;

pub trait FileAndPathHelper {
    type FileContents: OwnedFileData;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>>>>;

    fn get_candidate_paths_for_pdb(
        &self,
        _debug_name: &str,
        _breakpad_id: &str,
        pdb_path_as_stored_in_binary: &std::ffi::CStr,
        _binary_path: &Path,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>>>> {
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
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::FileContents>>>>;
}

pub trait SymbolicationResult {
    fn from_map<S>(map: HashMap<u32, S>, addresses: &[u32]) -> Self
    where
        S: Deref<Target = str>;
}

#[derive(Clone)]
pub struct SymbolicationQuery<'a> {
    pub debug_name: &'a str,
    pub breakpad_id: &'a str,
    pub path: &'a Path,
    pub addresses: &'a [u32],
}

pub fn object_to_map<'a, 'b, T>(object_file: &'b T) -> HashMap<u32, &'a str>
where
    T: Object<'a, 'b>,
{
    object_file
        .dynamic_symbols()
        .chain(object_file.symbols())
        .filter(|(_, symbol)| symbol.kind() == SymbolKind::Text)
        .filter_map(|(_, symbol)| symbol.name().map(|name| (symbol.address() as u32, name)))
        .collect()
}
