use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use samply_symbols::{BreakpadIndex, BreakpadIndexCreator, BreakpadParseError, OwnedBreakpadIndex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::downloader::{ChunkConsumer, Downloader, DownloaderObserver, FileDownloadOutcome};
use crate::file_creation::{create_file_cleanly, CleanFileCreationError};
use crate::DownloadError;

/// The error type used in the observer notification [`DownloaderObserver::on_symindex_generation_failed`].
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

pub struct BreakpadSymbolDownloader {
    inner: Arc<BreakpadSymbolDownloaderInner>,
}

impl BreakpadSymbolDownloader {
    pub fn new(
        breakpad_directories_readonly: Vec<PathBuf>,
        breakpad_servers: Vec<(String, PathBuf)>,
        breakpad_symindex_cache_dir: Option<PathBuf>,
        downloader: Option<Arc<Downloader>>,
    ) -> Self {
        let inner = BreakpadSymbolDownloaderInner {
            breakpad_directories_readonly,
            breakpad_servers,
            breakpad_symindex_cache_dir,
            observer: None,
            downloader: downloader.unwrap_or_default(),
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
    /// See the [`DownloaderObserver`] trait for more information.
    pub fn set_observer(&mut self, observer: Option<Arc<dyn DownloaderObserver>>) {
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
    observer: Option<Arc<dyn DownloaderObserver>>,
    downloader: Arc<Downloader>,
}

impl BreakpadSymbolDownloaderInner {
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

    pub async fn get_file(&self, rel_path: &str) -> Option<PathBuf> {
        if let Some(path) = self.get_file_no_download(rel_path).await {
            return Some(path);
        }

        for (server_base_url, cache_dir) in &self.breakpad_servers {
            if let Ok(path) = self
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

    async fn get_bp_sym_file_from_server(
        &self,
        rel_path: &str,
        server_base_url: &str,
        cache_dir: &Path,
    ) -> Result<PathBuf, DownloadError> {
        let dest_path = cache_dir.join(rel_path);
        let server_base_url = server_base_url.trim_end_matches('/');
        let url = format!("{server_base_url}/{rel_path}");

        let observer = self.observer.clone();
        let download = self.downloader.initiate_download(&url, observer).await?;
        let index_generator = BreakpadIndexCreatorChunkConsumer(BreakpadIndexCreator::new());
        let outcome = download
            .download_to_file_with_chunk_consumer(&dest_path, index_generator)
            .await?;

        match outcome {
            FileDownloadOutcome::DidCreateNewFile(index_result) => {
                if let Ok(index) = index_result {
                    if let Some(symindex_path) = self.symindex_path(rel_path) {
                        let _ = self.write_symindex(&symindex_path, index).await;
                    }
                }
            }
            FileDownloadOutcome::FoundExistingFile => {
                let _ = self.ensure_symindex(&dest_path, rel_path).await;
            }
        }

        Ok(dest_path)
    }

    pub fn symindex_path(&self, rel_path: &str) -> Option<PathBuf> {
        let symindex_dir = self.breakpad_symindex_cache_dir.as_deref()?;
        Some(symindex_dir.join(rel_path).with_extension("symindex"))
    }

    async fn write_symindex(
        &self,
        symindex_path: &Path,
        index: OwnedBreakpadIndex,
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
                |mut index_file| async move {
                    tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
                        index.index().to_writer(&mut index_file)?;
                        let metadata = index_file.metadata()?;
                        Ok(metadata.len())
                    })
                    .await
                    .unwrap()
                    .map_err(SymindexGenerationError::FileWriting)
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

        if let Ok(mut symindex_file) = tokio::fs::File::open(&symindex_path).await {
            let file_check_ok = validate_symindex_magic_and_version(&mut symindex_file).await;
            let _ = symindex_file.flush().await;
            drop(symindex_file);

            if file_check_ok {
                if let Some(observer) = self.observer.as_deref() {
                    observer.on_file_accessed(&symindex_path);
                }
                return Ok(symindex_path);
            }

            // Bad symindex, let's remove it and regenerate it.
            let _ = tokio::fs::remove_file(&symindex_path).await;
        }

        self.create_symindex_for_sym_file(sym_path, &symindex_path)
            .await?;
        Ok(symindex_path)
    }

    async fn create_symindex_for_sym_file(
        &self,
        sym_path: &Path,
        symindex_path: &Path,
    ) -> Result<(), SymindexGenerationError> {
        let index_bytes = self.parse_sym_file_into_index(sym_path).await?;
        self.write_symindex(symindex_path, index_bytes).await?;
        Ok(())
    }

    async fn parse_sym_file_into_index(
        &self,
        sym_path: &Path,
    ) -> Result<OwnedBreakpadIndex, SymindexGenerationError> {
        let sym_path = sym_path.to_path_buf();
        tokio::task::spawn_blocking(|| {
            let mut sym_file =
                std::fs::File::open(sym_path).map_err(SymindexGenerationError::SymReading)?;
            let mut parser = BreakpadIndexCreator::new();
            const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
            let mut buffer = vec![0; CHUNK_SIZE];
            loop {
                let read_len = sym_file
                    .read(&mut buffer)
                    .map_err(SymindexGenerationError::SymReading)?;
                if read_len == 0 {
                    break;
                }
                parser.consume(&buffer[..read_len]);
            }
            parser
                .finish()
                .map_err(SymindexGenerationError::BreakpadParsing)
        })
        .await
        .unwrap()
    }
}

async fn validate_symindex_magic_and_version(file: &mut tokio::fs::File) -> bool {
    let mut magic_and_version_bytes = [0u8; 12];
    file.read_exact(&mut magic_and_version_bytes).await.is_ok()
        && BreakpadIndex::validate_magic_and_version(&magic_and_version_bytes).is_ok()
}

struct BreakpadIndexCreatorChunkConsumer(BreakpadIndexCreator);

impl ChunkConsumer for BreakpadIndexCreatorChunkConsumer {
    type Output = Result<OwnedBreakpadIndex, BreakpadParseError>;

    fn consume_chunk(&mut self, chunk_data: &[u8]) {
        self.0.consume(chunk_data);
    }

    fn finish(self) -> Self::Output {
        self.0.finish()
    }
}
