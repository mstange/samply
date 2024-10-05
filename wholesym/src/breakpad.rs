use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, Arc},
    time::{Duration, Instant},
};

use futures_util::AsyncReadExt as _;
use samply_symbols::{BreakpadIndex, BreakpadIndexParser, BreakpadParseError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    download::response_to_uncompressed_stream_with_progress,
    file_creation::{create_file_cleanly, CleanFileCreationError},
};

/// The error type used in the observer notification [`BreakpadSymbolObserver::on_download_failed`].
#[derive(thiserror::Error, Debug)]
pub enum DownloadError {
    /// Creating the reqwest Client failed.
    #[error("Creating the client failed: {0}")]
    ClientCreationFailed(String),

    /// Opening the request failed.
    #[error("Opening the request failed: {0}")]
    OpenFailed(Box<dyn std::error::Error + Send + Sync>),

    /// The download timed out.
    #[error("The download timed out")]
    Timeout,

    /// The server returned a non-success status code.
    #[error("The server returned status code {0}")]
    StatusError(http::StatusCode),

    /// The destination directory could not be created.
    #[error("The destination directory could not be created")]
    CouldNotCreateDestinationDirectory,

    /// The response used an unexpected Content-Encoding.
    #[error("The response used an unexpected Content-Encoding: {0}")]
    UnexpectedContentEncoding(String),

    /// An I/O error occurred in the middle of downloading.
    #[error("Error during downloading: {0}")]
    ErrorDuringDownloading(std::io::Error),

    /// Error while writing the downloaded file.
    #[error("Error while writing the downloaded file: {0}")]
    ErrorWhileWritingDownloadedFile(std::io::Error),

    /// Redirect-related error.
    #[error("Redirect-related error")]
    Redirect(Box<dyn std::error::Error + Send + Sync>),

    /// Other error.
    #[error("Other error: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// The error type used in the observer notification [`BreakpadSymbolObserver::on_symindex_generation_failed`].
#[derive(thiserror::Error, Debug)]
pub enum SymindexGenerationError {
    /// No cache directory for breakpad symindex files has been configured.
    #[error("No symindex cache directory")]
    NoSymindexCacheDir,

    /// Could not create destination directory.
    #[error("Could not create destination directory {0}: {1}")]
    CouldNotCreateDestinationDirectory(PathBuf, std::io::Error),

    /// Could not parse breakpad sym file.
    #[error("Could not parse breakpad sym file: {0}")]
    BreakpadParsing(BreakpadParseError),

    /// There was an error while reading the breakpad symbol file.
    #[error("Error while reading the breakpad symbol file: {0}")]
    SymReading(std::io::Error),

    /// There was an error while writing the extracted file.
    #[error("Error while writing the file: {0}")]
    FileWriting(std::io::Error),

    /// Other error.
    #[error("Other error: {0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
#[test]
fn test_download_error_is_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<DownloadError>();
}

impl From<reqwest::Error> for DownloadError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_status() {
            DownloadError::StatusError(e.status().unwrap())
        } else if e.is_request() {
            DownloadError::OpenFailed(e.into())
        } else if e.is_redirect() {
            DownloadError::Redirect(e.into())
        } else if e.is_timeout() {
            DownloadError::Timeout
        } else {
            DownloadError::Other(e.into())
        }
    }
}

