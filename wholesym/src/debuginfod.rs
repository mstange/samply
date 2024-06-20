use std::path::{Path, PathBuf};

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
                servers_and_caches.extend(extra_servers);
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
    pub async fn get_file_only_cached(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        match &self.0 {
            DebuginfodSymbolCacheInner::Official(official) => {
                official.get_file_only_cached(buildid, file_type).await
            }
            DebuginfodSymbolCacheInner::Manual(manual) => {
                manual.get_file_only_cached(buildid, file_type).await
            }
        }
    }

    pub async fn get_file(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
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
    pub async fn get_file_only_cached(&self, _buildid: &str, _file_type: &str) -> Option<PathBuf> {
        None // TODO
    }

    pub async fn get_file(&self, _buildid: &str, _file_type: &str) -> Option<PathBuf> {
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
    pub async fn get_file_only_cached(&self, buildid: &str, file_type: &str) -> Option<PathBuf> {
        for (_server_base_url, cache_dir) in &self.servers_and_caches {
            let cached_file_path = cache_dir.join(buildid).join(file_type);
            if cached_file_path.exists() {
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
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let server_base_url = server_base_url.trim_end_matches('/');
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
        Ok(dest_path)
    }
}
