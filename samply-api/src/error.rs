use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Symbols(
        #[from]
        #[source]
        samply_symbols::Error,
    ),

    #[error("Couldn't parse request: {0}")]
    ParseRequestErrorSerde(#[from] serde_json::error::Error),

    #[error("Malformed request JSON: {0}")]
    ParseRequestErrorContents(&'static str),
}
