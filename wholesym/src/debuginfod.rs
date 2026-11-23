use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::AsyncBufReadExt;

use crate::downloader::{Downloader, DownloaderObserver};

const OFFICIAL_DEBUGINFOD_BINARY: &str = "debuginfod-find";

pub struct DebuginfodDownloader(DebuginfodDownloaderInner);

#[derive(Clone)]
pub enum DebuginfodRequestType {
    Debuginfo,
    Executable,
    Source(String),
}
impl DebuginfodRequestType {
    fn subcommand(&self) -> &'static str {
        match self {
            DebuginfodRequestType::Debuginfo => "debuginfo",
            DebuginfodRequestType::Executable => "executable",
            DebuginfodRequestType::Source(_) => "source",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DebuginfodError {
    #[error("Unknown debuginfod error: {0}")]
    UnknownDebuginfodError(String),
}

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
        let debuginfod_test_result = std::process::Command::new(OFFICIAL_DEBUGINFOD_BINARY)
            .arg("--version")
            .spawn();
        let mut debuginfod_urls: Vec<String> = std::env::var("DEBUGINFOD_URLS")
            .unwrap_or_default()
            .split_ascii_whitespace()
            .flat_map(String::from_str)
            .collect();

        match debuginfod_test_result {
            Ok(_) => {
                for (server, _) in servers_and_caches {
                    if !debuginfod_urls.contains(&server) {
                        debuginfod_urls.push(server);
                    }
                }
                Self(DebuginfodDownloaderInner::Official(
                    OfficialDebuginfodDownloader {
                        all_urls: debuginfod_urls,
                        observer: None,
                    },
                ))
            }
            Err(_) => {
                if let Some(cache_dir) = debuginfod_cache_dir_if_not_installed {
                    if !debuginfod_urls.is_empty() {
                        let mut servers_from_env = Vec::new();
                        for url in debuginfod_urls {
                            servers_from_env.push((url.to_string(), cache_dir.clone()));
                        }
                        let extra_servers =
                            std::mem::replace(&mut servers_and_caches, servers_from_env);
                        servers_and_caches.extend(extra_servers);
                    }
                }

                println!("debuginfod builtin");
                Self(DebuginfodDownloaderInner::Manual(
                    ManualDebuginfodDownloader {
                        servers_and_caches,
                        observer: None,
                        downloader: downloader.unwrap_or_default(),
                    },
                ))
            }
        }
    }

    #[allow(unused)]
    pub async fn get_file_only_cached(
        &self,
        buildid: &str,
        file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
        match &self.0 {
            DebuginfodDownloaderInner::Official(official) => {
                official.get_file_only_cached(buildid, file_type).await
            }
            DebuginfodDownloaderInner::Manual(manual) => {
                manual.get_file_only_cached(buildid, file_type).await
            }
        }
    }

