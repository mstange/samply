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

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}