/// A trait for observing the behavior of a `BreakpadSymbolDownloader`.
/// This can be used for logging, displaying progress bars, expiring cached files, etc.
pub trait BreakpadSymbolObserver: Send + Sync + 'static {
    /// Called when a new download is about to start, before the connection is established.
    ///
    /// The download ID is unique for each download.
    ///
    /// For each download ID, we guarantee that exactly one of the following methods
    /// will be called at the end of the download: `on_download_completed`,
    /// `on_download_failed`, or `on_download_canceled`.
    fn on_new_download_before_connect(&self, download_id: u64, url: &str);

    /// Called once the connection has been established and HTTP headers
    /// with a success status have arrived.
    fn on_download_started(&self, download_id: u64);

    /// Called frequently during the download, whenever a new chunk has been read.
    ///
    /// If the HTTP response is gzip-compressed, the number of bytes can refer to
    /// either the compressed or the uncompressed bytes - but it'll be consistent:
    /// Either both `bytes_so_far` and `total_bytes` refer to the compressed sizes,
    /// or both refer to the uncompressed sizes.
    ///
    /// If `total_bytes` is `None`, the total size is unknown.
    fn on_download_progress(&self, download_id: u64, bytes_so_far: u64, total_bytes: Option<u64>);

    /// Called when the download has completed successfully.
    ///
    /// Mutually exclusive with `on_download_failed` and `on_download_canceled` for a
    /// given download ID.
    fn on_download_completed(
        &self,
        download_id: u64,
        uncompressed_size_in_bytes: u64,
        time_until_headers: Duration,
        time_until_completed: Duration,
    );

    /// Called when the download has failed.
    ///
    /// This is quite common; the most common reason is [`DownloadError::StatusError`]
    /// with [`StatusCode::NOT_FOUND`](http::StatusCode::NOT_FOUND), for files which
    /// are not available on the server.
    ///
    /// Mutually exclusive with `on_download_completed` and `on_download_canceled` for a
    /// given download ID.
    fn on_download_failed(&self, download_id: u64, reason: DownloadError);

    /// Called when the download has been canceled.
    ///
    /// This does not indicate an error. We commonly attempt to download a file from
    /// multiple sources simultaneously, and cancel other downloads once one has succeeded.
    ///
    /// This function is also called if the user cancels the download by dropping the future
    /// returned from [`BreakpadSymbolDownloader::get_file`].
    ///
    /// Mutually exclusive with `on_download_completed` and `on_download_failed` for a
    /// given download ID.
    fn on_download_canceled(&self, download_id: u64);

    /// Called when a file has been created, for example because it was downloaded from
    /// a server, copied from a different cache directory, or extracted from a compressed
    /// file.
    fn on_file_created(&self, path: &Path, size_in_bytes: u64);

    /// Called when a file from the cache has been used to service a [`BreakpadSymbolDownloader::get_file`] call.
    ///
    /// This is only called for pre-existing files and not for newly-created files - newly-created
    /// files only trigger a call to `on_file_created`.
    ///
    /// Useful to guide expiration decisions.
    fn on_file_accessed(&self, path: &Path);

    /// Called when we were looking for a file in the cache, and it wasn't there. Used for
    /// debug logging.
    ///
    /// Also called if checking for file existence fails for any other reason.
    fn on_file_missed(&self, path: &Path);
}

static NEXT_DOWNLOAD_ID: AtomicU64 = AtomicU64::new(0);

pub struct BreakpadSymbolDownloader {
    inner: Arc<BreakpadSymbolDownloaderInner>,
}