    pub async fn get_file(
        &self,
        buildid: &str,
        file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
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

/// Uses debuginfod-find on the shell
struct OfficialDebuginfodDownloader {
    all_urls: Vec<String>,
    observer: Option<Arc<dyn DownloaderObserver>>,
}

impl OfficialDebuginfodDownloader {
    pub fn set_observer(&mut self, observer: Option<Arc<dyn DownloaderObserver>>) {
        self.observer = observer;
    }

    pub async fn get_file_only_cached(
        &self,
        _buildid: &str,
        _file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
        None // TODO: debuginfod-find doesn't seem to have a 'get cached without fetch' option, skip it.
    }

    pub async fn get_file(
        &self,
        buildid: &str,
        file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
        let mut cmd = tokio::process::Command::new("debuginfod-find");

        cmd.env("DEBUGINFOD_URLS", self.all_urls.join(" "));

        if self.observer.is_some() {
            cmd.arg("-vv"); // need verbose output for reporting to observer
        }
        cmd.arg(file_type.subcommand())
            .arg(buildid)
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());

        if let DebuginfodRequestType::Source(source_file) = file_type {
            cmd.arg(source_file);
        }

        let mut child = cmd.spawn().ok()?;
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take()?;

        if let Some(ref observer) = self.observer {
            let observer = Arc::clone(observer);
            tokio::spawn(async move {
                let mut buildid = None;
                let mut cache_dir = None;
                let mut found = false;
                let mut time_until_headers = None;
                let mut raw_size = 0;
                let mut chosen_dlid = None;
                let mut chosen_start = None;
                let mut urls = vec![];
                let mut stderr = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr.next_line().await {
                    let words: Vec<&str> = line.split_whitespace().collect();
                    match words.as_slice() {
                        ["debuginfod_find_debuginfo", buildid_] => {
                            buildid = Some(buildid_.to_string());
                        }
                        ["debuginfod_find_source", buildid_, _path] => {
                            buildid = Some(buildid_.to_string());
                        }

                        ["checking", "cache", "dir", dir] => {
                            cache_dir = Some(PathBuf::from_str(dir).unwrap());
                        }
                        ["found", path, ..] if path.ends_with("/debuginfo") => {
                            found = true;
                            println!("from debuginfod downloader:");
                            observer.on_file_accessed(Path::new(path));
                        }
                        ["init", "server", ..] if !found => {
                            // logged when setting up
                            if let Some(ref cache_dir) = cache_dir {
                                if let Some(ref buildid) = buildid {
                                    observer.on_file_missed(cache_dir.join(buildid).as_path());
                                }
                            }
                        }
                        ["url", i, url] => {
                            if let Ok(i) = i.parse::<usize>() {
                                let mut hasher = DefaultHasher::new();
                                (i, url).hash(&mut hasher);
                                let download_id = hasher.finish();
                                if i + 1 > urls.len() {
                                    urls.resize(i + 1, None);
                                }
                                urls[i] = Some((download_id, Instant::now(), url.to_string()));
                                observer.on_new_download_before_connect(download_id, url);
                            }
                        }
                        // ["header", http, status] if http.starts_with("HTTP/") => {
                        //     if let Ok(status) = status.parse::<u32>(){
                        //         if status >= 200 && status < 300 {
                        //             observer.on_download_started(...);
                        //         }
                        //     }
                        // }
                        // committed only happens when one server responded with 200
                        ["committed", "to", "url", i] => {
                            if let Ok(chosen_idx) = i.parse::<usize>() {
                                for (idx, v) in urls.iter().enumerate() {
                                    if let Some((download_id, start, _url)) = v.as_ref() {
                                        if idx == chosen_idx {
                                            time_until_headers = Some(start.elapsed());
                                            observer.on_download_started(*download_id);
                                            chosen_dlid = Some(*download_id);
                                            chosen_start = Some(*start);
                                        } else {
                                            observer.on_download_canceled(*download_id);
                                        };
                                    };
                                }
                            }
                        }
                        ["Progress", bytes, "/", total_bytes] if chosen_dlid.is_some() => {
                            if let Ok(bytes) = bytes.parse::<u64>() {
                                if let Ok(total_bytes) = total_bytes.parse::<u64>() {
                                    observer.on_download_progress(
                                        chosen_dlid.unwrap(),
                                        bytes,
                                        Some(total_bytes),
                                    );
                                }
                            }
                        }
                        ["x-debuginfod-size:", size] => {
                            if let Ok(size) = size.parse() {
                                raw_size = size;
                            }
                        }
                        // only on success, as otherwise urls[0] potentially isn't the only url
                        ["server", "response", "No", "Error"]
                            if time_until_headers.is_some() && chosen_dlid.is_some() =>
                        {
                            observer.on_download_completed(
                                chosen_dlid.unwrap(),
                                raw_size,
                                time_until_headers.unwrap(),
                                chosen_start.unwrap().elapsed(),
                            );
                        }
                        ["url", i, "The", "requested", "URL", "returned", "error:", code]
                            if chosen_dlid.is_none() =>
                        {
                            if let Ok(i) = i.parse::<usize>() {
                                if let Some((download_id, _, _)) = urls[i] {
                                    // if there is no chosen_i, on_download_started was never called, so start it now to uphold
                                    // the structure expected by observer
                                    observer.on_download_started(download_id);
                                    if let Ok(code) = code.parse() {
                                        observer.on_download_failed(
                                            download_id,
                                            crate::DownloadError::StatusError(code),
                                        );
                                    }
                                }
                            }
                        }
                        // if it was started, but still failed, its an error which is not tracked yet
                        ["Server", "query", "failed:", reason @ ..] if chosen_dlid.is_some() => {
                            observer.on_download_failed(
                                chosen_dlid.unwrap(),
                                crate::DownloadError::Other(Box::new(
                                    DebuginfodError::UnknownDebuginfodError(reason.concat()),
                                )),
                            );
                        }
                        _ => (),
                    }
                }
            });
        }
        let status = child.wait().await;
        if status.is_err() {
            return None;
        }
        let mut stdout = tokio::io::BufReader::new(stdout).lines();

        let file = stdout
            .next_line()
            .await
            .ok()
            .flatten()
            .map(|path| PathBuf::from_str(&path).unwrap());
        file
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

    pub async fn get_file_only_cached(
        &self,
        buildid: &str,
        file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
        for (_server_base_url, cache_dir) in &self.servers_and_caches {
            let cached_file_path = cache_dir.join(buildid).join(file_type.subcommand());
            if self.check_file_exists(&cached_file_path).await {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&cached_file_path);
                }
                return Some(cached_file_path);
            }
        }
        None
    }

    pub async fn get_file(
        &self,
        buildid: &str,
        file_type: DebuginfodRequestType,
    ) -> Option<PathBuf> {
        if let Some(f) = self.get_file_only_cached(buildid, file_type.clone()).await {
            return Some(f);
        }

        for (server_base_url, cache_dir) in &self.servers_and_caches {
            if let Some(file) = self
                .get_file_from_server(buildid, file_type.clone(), server_base_url, cache_dir)
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
        file_type: DebuginfodRequestType,
        server_base_url: &str,
        cache_dir: &Path,
    ) -> Option<PathBuf> {
        let subcommand = file_type.subcommand();
        let dest_path = cache_dir.join(buildid).join(subcommand);
        let server_base_url = server_base_url.trim_end_matches('/');
        let mut url = format!("{server_base_url}/buildid/{buildid}/{subcommand}");

        if let DebuginfodRequestType::Source(source_file) = file_type {
            // debuginfod source file request example:
            // <base_url>/buildid/<buildid>/source/usr/src/debug/glibc/glibc/csu/../csu/libc-start.c
            url += &source_file;
        }

        let download = self
            .downloader
            .initiate_download(&url, self.observer.clone())
            .await
            .ok()?;
        download.download_to_file(&dest_path).await.ok()?;

        Some(dest_path)
    }
}
