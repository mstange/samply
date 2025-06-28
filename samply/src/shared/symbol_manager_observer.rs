use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use samply_quota_manager::QuotaManagerNotifier;
use wholesym::{DownloadError, SymbolManagerObserver};

pub struct SamplySymbolManagerObserver {
    verbose: bool,
    quota_manager_notifiers: Vec<QuotaManagerNotifier>,
    urls: Mutex<HashMap<u64, String>>,
}

impl SamplySymbolManagerObserver {
    pub fn new(verbose: bool, quota_manager_notifiers: Vec<QuotaManagerNotifier>) -> Self {
        Self {
            verbose,
            quota_manager_notifiers,
            urls: Mutex::new(HashMap::new()),
        }
    }
}

impl SymbolManagerObserver for SamplySymbolManagerObserver {
    fn on_new_download_before_connect(&self, download_id: u64, url: &str) {
        if self.verbose {
            eprintln!("Connecting to {url}...");
        }
        self.urls
            .lock()
            .unwrap()
            .insert(download_id, url.to_owned());
    }

    fn on_download_started(&self, download_id: u64) {
        if self.verbose {
            let urls = self.urls.lock().unwrap();
            let url = urls.get(&download_id).unwrap();
            eprintln!("Downloading from {url}...");
        }
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
        if self.verbose {
            let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
            eprintln!("Finished download from {url}.");
        }
    }

    fn on_download_failed(&self, download_id: u64, reason: DownloadError) {
        if self.verbose {
            let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
            eprintln!("Failed to download from {url}: {reason}.");
        }
    }

    fn on_download_canceled(&self, download_id: u64) {
        if self.verbose {
            let url = self.urls.lock().unwrap().remove(&download_id).unwrap();
            eprintln!("Canceled download from {url}.");
        }
    }

    fn on_file_created(&self, path: &Path, size_in_bytes: u64) {
        if self.verbose {
            eprintln!("Created new file at {path:?} (size: {size_in_bytes} bytes).");
        }
        for notifier in &self.quota_manager_notifiers {
            notifier.on_file_created(path, size_in_bytes, SystemTime::now());
            notifier.trigger_eviction_if_needed();
        }
    }

    fn on_file_accessed(&self, path: &Path) {
        if self.verbose {
            eprintln!("Checking if {path:?} exists... yes");
        }
        for notifier in &self.quota_manager_notifiers {
            notifier.on_file_accessed(path, SystemTime::now());
        }
    }

    fn on_file_missed(&self, path: &Path) {
        if self.verbose {
            eprintln!("Checking if {path:?} exists... no");
        }
    }
}
