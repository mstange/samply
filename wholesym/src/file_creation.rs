use std::io;
use std::path::Path;

use fs4::{fs_std::FileExt, lock_contended_error};

/// The error type for the `create_file_cleanly` function.
#[derive(thiserror::Error, Debug)]
pub enum CleanFileCreationError<E: std::error::Error + Send + Sync + 'static> {
    #[error("The destination path is invalid (no filename)")]
    InvalidPath,

    #[error("The lockfile could not be created: {0}")]
    LockFileCreation(io::Error),

    #[error("The temporary file could not be created: {0}")]
    TempFileCreation(io::Error),

    #[error("The temporary file could not be locked: {0}")]
    LockFileLocking(io::Error),

    #[error("The callback function indicated an error: {0}")]
    CallbackIndicatedError(E),

    #[error("The temporary file could not be renamed to the destination file: {0}")]
    RenameError(io::Error),
}

impl<E: std::error::Error + Send + Sync + 'static> From<CleanFileCreationError<E>> for io::Error {
    fn from(e: CleanFileCreationError<E>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, e)
    }
}

/// Creates a file at `dest_path` with the contents written by `write_fn`.
/// If the file already exists, we call `handle_existing_fn`.
/// If multiple calls to `create_file_cleanly` (in this process or even in different processes)
/// try to create the same destination file at the same time, the first call will create the
/// file and the other calls will block on the file lock until the destination file is present,
/// and then call `handle_existing_fn`.
///
/// `write_fn` must drop the file before it returns.
///
/// This function tries to minimize the chance of leaving a partially-written file at the dest_path;
/// the final file is only created once the write function has returned successfully.
/// This is achieved by writing to a temporary file and then renaming it to the final file.
///
/// We use file locks to coordinate multiple attempted writes of the same file from different
/// processes running `create_file_cleanly`. This is done with a separate lock file.
/// The lock file is locked for the full duration of "create temp file, write into temp file, rename
/// to dest file". Once it's unlocked, the presence or absence of the destination file clearly
/// indicates whether we've succeeded or failed to create the destination file.
///
/// The steps are:
///
/// 1. Create a lock file in the same directory as the final file.
/// 2. Lock the lock file for exclusive write access.
/// 3. Create a temporary file in the same directory as the final file, truncating it if it already exists.
/// 4. Call `write_fn` with the temporary file, and wait for `write_fn` to complete successfully.
/// 5. Close the temporary file.
/// 6. Rename the temporary file to the final file.
/// 7. Close (and automatically unlock) the lock file.
/// 8. Remove the lock file.
///
/// In regular failure cases (full disk, other IO errors, etc), we try to clean up the temporary
/// file. If this process is terminated before we can do so, the temporary file will be left
/// behind.
/// We also clean up the lock file in the case where the destination file has been successfully
/// created. In failure cases, we do not clean up the lock file because it's hard to do so without
/// interfering with other processes which might have started using the lock file in the meantime.
pub async fn create_file_cleanly<E, F, FE, G, GE, V>(
    dest_path: &Path,
    write_fn: F,
    handle_existing_fn: FE,
) -> Result<V, CleanFileCreationError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
    G: std::future::Future<Output = Result<V, E>>,
    GE: std::future::Future<Output = Result<V, E>>,
    F: FnOnce(std::fs::File) -> G,
    FE: FnOnce() -> GE,
{
    let Some(file_name) = dest_path.file_name() else {
        return Err(CleanFileCreationError::InvalidPath);
    };

    // Create a lock file in the same directory as the final file, or open it if it already exists.
    let lock_file_path = dest_path.with_file_name(format!("{}.lock", file_name.to_string_lossy()));
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_file_path)
        .map_err(CleanFileCreationError::LockFileCreation)?;

    let locked_file = lock_file_exclusive(lock_file)
        .await
        .map_err(CleanFileCreationError::LockFileLocking)?;

    // Now the lock file is locked by us. We may be in any one of the following scenarios:
    //  - No collision, destination file doesn't exist yet: We made a new lock file, we locked
    //    it immediately. This is the most common case.
    //  - No collision, but destination file was already there all along: Someone else had
    //    already finished creating this file and cleaned up the lock file before we got here.
    //    We made a new lock file without much purpose.
    //  - The lock call above blocked and we waited for someone else to finish: The destination
    //    file did not exist when we locked, but the temp file and lock file existed, and we
    //    blocked on the lock file being released. It was released once the temp file had been
    //    successfully renamed to the destination file.
    //  - The lock call above blocked but the lock got released before the destination file
    //    existed: This means someone else was trying to create the destination file but failed
    //    to do so.
    //  - The lock call above didn't block but a leftover lock file or temp file might exist
    //    because someone else had failed in the middle of downloading and left some remnants.

    // Check if the destination file is already there.
    let destination_file_exists =
        matches!(std::fs::metadata(dest_path), Ok(meta) if meta.is_file());
    if destination_file_exists {
        // Someone else has already fully created the destination file.
        // This happened either before or after we attempted to lock the lock file.
        // Clean up the lock file we created. There probably is no temp file because
        // the original temp file was renamed into the destination file.
        drop(locked_file);
        let _ = std::fs::remove_file(&lock_file_path);
        let v = handle_existing_fn()
            .await
            .map_err(CleanFileCreationError::CallbackIndicatedError)?;
        return Ok(v);
    }

    // Create the temporary file, or open it if it already exists, and truncate it.
    let temp_file_path = dest_path.with_file_name(format!("{}.part", file_name.to_string_lossy()));
    let temp_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_file_path)
        .map_err(CleanFileCreationError::TempFileCreation)?;

    // Call the write function with the temporary file. We pass ownership of the
    // file to the write function. The write function is responsible for dropping
    // the file before it returns - this will close the file.
    let write_result = write_fn(temp_file).await;

    // The temp file is now closed.

    // If the write callback failed, propagate the error.
    let v = match write_result {
        Ok(v) => v,
        Err(write_error) => {
            // Remove the temporary file and close + unlock the lock file.
            let _ = std::fs::remove_file(&temp_file_path);
            drop(locked_file);
            return Err(CleanFileCreationError::CallbackIndicatedError(write_error));
        }
    };

    // Everything seems to have worked out. The file has been written to successfully.
    // Rename it to its final path.
    match std::fs::rename(&temp_file_path, dest_path) {
        Ok(_) => {}
        Err(rename_error) => {
            // Renaming failed; remove the temporary file and close + unlock the lock file.
            let _ = std::fs::remove_file(&temp_file_path);
            drop(locked_file);
            return Err(CleanFileCreationError::RenameError(rename_error));
        }
    }

    // The destination file is in place. Remove the lock file and indicate success.
    // We remove the lock file in this case because, unlike in the failure cases,
    // in the success case there's no reason to ever need this lock file again -
    // if anybody else is currently blocking on the file lock, or if anyone else
    // locks the lock file between the time that we close + unlock the file and the
    // time that we remove it, they'll just notice that the destination file is
    // already there and clean up after themselves.
    drop(locked_file);
    let _ = std::fs::remove_file(&lock_file_path);

    Ok(v)
}

