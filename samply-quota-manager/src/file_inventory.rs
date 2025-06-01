use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, Transaction};

/// Stores information about files (size, last access time) in a sqlite DB.
///
/// It only cares about files in the "managed directory". All file paths
/// are stored as relative paths relative to that directory.
///
/// You tell the inventory when files are added / deleted / accessed.
///
/// When the time comes to enforce a limit on the total size or max age,
/// you can ask it for a list of files to delete.
///
/// The inventory never touches files directly, other than the sqlite DB file.
/// It just stores and queries information.
pub struct FileInventory {
    /// The path of the managed directory.
    root_path: PathBuf,
    /// The database connection.
    db_connection: rusqlite::Connection,
}

/// Information about a file that `FileInventory` keeps track of.
pub struct FileInfo {
    pub path: PathBuf,
    pub size_in_bytes: u64,
    pub creation_time: SystemTime,
    pub last_access_time: SystemTime,
}

impl FileInventory {
    /// Creates a `FileInventory` instance, with the managed directory being passed
    /// in `root_path`. The information is stored in a sqlite file at `db_path`.
    ///
    /// The parent directory of `db_path` must exist.
    ///
    /// If the database at `db_path` does not exist, this function calls the
    /// `list_existing_files_fn` callback so that it can populate the new database.
    pub fn new<F>(
        root_path: &Path,
        db_path: &Path,
        list_existing_files_fn: F,
    ) -> rusqlite_migration::Result<Self>
    where
        F: Fn() -> Vec<FileInfo> + Send + Sync + 'static,
    {
        let root_path = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.to_path_buf());
        let db_connection = Self::init_db_at(&root_path, db_path, list_existing_files_fn)?;

