use crate::pdb_crate::Error as PDBError;
use goblin::error::Error as GoblinError;
use object;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, GetSymbolsError>;

#[derive(Error, Debug)]
pub enum GetSymbolsError {
    #[error("Unmatched breakpad_id: Expected {0}, but received {1}")]
    UnmatchedBreakpadId(String, String),

    #[error("No match in multi-arch binary, errors: {}", .0.iter().map(|e| format!("{}", e)).collect::<Vec<String>>().join(", "))]
    NoMatchMultiArch(Vec<GetSymbolsError>),

    #[error("pdb_crate error: {1} ({0})")]
    PDBError(&'static str, PDBError),

    #[error("Invalid input: {0}")]
    InvalidInputError(&'static str),

    #[error("goblin error: {0}")]
    GoblinError(#[from] GoblinError),

    #[error("MachOHeader parsing error: {0}")]
    MachOHeaderParseError(#[source] object::read::Error),
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

impl GetSymbolsError {
    pub fn enum_as_string(&self) -> &'static str {
        match *self {
            GetSymbolsError::UnmatchedBreakpadId(_, _) => "UnmatchedBreakpadId",
            GetSymbolsError::NoMatchMultiArch(_) => "NoMatchMultiArch",
            GetSymbolsError::PDBError(_, _) => "PDBError",
            GetSymbolsError::InvalidInputError(_) => "InvalidInputError",
            GetSymbolsError::GoblinError(_) => "GoblinError",
            GetSymbolsError::MachOHeaderParseError(_) => "MachOHeaderParseError",
        }
    }
}
