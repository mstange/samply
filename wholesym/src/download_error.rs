/// The error type used in the observer notification [`SymbolManagerObserver::on_download_failed`].
#[derive(thiserror::Error, Debug)]
pub enum DownloadError {
    /// Creating the reqwest Client failed.
    #[error("Creating the reqwest client failed: {0}")]
    ClientCreationFailed(String),

    /// Opening the request failed.
    #[error("Opening the request failed: {0}")]
    OpenFailed(Box<dyn std::error::Error + Send + Sync>),

    /// The download timed out.
    #[error("The download timed out")]
    Timeout,

    /// The server returned a non-success status code.
    #[error("The server returned status code {0}")]
    StatusError(u16),

    /// The destination directory could not be created.
    #[error("The destination directory could not be created")]
    CouldNotCreateDestinationDirectory,

    /// The response used an unexpected Content-Encoding.
    #[error("The response used an unexpected Content-Encoding: {0}")]
    UnexpectedContentEncoding(String),

    /// An error occurred when reading the download stream.
    #[error("Error when reading the download stream: {0}")]
    StreamRead(std::io::Error),

    /// An I/O error occurred while writing the downloaded file.
    #[error("Error while writing the downloaded file to disk: {0}")]
    DiskWrite(std::io::Error),

    /// Redirect-related error.
    #[error("Redirect-related error")]
    Redirect(Box<dyn std::error::Error + Send + Sync>),

    /// Other error.
    #[error("Other error: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}