async fn lock_file_exclusive(file: std::fs::File) -> Result<std::fs::File, io::Error> {
    // Use try_lock_exclusive first. If it returns WouldBlock, do the actual blocking locking,
    // but do it on a different thread so that this thread is available for other tokio tasks.
    // We do try_lock_exclusive first so that we can avoid launching a new thread in the common case.

    // We have a retry loop here because file locking can be interrupted by signals.
    for _ in 0..5 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.raw_os_error() == lock_contended_error().raw_os_error() => {
                return lock_file_exclusive_with_blocking_thread(file).await
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "File locking was interrupted too many times",
    ))
}

async fn lock_file_exclusive_with_blocking_thread(
    file: std::fs::File,
) -> Result<std::fs::File, io::Error> {
    // Launch a new thread and call `lock_file_exclusive_blocking_this_thread` in it.
    //
    // We don't use tokio::spawn_blocking because you can only use it with tasks that
    // will make progress even if all other tasks / threads are paused. Grabbing an
    // exclusive file lock cannot make that guarantee; the lock might be held by
    // someone in this process who needs to make progress before unlocking, and they
    // might need tokio's blocking thread pool to do so.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("flock".to_string())
        .spawn(move || {
            let locked_file_result = lock_file_exclusive_blocking_this_thread(file);
            let _ = tx.send(locked_file_result);
        })
        .expect("couldn't create flock thread");

    let locked_file = rx.await.expect("flock thread disappeared unexpectedly")?;

    thread.join().expect("flock thread panicked");

    Ok(locked_file)
}

fn lock_file_exclusive_blocking_this_thread(
    file: std::fs::File,
) -> Result<std::fs::File, io::Error> {
    // We have a retry loop here because file locking can be interrupted by signals.
    for _ in 0..5 {
        match file.lock_exclusive() {
            // can block
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "File locking was interrupted too many times",
    ))
}