        Ok(Self {
            root_path,
            db_connection,
        })
    }

    /// Returns the canonicalized path of the managed directory.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    fn init_db_at<F>(
        root_path: &Path,
        db_path: &Path,
        list_existing_files_fn: F,
    ) -> rusqlite_migration::Result<rusqlite::Connection>
    where
        F: Fn() -> Vec<FileInfo> + Send + Sync + 'static,
    {
        use rusqlite_migration::{Migrations, M};

        let list_existing_files_fn = Arc::new(list_existing_files_fn);
        let root_path = root_path.to_path_buf();

        let migrations = Migrations::new(vec![
            M::up_with_hook(
                r#"
                    CREATE TABLE "files"
                    (
                        [Path] TEXT NOT NULL,
                        [Size] INT NOT NULL,
                        [CreationTime] INT NOT NULL,
                        [LastAccessTime] INT NOT NULL,
                        PRIMARY KEY ([Path])
                    );
                    CREATE INDEX idx_files_LastAccessTime ON "files" ([LastAccessTime]);
                "#,
                move |transaction: &Transaction| {
                    let existing_files = list_existing_files_fn();
                    Self::insert_existing_files(&root_path, transaction, existing_files);
                    Ok(())
                },
            ),
            // Future migrations can be added here.
        ]);

        // Open the database.
        // SQLITE_OPEN_CREATE only creates the file, it fails if the parent directory
        // doesn't exist.
        let mut conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrations.to_latest(&mut conn)?;

        Ok(conn)
    }

    fn insert_existing_files(
        root_path: &Path,
        transaction: &Transaction,
        existing_files: Vec<FileInfo>,
    ) {
        let mut stmt = transaction
            .prepare(
                r#"
                    INSERT INTO files (Path, Size, CreationTime, LastAccessTime)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(Path) DO UPDATE SET
                        Size=?2,
                        CreationTime=?3,
                        LastAccessTime=?4;
                "#,
            )
            .unwrap();
        for file_info in existing_files {
            let FileInfo {
                path,
                size_in_bytes,
                creation_time,
                last_access_time,
            } = file_info;
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let Ok(relative_path) = path.strip_prefix(root_path) else {
                continue;
            };

            stmt.execute(params![
                relative_path.to_string_lossy(),
                size_in_bytes,
                creation_time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
                last_access_time
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
            ])
            .unwrap();
        }
    }

    fn relative_path_under_managed_directory(&self, path: &Path) -> Option<PathBuf> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let relative_path = path.strip_prefix(&self.root_path).ok()?;
        Some(relative_path.to_path_buf())
    }

    fn to_absolute_path(&self, relative_path: &Path) -> PathBuf {
        let abs_path = self.root_path.join(relative_path);
        let abs_path = path_clean::clean(abs_path);
        assert!(abs_path.starts_with(&self.root_path));
        abs_path
    }

    /// Notifies the inventory that a file has been created.
    ///
    /// If the path is not under the "managed directory", this call is ignored.
    pub fn on_file_created(&mut self, path: &Path, size_in_bytes: u64, creation_time: SystemTime) {
        let Some(relative_path) = self.relative_path_under_managed_directory(path) else {
            return;
        };

        let creation_time = creation_time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let last_access_time = creation_time;

        let mut stmt = self
            .db_connection
            .prepare_cached(
                r#"
                    INSERT INTO files (Path, Size, CreationTime, LastAccessTime)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(Path) DO UPDATE SET
                        Size=?2,
                        CreationTime=?3,
                        LastAccessTime=?4;
                "#,
            )
            .unwrap();
        stmt.execute(params![
            relative_path.to_string_lossy(),
            size_in_bytes as i64,
            creation_time,
            last_access_time
        ])
        .unwrap();
    }

    /// Notifies the inventory that a file has been accessed.
    ///
    /// If the path is not under the "managed directory", this call is ignored.
    pub fn on_file_accessed(&mut self, path: &Path, access_time: SystemTime) {
        let Some(relative_path) = self.relative_path_under_managed_directory(path) else {
            return;
        };

        let access_time = access_time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        let mut stmt = self
            .db_connection
            .prepare_cached("UPDATE files SET LastAccessTime = ?1 WHERE Path = ?2")
            .unwrap();
        stmt.execute(params![access_time, relative_path.to_string_lossy()])
            .unwrap();
    }

    /// Notifies the inventory that a file has been deleted.
    ///
    /// If the path is not under the "managed directory", this call is ignored.
    pub fn on_file_deleted(&mut self, path: &Path) {
        let Some(relative_path) = self.relative_path_under_managed_directory(path) else {
            return;
        };

        let mut stmt = self
            .db_connection
            .prepare_cached("DELETE FROM files WHERE Path = ?1")
            .unwrap();
        stmt.execute(params![relative_path.to_string_lossy()])
            .unwrap();
    }

    /// Notifies the inventory that no file has been observed at the path.
    ///
    /// If the path is not under the "managed directory", this call is ignored.
    pub fn on_file_found_to_be_absent(&mut self, path: &Path) {
        self.on_file_deleted(path);
    }

    /// Returns the total size of all files under the managed directory. This only uses
    /// the information stored in the database, it doesn't look at the file system.
    pub fn total_size_in_bytes(&self) -> u64 {
        let total_size: i64 = self
            .db_connection
            .query_row("SELECT SUM(Size) FROM files", [], |row| row.get(0))
            .unwrap_or(0);
        total_size as u64
    }

    fn file_info_from_row(&self, row: &rusqlite::Row) -> rusqlite::Result<FileInfo> {
        let relative_path: String = row.get(0)?;
        let size: i64 = row.get(1)?;
        let creation_time: i64 = row.get(2)?;
        let last_access_time: i64 = row.get(3)?;
        let path = self.to_absolute_path(Path::new(&relative_path));
        Ok(FileInfo {
            path,
            size_in_bytes: size.try_into().unwrap(),
            creation_time: SqliteTime(creation_time).into(),
            last_access_time: SqliteTime(last_access_time).into(),
        })
    }

    /// Returns a list of file paths. Deleting all the listed files will reduce
    /// the total size of the managed directory below `max_size_bytes`, assuming
    /// that the information stored in the DB is complete and accurate.
    pub fn get_files_to_delete_to_enforce_max_size(&self, max_size_bytes: u64) -> Vec<FileInfo> {
        let total_size = self.total_size_in_bytes();
        if total_size <= max_size_bytes {
            // Nothing needs to be deleted.
            return vec![];
        };

        let mut excess_bytes = i64::try_from(total_size - max_size_bytes).unwrap();
        assert!(excess_bytes > 0);

        let mut stmt = self
            .db_connection
            .prepare_cached("SELECT Path, Size, CreationTime, LastAccessTime FROM files ORDER BY LastAccessTime ASC")
            .unwrap();

        let files = stmt
            .query_map([], |row| self.file_info_from_row(row))
            .unwrap()
            .filter_map(Result::ok);

        // Add files to a list until we've accumulated enough size.
        let mut files_to_delete = vec![];
        for file_info in files {
            let size = i64::try_from(file_info.size_in_bytes).unwrap();

            files_to_delete.push(file_info);
            excess_bytes = excess_bytes.checked_sub(size).unwrap();
            if excess_bytes <= 0 {
                break;
            }
        }

        // Now we know: Deleting all files in `files_to_delete` will definitely free up
        // enough space.
        // But it may free up more space than necessary! There might be some really big
        // files at the end of the list, making it unnecessary to delete some of the
        // smaller, less recently accessed files near the start of the list.
        if excess_bytes < 0 {
            // Find a subset of `files_to_delete` which still frees enough space.
            let available_bytes = excess_bytes.checked_neg().unwrap();
            let mut available_bytes = u64::try_from(available_bytes).unwrap();

            // `files_to_delete` is currently ordered from least recently-used ("oldest")
            // to most recently-used ("newest").
            // Reverse the order so that we visit the most-recently used files first.
            files_to_delete.reverse();
            files_to_delete.retain(|file_info| {
                if file_info.size_in_bytes <= available_bytes {
                    // This file can stay.
                    available_bytes -= file_info.size_in_bytes;
                    false // false means "don't retain in the list of files to delete", i.e. "don't delete", i.e. "keep this file alive"
                } else {
                    true
                }
            });
        }

        // Delete the largest files first.
        files_to_delete.sort_unstable_by_key(|file_info| {
            let size = i32::try_from(file_info.size_in_bytes).unwrap();
            let negative_size = size.checked_neg().unwrap();
            (negative_size, file_info.last_access_time)
        });
        files_to_delete
    }

    /// Returns a list of file paths, listing all the files whose last access time (as
    /// stored by the inventory) is older than `max_age_seconds`.
    pub fn get_files_last_accessed_before(&self, cutoff_time: SystemTime) -> Vec<FileInfo> {
        let mut stmt = self
            .db_connection
            .prepare_cached("SELECT Path, Size, CreationTime, LastAccessTime FROM files WHERE LastAccessTime < ?1")
            .unwrap();

        let files_to_delete = stmt
            .query_map([SqliteTime::from(cutoff_time).0], |row| {
                self.file_info_from_row(row)
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        files_to_delete
    }
}

struct SqliteTime(pub i64);

impl From<SqliteTime> for SystemTime {
    fn from(value: SqliteTime) -> Self {
        let dur = Duration::from_secs(u64::try_from(value.0).unwrap());
        UNIX_EPOCH.checked_add(dur).unwrap()
    }
}

impl From<SystemTime> for SqliteTime {
    fn from(value: SystemTime) -> Self {
        let dur = value.duration_since(UNIX_EPOCH).unwrap();
        Self(dur.as_secs().try_into().unwrap())
    }
}