impl BreakpadSymbolDownloader {
    pub fn new(
        breakpad_directories_readonly: Vec<PathBuf>,
        breakpad_servers: Vec<(String, PathBuf)>,
        breakpad_symindex_cache_dir: Option<PathBuf>,
    ) -> Self {
        let builder = reqwest::Client::builder();

        // Turn off HTTP 2, in order to work around https://github.com/seanmonstar/reqwest/issues/1761 .
        let builder = builder.http1_only();

        // Turn off automatic decompression because it doesn't allow us to compute
        // download progress percentages: we'd only know the decompressed current
        // size and the compressed total size.
        // Instead, we do the streaming decompression manually, see download.rs.
        let builder = builder.no_gzip().no_brotli().no_deflate();

        // Create the client.
        // TODO: Add timeouts, user agent, maybe other settings
        let reqwest_client = builder.build();

        let inner = BreakpadSymbolDownloaderInner {
            breakpad_directories_readonly,
            breakpad_servers,
            breakpad_symindex_cache_dir,
            observer: None,
            reqwest_client,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Set the observer for this downloader.
    ///
    /// The observer can be used for logging, displaying progress bars, informing
    /// automatic expiration of cached files, and so on.
    ///
    /// See the [`BreakpadSymbolObserver`] trait for more information.
    pub fn set_observer(&mut self, observer: Option<Arc<dyn BreakpadSymbolObserver>>) {
        Arc::get_mut(&mut self.inner).unwrap().observer = observer;
    }

    pub async fn get_file(&self, rel_path: &str) -> Option<PathBuf> {
        self.inner.get_file(rel_path).await
    }

    pub async fn get_file_no_download(&self, rel_path: &str) -> Option<PathBuf> {
        self.inner.get_file_no_download(rel_path).await
    }

    /// If we have a configured symindex cache directory, and there is a .sym file at
    /// `local_path` for which we don't have a .symindex file, create the .symindex file.
    pub async fn ensure_symindex(
        &self,
        sym_path: &Path,
        rel_path: &str,
    ) -> Result<PathBuf, SymindexGenerationError> {
        self.inner.ensure_symindex(sym_path, rel_path).await
    }

    #[allow(dead_code)]
    pub fn symindex_path(&self, rel_path: &str) -> Option<PathBuf> {
        self.inner.symindex_path(rel_path)
    }
}

struct BreakpadSymbolDownloaderInner {
    breakpad_directories_readonly: Vec<PathBuf>,
    breakpad_servers: Vec<(String, PathBuf)>,
    breakpad_symindex_cache_dir: Option<PathBuf>,
    observer: Option<Arc<dyn BreakpadSymbolObserver>>,
    reqwest_client: Result<reqwest::Client, reqwest::Error>,
}

impl BreakpadSymbolDownloaderInner {
    pub async fn get_file(&self, rel_path: &str) -> Option<PathBuf> {
        for dir in &self.breakpad_directories_readonly {
            let path = dir.join(rel_path);
            if self.check_file_exists(&path).await {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&path);
                }
                return Some(path);
            }
        }

        for (server_base_url, cache_dir) in &self.breakpad_servers {
            let path = cache_dir.join(rel_path);
            if self.check_file_exists(&path).await {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&path);
                }
                return Some(path);
            }
            if let Some(path) = self
                .get_bp_sym_file_from_server(rel_path, server_base_url, cache_dir)
                .await
            {
                return Some(path);
            }
        }
        None
    }

    /// Return whether a file is found at `path`, and notify the observer if not.
    async fn check_file_exists(&self, path: &Path) -> bool {
        let file_exists = matches!(tokio::fs::metadata(path).await, Ok(meta) if meta.is_file());
        if !file_exists {
            if let Some(observer) = self.observer.as_deref() {
                observer.on_file_missed(path);
            }
        }
        file_exists
    }

    pub async fn get_file_no_download(&self, rel_path: &str) -> Option<PathBuf> {
        let dirs: Vec<_> = self
            .breakpad_directories_readonly
            .iter()
            .chain(self.breakpad_servers.iter().map(|(_url, dir)| dir))
            .collect();
        for dir in dirs {
            let path = dir.join(rel_path);
            if self.check_file_exists(&path).await {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&path);
                }
                return Some(path);
            }
        }

        None
    }

    async fn prepare_download_of_file(
        &self,
        url: &str,
    ) -> Option<(DownloadStatusReporter, reqwest::Response)> {
        let download_id = NEXT_DOWNLOAD_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(observer) = self.observer.as_deref() {
            observer.on_new_download_before_connect(download_id, url);
        }

        let reporter = DownloadStatusReporter::new(download_id, self.observer.clone());

        let reqwest_client = match self.reqwest_client.as_ref() {
            Ok(client) => client,
            Err(e) => {
                reporter.download_failed(DownloadError::ClientCreationFailed(e.to_string()));
                return None;
            }
        };

        let request_builder = reqwest_client.get(url);

        // Manually specify the Accept-Encoding header.
        // This would happen automatically if we hadn't turned off automatic
        // decompression for this reqwest client.
        let request_builder = request_builder.header("Accept-Encoding", "gzip");

        // Send the request and wait for the headers.
        let response_result = request_builder.send().await;

        // Check the HTTP status code.
        let response_result = response_result.and_then(|response| response.error_for_status());

        let response = match response_result {
            Ok(response) => response,
            Err(e) => {
                // The request failed, most commonly due to a 404 status code.
                reporter.download_failed(DownloadError::from(e));
                return None;
            }
        };

        Some((reporter, response))
    }

    /// Given a relative file path and a cache directory path, concatenate the two to make
    /// a destination path, and create the necessary directories so that a file can be stored
    /// at the destination path.
    async fn make_dest_path_and_ensure_parent_dirs(
        &self,
        rel_path: &str,
        cache_path: &Path,
    ) -> Result<PathBuf, std::io::Error> {
        let dest_path = cache_path.join(rel_path);
        if let Some(dir) = dest_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(dest_path)
    }

    async fn get_bp_sym_file_from_server(
        &self,
        rel_path: &str,
        server_base_url: &str,
        cache_dir: &Path,
    ) -> Option<PathBuf> {
        let server_base_url = server_base_url.trim_end_matches('/');
        let url = format!("{server_base_url}/{rel_path}");
        let (reporter, response) = self.prepare_download_of_file(&url).await?;

        let ts_after_status = Instant::now();
        let download_id = reporter.download_id();
        if let Some(observer) = self.observer.as_deref() {
            observer.on_download_started(download_id);
        }

        let dest_path = match self
            .make_dest_path_and_ensure_parent_dirs(rel_path, cache_dir)
            .await
        {
            Ok(dest_path) => dest_path,
            Err(_e) => {
                reporter.download_failed(DownloadError::CouldNotCreateDestinationDirectory);
                return None;
            }
        };

        let observer = self.observer.clone();
        let mut stream = match response_to_uncompressed_stream_with_progress(
            response,
            move |bytes_so_far, total_bytes| {
                if let Some(observer) = observer.as_deref() {
                    observer.on_download_progress(download_id, bytes_so_far, total_bytes)
                }
            },
        ) {
            Ok(stream) => stream,
            Err(crate::download::Error::UnexpectedContentEncoding(encoding)) => {
                reporter.download_failed(DownloadError::UnexpectedContentEncoding(encoding));
                return None;
            }
        };

        let download_result: Result<
            (Option<BreakpadIndexParser>, u64),
            CleanFileCreationError<std::io::Error>,
        > = create_file_cleanly(
            &dest_path,
            |dest_file: std::fs::File| async move {
                let mut dest_file = tokio::fs::File::from_std(dest_file);
                let mut buf = vec![0u8; 4096];
                let mut uncompressed_size_in_bytes = 0;
                let mut index_generator = BreakpadIndexParser::new();
                loop {
                    let count = stream.read(&mut buf).await?;
                    if count == 0 {
                        break;
                    }
                    uncompressed_size_in_bytes += count as u64;
                    dest_file.write_all(&buf[..count]).await?;
                    index_generator.consume(&buf[..count]);
                }
                dest_file.flush().await?;
                Ok((Some(index_generator), uncompressed_size_in_bytes))
            },
            || async {
                let size = std::fs::metadata(&dest_path)?.len();
                Ok((None, size))
            },
        )
        .await;

        let (index_generator, uncompressed_size_in_bytes) = match download_result {
            Ok((index_generator, size)) => (index_generator, size),
            Err(CleanFileCreationError::CallbackIndicatedError(e)) => {
                reporter.download_failed(DownloadError::ErrorDuringDownloading(e));
                return None;
            }
            Err(e) => {
                reporter.download_failed(DownloadError::ErrorWhileWritingDownloadedFile(e.into()));
                return None;
            }
        };

        let ts_after_download = Instant::now();
        reporter.download_completed(
            uncompressed_size_in_bytes,
            ts_after_status,
            ts_after_download,
        );

        if let Some(observer) = self.observer.as_deref() {
            observer.on_file_created(&dest_path, uncompressed_size_in_bytes);
        }

        match index_generator {
            Some(index_generator) => {
                if let Ok(index) = index_generator.finish() {
                    if let Some(symindex_path) = self.symindex_path(rel_path) {
                        let _ = self.write_symindex(&symindex_path, index).await;
                    }
                }
            }
            None => {
                let _ = self.ensure_symindex(&dest_path, rel_path).await;
            }
        }

        Some(dest_path)
    }

    pub fn symindex_path(&self, rel_path: &str) -> Option<PathBuf> {
        let symindex_dir = self.breakpad_symindex_cache_dir.as_deref()?;
        Some(symindex_dir.join(rel_path).with_extension("symindex"))
    }

    async fn write_symindex(
        &self,
        symindex_path: &Path,
        index: BreakpadIndex,
    ) -> Result<(), SymindexGenerationError> {
        if let Some(parent_dir) = symindex_path.parent() {
            tokio::fs::create_dir_all(parent_dir).await.map_err(|e| {
                SymindexGenerationError::CouldNotCreateDestinationDirectory(
                    parent_dir.to_owned(),
                    e,
                )
            })?;
        }
        let index_size_result: Result<u64, CleanFileCreationError<SymindexGenerationError>> =
            create_file_cleanly(
                symindex_path,
                |index_file| async move {
                    let mut index_file = tokio::fs::File::from_std(index_file);
                    let bytes = index.serialize_to_bytes();
                    index_file
                        .write_all(&bytes)
                        .await
                        .map_err(SymindexGenerationError::FileWriting)?;
                    index_file
                        .flush()
                        .await
                        .map_err(SymindexGenerationError::FileWriting)?;
                    Ok(bytes.len() as u64)
                },
                || async {
                    let size = std::fs::metadata(symindex_path)
                        .map_err(|_| {
                            SymindexGenerationError::Other(
                                "Could not get size of existing extracted file".into(),
                            )
                        })?
                        .len();
                    Ok(size)
                },
            )
            .await;

        match index_size_result {
            Ok(size_in_bytes) => {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_created(symindex_path, size_in_bytes);
                }
            }
            Err(CleanFileCreationError::CallbackIndicatedError(e)) => return Err(e),
            Err(e) => return Err(SymindexGenerationError::FileWriting(e.into())),
        }

        Ok(())
    }

    /// If we have a configured symindex cache directory, and there is a .sym file at
    /// `local_path` for which we don't have a .symindex file, create the .symindex file.
    pub async fn ensure_symindex(
        &self,
        sym_path: &Path,
        rel_path: &str,
    ) -> Result<PathBuf, SymindexGenerationError> {
        let Some(symindex_path) = self.symindex_path(rel_path) else {
            return Err(SymindexGenerationError::NoSymindexCacheDir);
        };

        if self.check_file_exists(&symindex_path).await {
            if let Some(observer) = self.observer.as_deref() {
                observer.on_file_accessed(&symindex_path);
            }
            return Ok(symindex_path);
        }

        let index = self.parse_sym_file_into_index(sym_path).await?;
        self.write_symindex(&symindex_path, index).await?;
        Ok(symindex_path)
    }

    async fn parse_sym_file_into_index(
        &self,
        sym_path: &Path,
    ) -> Result<BreakpadIndex, SymindexGenerationError> {
        let mut sym_file = tokio::fs::File::open(sym_path)
            .await
            .map_err(SymindexGenerationError::SymReading)?;
        let mut parser = BreakpadIndexParser::new();
        const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MiB
        let mut buffer = vec![0; CHUNK_SIZE];
        loop {
            let read_len = sym_file
                .read(&mut buffer)
                .await
                .map_err(SymindexGenerationError::SymReading)?;
            if read_len == 0 {
                break;
            }
            parser.consume(&buffer[..read_len]);
        }
        parser
            .finish()
            .map_err(SymindexGenerationError::BreakpadParsing)
    }
}

