use pdb_addr2line::pdb::Error as PDBError;
use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, GetSymbolsError>;

#[derive(Error, Debug)]
pub enum GetSymbolsError {
    #[error("Unmatched breakpad_id: Expected {0}, but received {1}")]
    UnmatchedBreakpadId(String, String),

    #[error("No match in multi-arch binary, available UUIDs: {}, errors: {}", .0.join(", "), .1.iter().map(|e| format!("{}", e)).collect::<Vec<String>>().join(", "))]
    NoMatchMultiArch(Vec<String>, Vec<GetSymbolsError>),

    #[error("Couldn't get symbols from system library, errors: {}", .0.iter().map(|e| format!("{}", e)).collect::<Vec<String>>().join(", "))]
    NoLuckMacOsSystemLibrary(Vec<GetSymbolsError>),

    #[error("PDB error: {1} ({0})")]
    PDBError(&'static str, PDBError),

    #[error("Invalid input: {0}")]
    InvalidInputError(&'static str),

    #[error("Object could not parse the file as {0:?}: {1}")]
    ObjectParseError(object::read::FileKind, #[source] object::read::Error),

    #[error("Dyld cache parsing error: {0}")]
    DyldCacheParseError(#[source] object::read::Error),

    #[error("The dyld shared cache file did not include an entry for the dylib at {0}")]
    NoMatchingDyldCacheImagePath(String),

    #[error("MachOHeader parsing error: {0}")]
    MachOHeaderParseError(#[source] object::read::Error),

    #[error(
        "get_candidate_paths_for_binary_or_pdb helper callback for {0} {1} returned error: {2}"
    )]
    HelperErrorDuringGetCandidatePathsForBinaryOrPdb(
        String,
        String,
        #[source] Box<dyn std::error::Error + Send + Sync>,
    ),

    #[error("get_candidate_paths_for_pdb helper callback for {0} {1} returned error: {2}")]
    HelperErrorDuringGetCandidatePathsForPdb(
        String,
        String,
        #[source] Box<dyn std::error::Error + Send + Sync>,
    ),

    #[error("open_file helper callback for file {0} returned error: {1}")]
    HelperErrorDuringOpenFile(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("FileContents read_bytes_at for file {0} returned error: {1}")]
    HelperErrorDuringFileReading(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("No candidate path for binary, for {0} {1}")]
    NoCandidatePathForBinary(String, String),

    #[error("The PE (Windows) binary at path {0} did not contain information about an associated PDB file")]
    NoDebugInfoInPeBinary(String),

    #[error("In the PE (Windows) binary at path {0}, the embedded path to the PDB file did not end with a nul byte.")]
    PdbPathDidntEndWithNul(String),

    #[error("Could not parse archive file at {0}, ArchiveFile::parse returned error: {1}.")]
    ArchiveParseError(PathBuf, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Couldn't parse request: {0}")]
    ParseRequestErrorSerde(#[from] serde_json::error::Error),

    #[error("Malformed request JSON: {0}")]
    ParseRequestErrorContents(&'static str),

    #[error("Error while formatting function from PDB: {0}")]
    PdbTypeFormatterError(#[source] pdb_addr2line::Error),
}

pub trait Context<T> {
    fn context(self, context_description: &'static str) -> Result<T>;
}

impl<T> Context<T> for std::result::Result<T, PDBError> {
    fn context(self, context_description: &'static str) -> Result<T> {
        self.map_err(|e| GetSymbolsError::PDBError(context_description, e))
    }
}

impl From<PDBError> for GetSymbolsError {
    fn from(err: PDBError) -> GetSymbolsError {
        GetSymbolsError::PDBError("Unknown", err)
    }
}

impl From<pdb_addr2line::Error> for GetSymbolsError {
    fn from(err: pdb_addr2line::Error) -> GetSymbolsError {
        GetSymbolsError::PdbTypeFormatterError(err)
    }
}

impl GetSymbolsError {
    pub fn enum_as_string(&self) -> &'static str {
        match *self {
            GetSymbolsError::UnmatchedBreakpadId(_, _) => "UnmatchedBreakpadId",
            GetSymbolsError::NoMatchMultiArch(_, _) => "NoMatchMultiArch",
            GetSymbolsError::NoLuckMacOsSystemLibrary(_) => "NoLuckMacOsSystemLibrary",
            GetSymbolsError::PDBError(_, _) => "PDBError",
            GetSymbolsError::InvalidInputError(_) => "InvalidInputError",
            GetSymbolsError::DyldCacheParseError(_) => "DyldCacheParseError",
            GetSymbolsError::NoMatchingDyldCacheImagePath(_) => "NoMatchingDyldCacheImagePath",
            GetSymbolsError::ObjectParseError(_, _) => "ObjectParseError",
            GetSymbolsError::MachOHeaderParseError(_) => "MachOHeaderParseError",
            GetSymbolsError::HelperErrorDuringGetCandidatePathsForBinaryOrPdb(_, _, _) => {
                "HelperErrorDuringGetCandidatePathsForBinaryOrPdb"
            }
            GetSymbolsError::HelperErrorDuringGetCandidatePathsForPdb(_, _, _) => {
                "HelperErrorDuringGetCandidatePathsForPdb"
            }
            GetSymbolsError::HelperErrorDuringOpenFile(_, _) => "HelperErrorDuringOpenFile",
            GetSymbolsError::HelperErrorDuringFileReading(_, _) => "HelperErrorDuringFileReading",
            GetSymbolsError::NoCandidatePathForBinary(_, _) => "NoCandidatePathForBinary",
            GetSymbolsError::NoDebugInfoInPeBinary(_) => "NoDebugInfoInPeBinary",
            GetSymbolsError::PdbPathDidntEndWithNul(_) => "PdbPathDidntEndWithNul",
            GetSymbolsError::ArchiveParseError(_, _) => "ArchiveParseError",
            GetSymbolsError::ParseRequestErrorSerde(_) => "ParseRequestErrorSerde",
            GetSymbolsError::ParseRequestErrorContents(_) => "ParseRequestErrorContents",
            GetSymbolsError::PdbTypeFormatterError(_) => "PdbTypeFormatterError",
        }
    }
}
