use std::borrow::Cow;

use samply_symbols::SourceFilePath;

pub fn to_api_file_path<'a>(file_path: &'a SourceFilePath<'_>) -> Cow<'a, str> {
    file_path
        .special_path_str()
        .unwrap_or_else(|| file_path.raw_path().into())
}
