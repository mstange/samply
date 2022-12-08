use debugid::DebugId;
use pdb_addr2line::pdb::Error as PdbError;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("Unmatched breakpad_id: Expected {0}, but received {1}")]
    UnmatchedDebugId(DebugId, DebugId),

    #[error("Unmatched breakpad_id: Expected {0}, but received {1:?}")]
    UnmatchedDebugIdOptional(DebugId, Option<DebugId>),

    #[error("Invalid breakpad ID {0}")]
    InvalidBreakpadId(String),

    #[error("Not enough information was supplied to identify the requested binary.")]
    NotEnoughInformationToIdentifyBinary,

    #[error(
        "Got fat archive but no debug ID was supplied to disambiguate between archive members"
    )]
    NoDisambiguatorForFatArchive,

    #[error("No match in multi-arch binary, available UUIDs: {}, errors: {}", .0.iter().map(|di| di.breakpad().to_string()).collect::<Vec<String>>().join(", "), .1.iter().map(|e| format!("{}", e)).collect::<Vec<String>>().join(", "))]
    NoMatchMultiArch(Vec<DebugId>, Vec<Error>),

    #[error("Couldn't get symbols from system library, errors: {}", .0.iter().map(|e| format!("{}", e)).collect::<Vec<String>>().join(", "))]
    NoLuckMacOsSystemLibrary(Vec<Error>),

    #[error("PDB error: {1} ({0})")]
    PdbError(&'static str, PdbError),

    #[error("pdb-addr2line error: {1} ({0})")]
    PdbAddr2lineErrorWithContext(&'static str, #[source] pdb_addr2line::Error),

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

    #[error("get_candidate_paths_for_debug_file helper callback for {0} {1} returned error: {2}")]
    HelperErrorDuringGetCandidatePathsForDebugFile(
        String,
        DebugId,
        #[source] Box<dyn std::error::Error + Send + Sync>,
    ),

    #[error("get_candidate_paths_for_binary helper callback for returned error: {0}")]
    HelperErrorDuringGetCandidatePathsForBinary(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("open_file helper callback for file {0} returned error: {1}")]
    HelperErrorDuringOpenFile(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("FileContents read_bytes_at for file {0} returned error: {1}")]
    HelperErrorDuringFileReading(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("No candidate path for binary, for {0:?} {1:?}")]
    NoCandidatePathForBinary(Option<String>, Option<DebugId>),

    #[error("No associated PDB file with the right debug ID was found for the PE (Windows) binary at path {0}")]
    NoMatchingPdbForBinary(String),

    #[error("The PE (Windows) binary at path {0} did not contain information about an associated PDB file")]
    NoDebugInfoInPeBinary(String),

    #[error("In the PE (Windows) binary at path {0}, the embedded path to the PDB file was not valid utf-8.")]
    PdbPathNotUtf8(String),

    #[error("In the PE (Windows) binary, the embedded path to the PDB file did not end with a file name: {0}")]
    PdbPathWithoutFilename(String),

    #[error("Could not parse archive file at {0}, ArchiveFile::parse returned error: {1}.")]
    ArchiveParseError(PathBuf, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Could not find file {0} in the archive file.")]
    FileNotInArchive(String),

    #[error("Error while getting function info from PDB: {0}")]
    PdbAddr2lineError(
        #[from]
        #[source]
        pdb_addr2line::Error,
    ),

    #[error("Error while parsing srcsrv stream from PDB: {0}")]
    SrcSrvParseError(
        #[from]
        #[source]
        srcsrv::ParseError,
    ),

    #[error("Error while evaluating srcsrv entry PDB: {0}")]
    SrcSrvEvalError(
        #[from]
        #[source]
        srcsrv::EvalError,
    ),

    #[error("Could not create addr2line Context: {0}")]
    Addr2lineContextCreationError(#[source] gimli::Error),
}

pub trait Context<T> {
    fn context(self, context_description: &'static str) -> Result<T, Error>;
}

impl<T> Context<T> for std::result::Result<T, PdbError> {
    fn context(self, context_description: &'static str) -> Result<T, Error> {
        self.map_err(|e| Error::PdbError(context_description, e))
    }
}

impl<T> Context<T> for std::result::Result<T, pdb_addr2line::Error> {
    fn context(self, context_description: &'static str) -> Result<T, Error> {
        self.map_err(|e| Error::PdbAddr2lineErrorWithContext(context_description, e))
    }
}

impl From<PdbError> for Error {
    fn from(err: PdbError) -> Error {
        Error::PdbError("Unknown", err)
    }
}

impl Error {
    pub fn enum_as_string(&self) -> &'static str {
        match self {
            Error::UnmatchedDebugId(_, _) => "UnmatchedDebugId",
            Error::NoDisambiguatorForFatArchive => "NoDisambiguatorForFatArchive",
            Error::NotEnoughInformationToIdentifyBinary => "NotEnoughInformationToIdentifyBinary",
            Error::UnmatchedDebugIdOptional(_, _) => "UnmatchedDebugIdOptional",
            Error::InvalidBreakpadId(_) => "InvalidBreakpadId",
            Error::NoMatchMultiArch(_, _) => "NoMatchMultiArch",
            Error::NoLuckMacOsSystemLibrary(_) => "NoLuckMacOsSystemLibrary",
            Error::PdbError(_, _) => "PdbError",
            Error::PdbAddr2lineErrorWithContext(_, _) => "PdbAddr2lineErrorWithContext",
            Error::InvalidInputError(_) => "InvalidInputError",
            Error::DyldCacheParseError(_) => "DyldCacheParseError",
            Error::NoMatchingDyldCacheImagePath(_) => "NoMatchingDyldCacheImagePath",
            Error::ObjectParseError(_, _) => "ObjectParseError",
            Error::MachOHeaderParseError(_) => "MachOHeaderParseError",
            Error::HelperErrorDuringGetCandidatePathsForDebugFile(_, _, _) => {
                "HelperErrorDuringGetCandidatePathsForDebugFile"
            }
            Error::HelperErrorDuringGetCandidatePathsForBinary(_) => {
                "HelperErrorDuringGetCandidatePathsForBinary"
            }
            Error::HelperErrorDuringOpenFile(_, _) => "HelperErrorDuringOpenFile",
            Error::HelperErrorDuringFileReading(_, _) => "HelperErrorDuringFileReading",
            Error::NoCandidatePathForBinary(_, _) => "NoCandidatePathForBinary",
            Error::NoDebugInfoInPeBinary(_) => "NoDebugInfoInPeBinary",
            Error::NoMatchingPdbForBinary(_) => "NoMatchingPdbForBinary",
            Error::PdbPathNotUtf8(_) => "PdbPathNotUtf8",
            Error::PdbPathWithoutFilename(_) => "PdbPathWithoutFilename",
            Error::ArchiveParseError(_, _) => "ArchiveParseError",
            Error::FileNotInArchive(_) => "FileNotInArchive",
            Error::PdbAddr2lineError(_) => "PdbAddr2lineError",
            Error::SrcSrvParseError(_) => "SrcSrvParseError",
            Error::SrcSrvEvalError(_) => "SrcSrvEvalError",
            Error::Addr2lineContextCreationError(_) => "Addr2lineContextCreationError",
        }
    }
}
