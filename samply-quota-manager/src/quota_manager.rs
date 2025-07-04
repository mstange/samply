use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, SystemTime};

use bytesize::ByteSize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::file_inventory::{FileInfo, FileInventory};

/// Evicts least-recently-used files in a managed directory when asked to do so.
///
/// `QuotaManager` does not observe file system mutations! You have to tell it
/// about any files you create or delete in this directory. It stores this
/// information in a sqlite database file at a path of your choosing.
pub struct QuotaManager {
    settings: Arc<Mutex<EvictionSettings>>,
    inventory: Arc<Mutex<FileInventory>>,
    /// Sent from QuotaManager::finish
    stop_signal_sender: tokio::sync::oneshot::Sender<()>,
    /// Sent whenever a file is added. A `Notify` has the semantics
    /// we want if the sender side notifies more frequently than the
    /// receiver side can check; multiple notifications are coalesced
    /// into one.
    eviction_signal_sender: Arc<tokio::sync::Notify>,
    /// The join handle for the tokio task that receives the signals
    /// and deletes the files. Stored here so that finish() can block on it.
    join_handle: JoinHandle<()>,
}

/// Used to initiate a [`QuotaManager`] eviction, and to tell it about
/// the creation and access of files. `Send` and `Sync`.
pub struct QuotaManagerNotifier {
    inventory: Arc<Mutex<FileInventory>>,
    eviction_signal_sender: Arc<tokio::sync::Notify>,
}

