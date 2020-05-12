use crate::pdb_crate::Error as PDBError;
use goblin::error::Error as GoblinError;
use object;
use std::fmt::{self};

pub type Result<T> = std::result::Result<T, GetSymbolsError>;

#[derive(Debug)]
pub enum GetSymbolsError {
    UnmatchedBreakpadId(String, String),
    NoMatchMultiArch(Vec<GetSymbolsError>),
    PDBError(&'static str, PDBError),
    InvalidInputError(&'static str),
    GoblinError(GoblinError),
    MachOHeaderParseError(object::read::Error),
}

impl From<PDBError> for GetSymbolsError {
    fn from(err: PDBError) -> GetSymbolsError {
        GetSymbolsError::PDBError("Unknown", err)
    }
}

impl From<GoblinError> for GetSymbolsError {
    fn from(err: GoblinError) -> GetSymbolsError {
        GetSymbolsError::GoblinError(err)
    }
}

impl fmt::Display for GetSymbolsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            GetSymbolsError::UnmatchedBreakpadId(ref expected, ref actual) => write!(
                f,
                "Unmatched breakpad_id: Expected {}, but received {}",
                expected, actual
            ),
            GetSymbolsError::NoMatchMultiArch(ref errors) => {
                let error_strings: Vec<String> = errors.iter().map(|e| format!("{}", e)).collect();
                write!(
                    f,
                    "No match in multi-arch binary, errors: {}",
                    error_strings.join(", ")
                )
            }
            GetSymbolsError::PDBError(invocation_description, ref pdb_error) => write!(
                f,
                "pdb_crate error: {} ({})",
                pdb_error.to_string(),
                invocation_description
            ),
            GetSymbolsError::InvalidInputError(ref invalid_input) => {
                write!(f, "Invalid input: {}", invalid_input)
            }
            GetSymbolsError::GoblinError(ref goblin_error) => {
                write!(f, "goblin error: {}", goblin_error.to_string())
            }
            GetSymbolsError::MachOHeaderParseError(object_error) => {
                write!(f, "MachOHeader parsing error: {}", object_error.to_string())
            }
        }
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
