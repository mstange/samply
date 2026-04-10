use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unrecognized URL: {0}")]
    UnrecognizedUrl(String),

    #[error("Couldn't parse request: {0}")]
    ParseRequestErrorSerde(#[from] serde_json::error::Error),

    #[error("Malformed request JSON: {0}")]
    ParseRequestErrorContents(&'static str),

    #[error("{0}")]
    Symbols(
        #[from]
        #[source]
        samply_symbols::Error,
    ),

    #[error("{0}")]
    Source(
        #[from]
        #[source]
        super::source::SourceError,
    ),

    #[error("{0}")]
    Asm(
        #[from]
        #[source]
        super::asm::AsmError,
    ),
}

impl Error {
    /// Returns the HTTP status code that best describes this error.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::ParseRequestErrorSerde(_) => 400,
            Error::ParseRequestErrorContents(_) => 400,
            Error::UnrecognizedUrl(_) => 404,
            Error::Symbols(_) | Error::Source(_) | Error::Asm(_) => 500,
        }
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}