impl Clone for QuotaManagerNotifier {
    fn clone(&self) -> Self {
        Self {
            inventory: self.inventory.clone(),
            eviction_signal_sender: self.eviction_signal_sender.clone(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EvictionSettings {
    max_size_bytes: Option<u64>,
    max_age_seconds: Option<u64>,
}

impl QuotaManager {
    /// Create an instance for the managed directory given by `root_path`.
    ///
    /// Uses the database at `db_path` to store information about the files
    /// under this directory, and creates it if it doesn't exist yet,
    /// prepopulating it with information about the current files in the
    /// managed directory.
    ///
    /// Both root_path and the parent directory of db_path must already exist.
    pub fn new(root_path: &Path, db_path: &Path) -> Result<Self, String> {
        let root_path = root_path.to_path_buf();
        let root_path_clone = root_path.clone();
        let inventory = FileInventory::new(&root_path, db_path, move || {
            Self::list_existing_files_sync(&root_path_clone)
        })
        .map_err(|e| format!("{e}"))?;
        let inventory = Arc::new(Mutex::new(inventory));
        let settings = Arc::new(Mutex::new(EvictionSettings::default()));

        let (stop_signal_sender, stop_signal_receiver) = tokio::sync::oneshot::channel();
        let eviction_signal_sender = Arc::new(Notify::new());

        let eviction_thread_runner = QuotaManagerEvictionThread {
            inventory: Arc::clone(&inventory),
            settings: Arc::clone(&settings),
        };

        let join_handle = tokio::spawn(
            eviction_thread_runner.run(stop_signal_receiver, eviction_signal_sender.clone()),
        );

        Ok(Self {
            settings,
            inventory,
            stop_signal_sender,
            eviction_signal_sender,
            join_handle,
        })
    }

    /// Returns a new `QuotaManagerNotifier`. This is how you tell this
    /// `QuotaManager` about file creations / accesses
    pub fn notifier(&self) -> QuotaManagerNotifier {
        QuotaManagerNotifier {
            inventory: Arc::clone(&self.inventory),
            eviction_signal_sender: Arc::clone(&self.eviction_signal_sender),
        }
    }

    /// Stops the background task that does the evictions, and waits for
    /// any currently-running eviction to finish.
    pub async fn finish(self) {
        let _ = self.stop_signal_sender.send(());
        self.join_handle.await.unwrap()
    }

    /// Change the desired maximum total size of the managed directory, in bytes.
    ///
    /// Respected during the next eviction.
    pub fn set_max_total_size(&self, max_size_bytes: Option<u64>) {
        self.settings.lock().unwrap().max_size_bytes = max_size_bytes;
    }

    /// Change the desired maximum age of tracked files in the managed directory,
    /// in seconds.
    ///
    /// Respected during the next eviction.
    pub fn set_max_age(&self, max_age_seconds: Option<u64>) {
        self.settings.lock().unwrap().max_age_seconds = max_age_seconds;
    }

    /// Returns the current total size of the managed directory, in bytes.
    ///
    /// The return value is only correct if the information in the database
    /// matches reality.
    ///
    /// No file accesses are performed to compute the total size; the information
    /// comes purely from the information passed to the [`QuotaManagerNotifier`]
    /// methods, .
    pub fn current_total_size(&self) -> u64 {
        self.inventory.lock().unwrap().total_size_in_bytes()
    }

    fn list_existing_files_sync(dir: &Path) -> Vec<FileInfo> {
        let mut files = Vec::new();
        let mut dirs_to_visit = VecDeque::new();
        dirs_to_visit.push_back(dir.to_path_buf());

        while let Some(current_dir) = dirs_to_visit.pop_front() {
            let entries = match fs::read_dir(&current_dir) {
                Ok(entries) => entries,
                Err(e) => {
                    log::error!("Failed to read directory {}: {}", current_dir.display(), e);
                    continue;
                }
            };
            for entry in entries {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(e) => {
                        log::error!("Failed to read directory entry in {current_dir:?}: {e}");
                        continue;
                    }
                };

                if path.is_dir() {
                    dirs_to_visit.push_back(path);
                    continue;
                }
                if !path.is_file() {
                    continue;
                }

                let metadata = match fs::metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(e) => {
                        log::error!("Failed to query file size for {path:?}: {e}");
                        continue;
                    }
                };
                files.push(FileInfo {
                    path,
                    size_in_bytes: metadata.len(),
                    creation_time: metadata.created().ok().unwrap_or_else(SystemTime::now),
                    last_access_time: metadata.accessed().ok().unwrap_or_else(SystemTime::now),
                });
            }
        }
        log::info!("Found {} existing files in {:?}", files.len(), dir);
        files
    }
}

struct QuotaManagerEvictionThread {
    settings: Arc<Mutex<EvictionSettings>>,
    inventory: Arc<Mutex<FileInventory>>,
}

impl QuotaManagerEvictionThread {
    pub async fn run(
        self,
        stop_signal_receiver: tokio::sync::oneshot::Receiver<()>,
        eviction_signal_receiver: Arc<tokio::sync::Notify>,
    ) {
        tokio::pin!(stop_signal_receiver);
        loop {
            // Wait until we've received the signal to do an eviction, or until
            // we've received the signal to stop. Check the eviction signal first,
            // because in the case that both are received at the same time, we want
            // to do the eviction before stopping.
            // This ordering is really only important in tests, where the same thread
            // sends both the eviction signal and the stop signal, in that order.
            // In cases where the signals are sent by different threads, the order
            // is arbitrary and it's fine to ignore an eviction signal that was sent
            // "after" the stop signal.
            use std::future::Future;
            let eviction_notified = eviction_signal_receiver.notified();
            tokio::pin!(eviction_notified);
            let (should_evict, should_stop) = futures::future::poll_fn(|cx| {
                let eviction_signal_status = eviction_notified.as_mut().poll(cx);
                let stop_signal_status = stop_signal_receiver.as_mut().poll(cx);
                match (eviction_signal_status, stop_signal_status) {
                    (Poll::Ready(()), Poll::Ready(_)) => Poll::Ready((true, true)),
                    (Poll::Ready(()), Poll::Pending) => Poll::Ready((true, false)),
                    (Poll::Pending, Poll::Ready(_)) => Poll::Ready((false, true)),
                    (Poll::Pending, Poll::Pending) => Poll::Pending,
                }
            })
            .await;

            if should_evict {
                self.perform_eviction_if_needed().await;
            }
            if should_stop {
                return;
            }
        }
    }

    async fn perform_eviction_if_needed(&self) {
        let settings = *self.settings.lock().unwrap();
        let (total_size_before, root_path) = {
            let inventory = self.inventory.lock().unwrap();
            (
                inventory.total_size_in_bytes(),
                inventory.root_path().to_owned(),
            )
        };
        log::info!(
            "Current total size: {}",
            ByteSize(total_size_before).display().si()
        );

        // Enforce max age first, and size limit second.
        // We know that files older than the max age need to be deleted anyway.
        // This may already free up some space. Then we can delete more files in
        // order to enforce the size limit.

        let files_to_delete_for_enforcing_max_age = match settings.max_age_seconds {
            Some(max_age_seconds) => {
                let cutoff_time = SystemTime::now() - Duration::from_secs(max_age_seconds);
                let inventory = self.inventory.lock().unwrap();
                inventory.get_files_last_accessed_before(cutoff_time)
            }
            None => vec![],
        };

        if !files_to_delete_for_enforcing_max_age.is_empty() {
            self.delete_files(files_to_delete_for_enforcing_max_age, &root_path)
                .await;
            let total_size = self.inventory.lock().unwrap().total_size_in_bytes();
            log::info!(
                "Current total size: {}",
                ByteSize(total_size).display().si()
            );
        }

        let files_to_delete_for_enforcing_max_size = match settings.max_size_bytes {
            Some(max_size_bytes) => {
                let inventory = self.inventory.lock().unwrap();
                inventory.get_files_to_delete_to_enforce_max_size(max_size_bytes)
            }
            None => vec![],
        };

        if !files_to_delete_for_enforcing_max_size.is_empty() {
            self.delete_files(files_to_delete_for_enforcing_max_size, &root_path)
                .await;
            let total_size = self.inventory.lock().unwrap().total_size_in_bytes();
            log::info!(
                "Current total size: {}",
                ByteSize(total_size).display().si()
            );
        }
    }

    async fn delete_files(&self, files: Vec<FileInfo>, root_path: &Path) {
        for file_info in files {
            log::info!(
                "Deleting file {} ({})",
                file_info.path.display(),
                ByteSize(file_info.size_in_bytes).display().si()
            );
            match tokio::fs::remove_file(&file_info.path).await {
                Ok(()) => {
                    let mut inventory = self.inventory.lock().unwrap();
                    inventory.on_file_deleted(&file_info.path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let mut inventory = self.inventory.lock().unwrap();
                    inventory.on_file_found_to_be_absent(&file_info.path);
                }
                Err(e) => {
                    log::error!("Error when deleting {}: {}", file_info.path.display(), e);
                }
            }

            // Clean up any empty parent directories.
            let mut path = file_info.path.as_path();
            while let Some(parent_dir) = path.parent() {
                if !parent_dir.starts_with(root_path) || parent_dir == root_path {
                    break;
                }
                match tokio::fs::remove_dir(parent_dir).await {
                    Ok(()) => {
                        log::info!("Deleted empty directory {}", parent_dir.display());
                        path = parent_dir;
                    }
                    Err(_) => break, // Directory not empty, or some other error
                }
            }
        }
    }
}

impl QuotaManagerNotifier {
    /// Trigger an eviction. The eviction runs asynchronously in a single
    /// shared eviction task and uses the current eviction settings.
    pub fn trigger_eviction_if_needed(&self) {
        self.eviction_signal_sender.notify_one();
    }

    /// You must call this whenever a new file gets added to the managed directory.
    ///
    /// Calls for files outside the managed directory are ignored.
    pub fn on_file_created(&self, path: &Path, size_in_bytes: u64, creation_time: SystemTime) {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.on_file_created(path, size_in_bytes, creation_time);
    }

    /// You must call this whenever a file in the managed directory is accessed.
    ///
    /// Calls for files outside the managed directory are ignored.
    pub fn on_file_accessed(&self, path: &Path, access_time: SystemTime) {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.on_file_accessed(path, access_time);
    }

    /// You usually don't need to call this because we expect you to leave any
    /// deleting to the [`QuotaManager`]. But if you do delete any files in the
    /// managed directory yourself, call this method so that the [`QuotaManager`]
    /// can update its information.
    pub fn on_file_deleted(&self, path: &Path) {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.on_file_deleted(path);
    }
}
