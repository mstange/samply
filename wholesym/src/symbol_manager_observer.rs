use std::path::Path;
use std::time::Duration;

use crate::download_error::DownloadError;

/// A trait for observing the behavior of a [`SymbolManager`](crate::SymbolManager).
/// This can be used for logging, displaying progress bars, expiring cached files, etc.
pub trait SymbolManagerObserver: Send + Sync + 'static {
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
    /// This function is also called if the user cancels the download by dropping a future.
    ///
    /// Mutually exclusive with `on_download_completed` and `on_download_failed` for a
    /// given download ID.
    fn on_download_canceled(&self, download_id: u64);

    /// Called when a file has been created, for example because it was downloaded from
    /// a server, copied from a different cache directory, or extracted from a compressed
    /// file.
    fn on_file_created(&self, path: &Path, size_in_bytes: u64);

    /// Called when a file from the cache has been used when obtaining a symbol map.
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