/// A helper struct with a drop handler. This lets us detect when a download
/// is cancelled by dropping the future.
struct DownloadStatusReporter {
    /// Set to `None` when `download_failed()` or `download_completed()` is called.
    download_id: Option<u64>,
    observer: Option<Arc<dyn BreakpadSymbolObserver>>,
    ts_before_connect: Instant,
}

impl DownloadStatusReporter {
    pub fn new(download_id: u64, observer: Option<Arc<dyn BreakpadSymbolObserver>>) -> Self {
        Self {
            download_id: Some(download_id),
            observer,
            ts_before_connect: Instant::now(),
        }
    }

    pub fn download_id(&self) -> u64 {
        self.download_id.unwrap()
    }

    pub fn download_failed(mut self, e: DownloadError) {
        if let (Some(download_id), Some(observer)) = (self.download_id, self.observer.as_deref()) {
            observer.on_download_failed(download_id, e);
        }
        self.download_id = None;
        // Drop self. Now the Drop handler won't do anything.
    }

    pub fn download_completed(
        mut self,
        uncompressed_size_in_bytes: u64,
        ts_after_headers: Instant,
        ts_after_completed: Instant,
    ) {
        if let (Some(download_id), Some(observer)) = (self.download_id, self.observer.as_deref()) {
            let time_until_headers = ts_after_headers.duration_since(self.ts_before_connect);
            let time_until_completed = ts_after_completed.duration_since(self.ts_before_connect);
            observer.on_download_completed(
                download_id,
                uncompressed_size_in_bytes,
                time_until_headers,
                time_until_completed,
            );
        }
        self.download_id = None;
        // Drop self. Now the Drop handler won't do anything.
    }
}

impl Drop for DownloadStatusReporter {
    fn drop(&mut self) {
        if let (Some(download_id), Some(observer)) = (self.download_id, self.observer.as_deref()) {
            // We were dropped before a call to `download_failed` or `download_completed`.
            // This was most likely because the future we were stored in was dropped.
            // Tell the observer.
            observer.on_download_canceled(download_id);
        }
    }
}
