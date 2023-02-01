use std::path::{Path, PathBuf};

use symsrv::{memmap2, FileContents};

pub struct DebuginfodSymbolCache(DebuginfodSymbolCacheInner);

enum DebuginfodSymbolCacheInner {
    #[allow(unused)]
    Official(OfficialDebuginfodSymbolCache),
    Manual(ManualDebuginfodSymbolCache),
}

impl DebuginfodSymbolCache {
    pub fn new(
        debuginfod_cache_dir_if_not_installed: Option<PathBuf>,
        mut servers_and_caches: Vec<(String, PathBuf)>,
        verbose: bool,
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
                servers_and_caches.extend(extra_servers.into_iter());
            }
            Self(DebuginfodSymbolCacheInner::Manual(
                ManualDebuginfodSymbolCache {
                    servers_and_caches,
                    verbose,
                },
            ))
        }
    }

    #[allow(unused)]
    pub async fn get_file_only_cached(
        &self,
        buildid: &str,
        file_type: &str,
    ) -> Option<symsrv::FileContents> {
        match &self.0 {
            DebuginfodSymbolCacheInner::Official(official) => {
                official.get_file_only_cached(buildid, file_type).await
            }
            DebuginfodSymbolCacheInner::Manual(manual) => {
                manual.get_file_only_cached(buildid, file_type).await
            }
        }
    }

    pub async fn get_file(&self, buildid: &str, file_type: &str) -> Option<symsrv::FileContents> {
        match &self.0 {
            DebuginfodSymbolCacheInner::Official(official) => {
                official.get_file(buildid, file_type).await
            }
            DebuginfodSymbolCacheInner::Manual(manual) => manual.get_file(buildid, file_type).await,
        }
    }
}

/// Uses debuginfod-find on the shell maybe, not sure
struct OfficialDebuginfodSymbolCache;

impl OfficialDebuginfodSymbolCache {
    pub async fn get_file_only_cached(
        &self,
        _buildid: &str,
        _file_type: &str,
    ) -> Option<symsrv::FileContents> {
        None // TODO
    }

    pub async fn get_file(&self, _buildid: &str, _file_type: &str) -> Option<symsrv::FileContents> {
        None // TODO
    }
}

/// Full reimplementation of a `debuginfod` client, used on non-Linux platforms or on Linux if debuginfod is not installed.
///
/// Does not use the official debuginfod's cache directory because the cache directory structure is not a stable API.
struct ManualDebuginfodSymbolCache {
    servers_and_caches: Vec<(String, PathBuf)>,
    verbose: bool,
}

impl ManualDebuginfodSymbolCache {
    pub async fn get_file_only_cached(
        &self,
        buildid: &str,
        file_type: &str,
    ) -> Option<symsrv::FileContents> {
        for (_server_base_url, cache_dir) in &self.servers_and_caches {
            let cached_file_path = cache_dir.join(buildid).join(file_type);
            if self.verbose {
                eprintln!("Opening file {:?}", cached_file_path.to_string_lossy());
            }
            if let Ok(file) = std::fs::File::open(&cached_file_path) {
                return Some(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file).ok()?
                }));
            }
        }
        None
    }

    pub async fn get_file(&self, buildid: &str, file_type: &str) -> Option<symsrv::FileContents> {
        if let Some(f) = self.get_file_only_cached(buildid, file_type).await {
            return Some(f);
        }

        for (server_base_url, cache_dir) in &self.servers_and_caches {
            if let Ok(file) = self
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
    ) -> Result<FileContents, Box<dyn std::error::Error>> {
        let url = format!("{server_base_url}/buildid/{buildid}/{file_type}");
        if self.verbose {
            eprintln!("Downloading {url}...");
        }
        let sym_file_response = reqwest::get(&url).await?.error_for_status()?;
        let mut stream = sym_file_response.bytes_stream();
        let dest_path = cache_dir.join(buildid).join(file_type);
        if let Some(dir) = dest_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        if self.verbose {
            eprintln!("Saving bytes to {dest_path:?}.");
        }
        let file = tokio::fs::File::create(&dest_path).await?;
        let mut writer = tokio::io::BufWriter::new(file);
        use futures_util::StreamExt;
        while let Some(item) = stream.next().await {
            tokio::io::copy(&mut item?.as_ref(), &mut writer).await?;
        }
        drop(writer);
        if self.verbose {
            eprintln!("Opening file {:?}", dest_path.to_string_lossy());
        }
        let file = std::fs::File::open(&dest_path)?;
        Ok(FileContents::Mmap(unsafe {
            memmap2::MmapOptions::new().map(&file)?
        }))
    }
}
