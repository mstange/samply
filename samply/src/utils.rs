use std::path::{Path, PathBuf};

pub fn open_file_with_fallback(
    path: &Path,
    extra_dir: Option<&Path>,
) -> std::io::Result<(std::fs::File, PathBuf)> {
    match (std::fs::File::open(path), extra_dir, path.file_name()) {
        (Ok(file), _, _) => Ok((file, path.to_owned())),
        (Err(_), Some(extra_dir), Some(filename)) => {
            let p: PathBuf = [extra_dir, Path::new(filename)].iter().collect();
            std::fs::File::open(&p).map(|file| (file, p))
        }
        (Err(e), _, _) => Err(e),
    }
}
