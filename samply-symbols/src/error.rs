use debugid::DebugId;
use linux_perf_data::jitdump::JitDumpError;
use object::FileKind;
use pdb_addr2line::pdb::Error as PdbError;
use std::path::PathBuf;
use thiserror::Error;

use crate::{breakpad::BreakpadParseError, CodeId, FatArchiveMember, LibraryInfo};

/// The error type used in this crate.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("Unmatched breakpad_id: Expected {0}, but received {1}")]
    UnmatchedDebugId(DebugId, DebugId),

    #[error("Unmatched breakpad_id: Expected {0}, but received {1:?}")]
    UnmatchedDebugIdOptional(DebugId, Option<DebugId>),

    #[error("Unmatched CodeId: Expected {0}, but received {}", .1.as_ref().map_or("<none>".into(), ToString::to_string))]
    UnmatchedCodeId(CodeId, Option<CodeId>),

    #[error("The Breakpad sym file was malformed, causing a parsing error: {0}")]
    BreakpadParsing(#[from] BreakpadParseError),

    #[error("The JITDUMP file was malformed, causing a parsing error: {0}")]
    JitDumpParsing(#[from] JitDumpError),

    #[error("Invalid index {0} for file or inline_origin in breakpad sym file")]
    InvalidFileOrInlineOriginIndexInBreakpadFile(u32),

    #[error("Invalid breakpad ID {0}")]
    InvalidBreakpadId(String),

    #[error("Not enough information was supplied to identify the requested binary.")]
    NotEnoughInformationToIdentifyBinary,

    #[error("Could not determine the FileKind of the external file.")]
    CouldNotDetermineExternalFileFileKind,

    #[error("External file has an unexpected FileKind: {0:?}")]
    UnexpectedExternalFileFileKind(FileKind),

    #[error("Not enough information was supplied to identify the requested symbol map. The debug ID is required.")]
    NotEnoughInformationToIdentifySymbolMap,

    #[error("The FileLocation for the debug file does not support loading dyld subcache files.")]
    FileLocationRefusedSubcacheLocation,

    #[error("The FileLocation for the debug file does not support loading external objects.")]
    FileLocationRefusedExternalObjectLocation,

    #[error(
        "The FileLocation for the binary file does not allow following the PDB path found in it."
    )]
    FileLocationRefusedPdbLocation,

    #[error("The FileLocation for the debug file does not support loading source files.")]
    FileLocationRefusedSourceFileLocation,

    #[error(
        "No disambiguator supplied for universal binary, available images: {}", format_multiarch_members(.0)
    )]
    NoDisambiguatorForFatArchive(Vec<FatArchiveMember>),

    #[error("The universal binary (fat archive) was empty")]
    EmptyFatArchive,

    #[error("No match in multi-arch binary, available UUIDs: {}", format_multiarch_members(.0))]
    NoMatchMultiArch(Vec<FatArchiveMember>),

    #[error("Couldn't get symbols from system library, errors: {}", format_errors(.0))]
    NoLuckMacOsSystemLibrary(Vec<Error>),

    #[error("CRC mismatch on file found via GNU debug link, got {0}, expected {1}")]
    DebugLinkCrcMismatch(u32, u32),

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

    #[error("get_candidate_paths_for_debug_file helper callback for {0:?} returned error: {1}")]
    HelperErrorDuringGetCandidatePathsForDebugFile(
        Box<LibraryInfo>,
        #[source] Box<dyn std::error::Error + Send + Sync>,
    ),

    #[error("get_candidate_paths_for_binary helper callback for returned error: {0}")]
    HelperErrorDuringGetCandidatePathsForBinary(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("get_dyld_shared_cache_paths helper callback returned error: {0}")]
    HelperErrorDuringGetDyldSharedCachePaths(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("open_file helper callback for file {0} returned error: {1}")]
    HelperErrorDuringOpenFile(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("FileContents read_bytes_at for file {0} returned error: {1}")]
    HelperErrorDuringFileReading(String, #[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("FileContents read_bytes_at during JITDUMP parsing returned error: {0}")]
    JitDumpFileReading(#[source] std::io::Error),

    #[error("No candidate path for binary, for {0:?} {1:?}")]
    NoCandidatePathForBinary(Option<String>, Option<DebugId>),

    #[error("No candidate path for dyld shared cache")]
    NoCandidatePathForDyldCache,

    #[error("No candidate path for binary, for {0:?}")]
    NoCandidatePathForDebugFile(Box<LibraryInfo>),

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

fn format_errors(errors: &[Error]) -> String {
    errors
        .iter()
        .map(|e| format!("{e}"))
        .collect::<Vec<String>>()
        .join(", ")
}

fn format_multiarch_members(members: &[FatArchiveMember]) -> String {
    members
        .iter()
        .map(|member| {
            let uuid_string = member
                .uuid
                .map(|uuid| DebugId::from_uuid(uuid).breakpad().to_string());
            format!(
                "{} ({} {:08x}/{:08x})",
                uuid_string.as_deref().unwrap_or("<no debug ID>"),
                member.arch.as_deref().unwrap_or("<unrecognized arch>"),
                member.cputype,
                member.cpusubtype
            )
        })
        .collect::<Vec<String>>()
        .join(", ")
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
            Error::NoDisambiguatorForFatArchive(_) => "NoDisambiguatorForFatArchive",
            Error::BreakpadParsing(_) => "BreakpadParsing",
            Error::JitDumpParsing(_) => "JitDumpParsing",
            Error::NotEnoughInformationToIdentifyBinary => "NotEnoughInformationToIdentifyBinary",
            Error::NotEnoughInformationToIdentifySymbolMap => {
                "NotEnoughInformationToIdentifySymbolMap"
            }
            Error::InvalidFileOrInlineOriginIndexInBreakpadFile(_) => {
                "InvalidFileOrInlineOriginIndexInBreakpadFile"
            }
            Error::UnmatchedDebugIdOptional(_, _) => "UnmatchedDebugIdOptional",
            Error::DebugLinkCrcMismatch(_, _) => "DebugLinkCrcMismatch",
            Error::UnmatchedCodeId(_, _) => "UnmatchedCodeId",
            Error::InvalidBreakpadId(_) => "InvalidBreakpadId",
            Error::EmptyFatArchive => "EmptyFatArchive",
            Error::CouldNotDetermineExternalFileFileKind => "CouldNotDetermineExternalFileFileKind",
            Error::FileLocationRefusedSubcacheLocation => "FileLocationRefusedSubcacheLocation",
            Error::FileLocationRefusedExternalObjectLocation => {
                "FileLocationRefusedExternalObjectLocation"
            }
            Error::FileLocationRefusedPdbLocation => "FileLocationRefusedPdbLocation",
            Error::FileLocationRefusedSourceFileLocation => "FileLocationRefusedSourceFileLocation",
            Error::UnexpectedExternalFileFileKind(_) => "UnexpectedExternalFileFileKind",
            Error::NoMatchMultiArch(_) => "NoMatchMultiArch",
            Error::NoLuckMacOsSystemLibrary(_) => "NoLuckMacOsSystemLibrary",
            Error::PdbError(_, _) => "PdbError",
            Error::PdbAddr2lineErrorWithContext(_, _) => "PdbAddr2lineErrorWithContext",
            Error::InvalidInputError(_) => "InvalidInputError",
            Error::DyldCacheParseError(_) => "DyldCacheParseError",
            Error::NoMatchingDyldCacheImagePath(_) => "NoMatchingDyldCacheImagePath",
            Error::ObjectParseError(_, _) => "ObjectParseError",
            Error::MachOHeaderParseError(_) => "MachOHeaderParseError",
            Error::HelperErrorDuringGetCandidatePathsForDebugFile(_, _) => {
                "HelperErrorDuringGetCandidatePathsForDebugFile"
            }
            Error::HelperErrorDuringGetCandidatePathsForBinary(_) => {
                "HelperErrorDuringGetCandidatePathsForBinary"
            }
            Error::HelperErrorDuringGetDyldSharedCachePaths(_) => {
                "HelperErrorDuringGetDyldSharedCachePaths"
            }
            Error::HelperErrorDuringOpenFile(_, _) => "HelperErrorDuringOpenFile",
            Error::HelperErrorDuringFileReading(_, _) => "HelperErrorDuringFileReading",
            Error::JitDumpFileReading(_) => "JitDumpFileReading",
            Error::NoCandidatePathForDebugFile(_) => "NoCandidatePathForDebugFile",
            Error::NoCandidatePathForBinary(_, _) => "NoCandidatePathForBinary",
            Error::NoCandidatePathForDyldCache => "NoCandidatePathForDyldCache",
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
