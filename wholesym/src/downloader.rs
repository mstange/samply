use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{AsyncRead, AsyncReadExt as _};
use tokio::io::AsyncWriteExt;

use crate::download::response_to_uncompressed_stream_with_progress;
use crate::file_creation::{create_file_cleanly, CleanFileCreationError};
use crate::DownloadError;

/// A trait for observing the behavior of a `BreakpadSymbolDownloader` or `DebuginfodDownloader`.
/// This can be used for logging, displaying progress bars, expiring cached files, etc.
pub trait DownloaderObserver: Send + Sync + 'static {
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

/// A helper struct with a drop handler. This lets us detect when a download
/// is cancelled by dropping the future.
pub struct DownloadStatusReporter {
    /// Set to `None` when `download_failed()` or `download_completed()` is called.
    download_id: Option<u64>,
    observer: Option<Arc<dyn DownloaderObserver>>,
    ts_before_connect: Instant,
}

impl DownloadStatusReporter {
    pub fn new(observer: Option<Arc<dyn DownloaderObserver>>, url: &str) -> Self {
        let download_id = NEXT_DOWNLOAD_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if let Some(observer) = &observer {
            observer.on_new_download_before_connect(download_id, url);
        }

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

pub trait ChunkConsumer {
    type Output;
    fn consume_chunk(&mut self, chunk_data: &[u8]);
    fn finish(self) -> Self::Output;
}

struct NoopChunkConsumer;

impl ChunkConsumer for NoopChunkConsumer {
    type Output = ();
    fn consume_chunk(&mut self, _chunk_data: &[u8]) {}
    fn finish(self) -> Self::Output {}
}

pub struct Downloader {
    reqwest_client: Result<reqwest::Client, reqwest::Error>,
}

impl Default for Downloader {
    fn default() -> Self {
        Downloader::new()
    }
}

impl Downloader {
    pub fn new() -> Self {
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

        Self { reqwest_client }
    }

    pub async fn initiate_download(
        &self,
        url: &str,
        observer: Option<Arc<dyn DownloaderObserver>>,
    ) -> Result<PendingDownload, DownloadError> {
        let reporter = DownloadStatusReporter::new(observer.clone(), url);

        let reqwest_client = match self.reqwest_client.as_ref() {
            Ok(client) => client,
            Err(e) => {
                reporter.download_failed(DownloadError::ClientCreationFailed(e.to_string()));
                return Err(DownloadError::ClientCreationFailed(e.to_string()));
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
            Err(e) if e.is_status() => {
                let status = e.status().unwrap().as_u16();
                reporter.download_failed(DownloadError::StatusError(status));
                return Err(DownloadError::StatusError(status));
            }
            Err(e) if e.is_request() => {
                let s = e.to_string();
                reporter.download_failed(DownloadError::OpenFailed(e.into()));
                return Err(DownloadError::OpenFailed(s.into()));
            }
            Err(e) if e.is_redirect() => {
                let s = e.to_string();
                reporter.download_failed(DownloadError::Redirect(e.into()));
                return Err(DownloadError::Redirect(s.into()));
            }
            Err(e) if e.is_timeout() => {
                reporter.download_failed(DownloadError::Timeout);
                return Err(DownloadError::Timeout);
            }
            Err(e) => {
                let s = e.to_string();
                reporter.download_failed(DownloadError::Other(e.into()));
                return Err(DownloadError::Other(s.into()));
            }
        };

        let ts_after_status = Instant::now();

        let observer2 = observer.clone();
        let download_id = reporter.download_id();

        let stream = match response_to_uncompressed_stream_with_progress(
            response,
            move |bytes_so_far, total_bytes| {
                if let Some(observer) = observer2.as_deref() {
                    observer.on_download_progress(download_id, bytes_so_far, total_bytes)
                }
            },
        ) {
            Ok(stream) => stream,
            Err(crate::download::Error::UnexpectedContentEncoding(encoding)) => {
                reporter
                    .download_failed(DownloadError::UnexpectedContentEncoding(encoding.clone()));
                return Err(DownloadError::UnexpectedContentEncoding(encoding));
            }
        };
        Ok(PendingDownload {
            reporter,
            stream,
            observer,
            ts_after_status,
        })
    }
}

pub struct PendingDownload {
    reporter: DownloadStatusReporter,
    stream: Pin<Box<dyn AsyncRead + Send + Sync>>,
    observer: Option<Arc<dyn DownloaderObserver>>,
    ts_after_status: Instant,
}

pub enum FileDownloadOutcome<T> {
    DidCreateNewFile(T),
    FoundExistingFile,
}

impl PendingDownload {
    pub async fn download_to_file(
        self,
        dest_path: &Path,
    ) -> Result<FileDownloadOutcome<()>, DownloadError> {
        self.download_to_file_with_chunk_consumer(dest_path, NoopChunkConsumer)
            .await
    }

    pub async fn download_to_file_with_chunk_consumer<C, O>(
        self,
        dest_path: &Path,
        chunk_consumer: C,
    ) -> Result<FileDownloadOutcome<O>, DownloadError>
    where
        C: ChunkConsumer<Output = O> + Send + 'static,
        O: Send + 'static,
    {
        let PendingDownload {
            reporter,
            stream,
            observer,
            ts_after_status,
        } = self;
        let download_id = reporter.download_id();
        if let Some(observer) = observer.as_deref() {
            observer.on_download_started(download_id);
        }

        if let Some(dir) = dest_path.parent() {
            match tokio::fs::create_dir_all(dir).await {
                Ok(_) => {}
                Err(_e) => {
                    reporter.download_failed(DownloadError::CouldNotCreateDestinationDirectory);
                    return Err(DownloadError::CouldNotCreateDestinationDirectory);
                }
            }
        }

        let download_result: Result<
            (FileDownloadOutcome<C::Output>, u64),
            CleanFileCreationError<DownloadError>,
        > = create_file_cleanly(
            dest_path,
            |dest_file: std::fs::File| async move {
                consume_stream_and_write_to_file(stream, chunk_consumer, dest_file).await
            },
            || async {
                let size = std::fs::metadata(dest_path)
                    .map_err(DownloadError::DiskWrite)?
                    .len();
                Ok((FileDownloadOutcome::FoundExistingFile, size))
            },
        )
        .await;

        let (outcome, uncompressed_size_in_bytes) = match download_result {
            Ok(outcome_and_size) => outcome_and_size,
            Err(CleanFileCreationError::CallbackIndicatedError(e)) => {
                let cloned_error = match &e {
                    DownloadError::StreamRead(e) => {
                        DownloadError::StreamRead(std::io::Error::new(e.kind(), e.to_string()))
                    }
                    DownloadError::DiskWrite(e) => {
                        DownloadError::DiskWrite(std::io::Error::new(e.kind(), e.to_string()))
                    }
                    e => DownloadError::Other(e.to_string().into()),
                };
                reporter.download_failed(e);
                return Err(cloned_error);
            }
            Err(e) => {
                let s = e.to_string();
                reporter.download_failed(DownloadError::DiskWrite(e.into()));
                return Err(DownloadError::DiskWrite(std::io::Error::other(s)));
            }
        };

        let ts_after_download = Instant::now();
        reporter.download_completed(
            uncompressed_size_in_bytes,
            ts_after_status,
            ts_after_download,
        );

        if let Some(observer) = &observer {
            observer.on_file_created(dest_path, uncompressed_size_in_bytes);
        }

        Ok(outcome)
    }

    #[allow(clippy::type_complexity)]
    #[allow(dead_code)]
    pub async fn download_to_memory(self) -> Result<Vec<u8>, DownloadError> {
        let PendingDownload {
            reporter,
            mut stream,
            observer,
            ts_after_status,
        } = self;
        let download_id = reporter.download_id();
        if let Some(observer) = observer.as_deref() {
            observer.on_download_started(download_id);
        }

        let mut bytes = Vec::new();
        let bytes_ref = &mut bytes;

        let download_result: Result<u64, std::io::Error> = async move {
            let mut buf = vec![0u8; 2 * 1024 * 1024 /* 2 MiB */];
            let mut uncompressed_size_in_bytes = 0;
            loop {
                let count = stream.read(&mut buf).await?;
                if count == 0 {
                    break;
                }
                uncompressed_size_in_bytes += count as u64;
                bytes_ref.extend_from_slice(&buf[..count]);
            }
            Ok(uncompressed_size_in_bytes)
        }
        .await;

        let uncompressed_size_in_bytes = match download_result {
            Ok(size) => size,
            Err(e) => {
                let kind = e.kind();
                let s = e.to_string();
                reporter.download_failed(DownloadError::StreamRead(e));
                return Err(DownloadError::StreamRead(std::io::Error::new(kind, s)));
            }
        };

        let ts_after_download = Instant::now();
        reporter.download_completed(
            uncompressed_size_in_bytes,
            ts_after_status,
            ts_after_download,
        );

        Ok(bytes)
    }
}

async fn consume_stream_and_write_to_file<C, O>(
    mut stream: Pin<Box<dyn AsyncRead + Send + Sync>>,
    mut chunk_consumer: C,
    dest_file: std::fs::File,
) -> Result<(FileDownloadOutcome<O>, u64), DownloadError>
where
    C: ChunkConsumer<Output = O> + Send + 'static,
    O: Send + 'static,
{
    let mut dest_file = tokio::fs::File::from_std(dest_file);
    let mut buf = vec![0u8; 2 * 1024 * 1024 /* 2 MiB */];
    let mut uncompressed_size_in_bytes = 0;
    loop {
        let count = stream
            .read(&mut buf)
            .await
            .map_err(DownloadError::StreamRead)?;
        if count == 0 {
            break;
        }
        uncompressed_size_in_bytes += count as u64;
        chunk_consumer.consume_chunk(&buf[..count]);
        dest_file
            .write_all(&buf[..count])
            .await
            .map_err(DownloadError::DiskWrite)?;
    }

    let chunk_consumer_output = chunk_consumer.finish();
    dest_file.flush().await.map_err(DownloadError::DiskWrite)?;

    Ok((
        FileDownloadOutcome::DidCreateNewFile(chunk_consumer_output),
        uncompressed_size_in_bytes,
    ))
}
