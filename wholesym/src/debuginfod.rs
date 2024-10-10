use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::downloader::{Downloader, DownloaderObserver};

pub struct DebuginfodDownloader(DebuginfodDownloaderInner);

enum DebuginfodDownloaderInner {
    #[allow(unused)]
    Official(OfficialDebuginfodDownloader),
    Manual(ManualDebuginfodDownloader),
}

impl DebuginfodDownloader {
    pub fn new(
        debuginfod_cache_dir_if_not_installed: Option<PathBuf>,
        mut servers_and_caches: Vec<(String, PathBuf)>,
        downloader: Option<Arc<Downloader>>,
    ) -> Self {
        let is_debuginfod_installed = false;
        if is_debuginfod_installed {
            todo!()
        } else {
            if let (Some(cache_dir), Ok(urls)) = (
                debuginfod_cache_dir_if_not_installed,
                std::env::var("DEBUGINFOD_URLS"),
            ) {
                let mut servers_from_env = Vec::new();
                for url in urls.split_ascii_whitespace() {
                    servers_from_env.push((url.to_string(), cache_dir.clone()));
                }
                let extra_servers = std::mem::replace(&mut servers_and_caches, servers_from_env);
                servers_and_caches.extend(extra_servers);
            }

            Self(DebuginfodDownloaderInner::Manual(
                ManualDebuginfodDownloader {
                    servers_and_caches,
                    observer: None,
                    downloader: downloader.unwrap_or_default(),
                },
            ))
        }
    }

    #[allow(unused)]
    pub async fn get_file_only_cached(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        match &self.0 {
            DebuginfodDownloaderInner::Official(official) => {
                official.get_file_only_cached(buildid, file_type).await
            }
            DebuginfodDownloaderInner::Manual(manual) => {
                manual.get_file_only_cached(buildid, file_type).await
            }
        }
    }

    pub async fn get_file(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        match &self.0 {
            DebuginfodDownloaderInner::Official(official) => {
                official.get_file(buildid, file_type).await
            }
            DebuginfodDownloaderInner::Manual(manual) => manual.get_file(buildid, file_type).await,
        }
    }

    /// Set the observer for this downloader.
    ///
    /// The observer can be used for logging, displaying progress bars, informing
    /// automatic expiration of cached files, and so on.
    ///
    /// See the [`DownloaderObserver`] trait for more information.
    pub fn set_observer(&mut self, observer: Option<Arc<dyn DownloaderObserver>>) {
        match &mut self.0 {
            DebuginfodDownloaderInner::Official(official) => official.set_observer(observer),
            DebuginfodDownloaderInner::Manual(manual) => manual.set_observer(observer),
        }
    }
}

/// Uses debuginfod-find on the shell maybe, not sure
struct OfficialDebuginfodDownloader;

impl OfficialDebuginfodDownloader {
    pub fn set_observer(&mut self, _observer: Option<Arc<dyn DownloaderObserver>>) {}

    pub async fn get_file_only_cached(&self, _buildid: &str, _file_type: &str) -> Option<PathBuf> {
        None // TODO
    }

    pub async fn get_file(&self, _buildid: &str, _file_type: &str) -> Option<PathBuf> {
        None // TODO
    }
}

/// A `debuginfod` client, used on non-Linux platforms or on Linux if debuginfod is not installed.
///
/// Does not use the official debuginfod's cache directory because the cache directory structure is not a stable API.
struct ManualDebuginfodDownloader {
    servers_and_caches: Vec<(String, PathBuf)>,
    observer: Option<Arc<dyn DownloaderObserver>>,
    downloader: Arc<Downloader>,
}

impl ManualDebuginfodDownloader {
    pub fn set_observer(&mut self, observer: Option<Arc<dyn DownloaderObserver>>) {
        self.observer = observer;
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

    pub async fn get_file_only_cached(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        for (_server_base_url, cache_dir) in &self.servers_and_caches {
            let cached_file_path = cache_dir.join(buildid).join(file_type);
            if self.check_file_exists(&cached_file_path).await {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&cached_file_path);
                }
                return Some(cached_file_path);
            }
        }
        None
    }

    pub async fn get_file(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        if let Some(f) = self.get_file_only_cached(buildid, file_type).await {
            return Some(f);
        }

        for (server_base_url, cache_dir) in &self.servers_and_caches {
            if let Some(file) = self
                .get_file_from_server(buildid, file_type, server_base_url, cache_dir)
                .await
            {
                return Some(file);
            }
        }
        None
    }

    async fn get_file_from_server(
        &self,
        buildid: &str,
        file_type: &str,
        server_base_url: &str,
        cache_dir: &Path,
    ) -> Option<PathBuf> {
        let dest_path = cache_dir.join(buildid).join(file_type);
        let server_base_url = server_base_url.trim_end_matches('/');
        let url = format!("{server_base_url}/buildid/{buildid}/{file_type}");

        let download = self
            .downloader
            .initiate_download(&url, self.observer.clone())
            .await
            .ok()?;
        download.download_to_file(&dest_path, None).await.ok()?;

        Some(dest_path)
    }
}
