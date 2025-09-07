use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::{DownloadError, SymbolManagerObserver};

pub struct VerboseSymbolManagerObserver {
    urls: Mutex<HashMap<u64, String>>,
}

impl VerboseSymbolManagerObserver {
    pub fn new() -> Self {
        Self {
            urls: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for VerboseSymbolManagerObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolManagerObserver for VerboseSymbolManagerObserver {
    fn on_new_download_before_connect(&self, download_id: u64, url: &str) {
        eprintln!("Connecting to {url}...");
        self.urls
            .lock()
            .unwrap()
            .insert(download_id, url.to_owned());
    }

    fn on_download_started(&self, download_id: u64) {
        let url = self.urls.lock().unwrap().get(&download_id).unwrap().clone();
        eprintln!("Downloading from {url}...");
    }

    fn on_download_progress(
        &self,
        _download_id: u64,
        _bytes_so_far: u64,
        _total_bytes: Option<u64>,
    ) {
    }

    fn on_download_completed(
        &self,
        download_id: u64,
        _uncompressed_size_in_bytes: u64,
        _time_until_headers: std::time::Duration,
        _time_until_completed: std::time::Duration,
    ) {
        let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
        eprintln!("Finished download from {url}.");
    }

    fn on_download_failed(&self, download_id: u64, reason: DownloadError) {
        let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
        eprintln!("Failed to download from {url}: {reason}.");
    }

    fn on_download_canceled(&self, download_id: u64) {
        let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
        eprintln!("Canceled download from {url}.");
    }

    fn on_file_created(&self, path: &Path, size_in_bytes: u64) {
        eprintln!("Created new file at {path:?} (size: {size_in_bytes} bytes).");
    }

    fn on_file_accessed(&self, path: &Path) {
        eprintln!("Checking if {path:?} exists... yes");
    }

    fn on_file_missed(&self, path: &Path) {
        eprintln!("Checking if {path:?} exists... no");
    }
}
