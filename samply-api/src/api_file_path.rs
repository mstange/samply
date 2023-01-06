use samply_symbols::FilePath;

pub fn to_api_file_path(file_path: &FilePath) -> String {
    match file_path.mapped_path() {
        Some(mapped_path) => mapped_path.to_special_path_str(),
        None => file_path.raw_path().to_owned(),
    }
}
