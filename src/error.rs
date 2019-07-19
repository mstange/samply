use goblin::error::Error as GoblinError;
use pdb_crate::Error as PDBError;
use serde::Serialize;
use std::fmt::{self};

pub type Result<T> = std::result::Result<T, GetSymbolsError>;

#[derive(Debug)]
pub enum GetSymbolsError {
    UnmatchedBreakpadId(String, String),
    NoMatchMultiArch(Vec<GetSymbolsError>),
    PDBError(PDBError),
    InvalidInputError(&'static str),
    GoblinError(GoblinError),
    MachOHeaderParseError(&'static str),
}

impl From<PDBError> for GetSymbolsError {
    fn from(err: PDBError) -> GetSymbolsError {
        GetSymbolsError::PDBError(err)
    }
}

impl From<GoblinError> for GetSymbolsError {
    fn from(err: GoblinError) -> GetSymbolsError {
        GetSymbolsError::GoblinError(err)
    }
}

impl fmt::Display for GetSymbolsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
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
            GetSymbolsError::PDBError(ref pdb_error) => {
                write!(f, "pdb_crate error: {}", pdb_error.to_string())
            }
            GetSymbolsError::InvalidInputError(ref invalid_input) => {
                write!(f, "Invalid input: {}", invalid_input)
            }
            GetSymbolsError::GoblinError(ref goblin_error) => {
                write!(f, "goblin error: {}", goblin_error.to_string())
            }
            GetSymbolsError::MachOHeaderParseError(ref err_msg) => {
                write!(f, "MachOHeader parsing error: {}", err_msg.to_string())
            }
        }
    }
}

impl GetSymbolsError {
    fn enum_as_string(&self) -> &'static str {
        match *self {
            GetSymbolsError::UnmatchedBreakpadId(_, _) => "UnmatchedBreakpadId",
            GetSymbolsError::NoMatchMultiArch(_) => "NoMatchMultiArch",
            GetSymbolsError::PDBError(_) => "PDBError",
            GetSymbolsError::InvalidInputError(_) => "InvalidInputError",
            GetSymbolsError::GoblinError(_) => "GoblinError",
            GetSymbolsError::MachOHeaderParseError(_) => "MachOHeaderParseError",
        }
    }
}

#[derive(Serialize)]
pub struct GetSymbolsErrorJson {
    error_type: String,
    error_msg: String,
}

impl GetSymbolsErrorJson {
    pub fn from_error(err: GetSymbolsError) -> Self {
        GetSymbolsErrorJson {
            error_type: err.enum_as_string().to_string(),
            error_msg: err.to_string(),
        }
    }
}
