mod index;
mod symbol_map;

pub use index::{
    BreakpadIndex, BreakpadIndexParser, BreakpadParseError, BreakpadSymindexParseError,
};
pub use symbol_map::get_symbol_map_for_breakpad_sym;

use crate::{FileContents, FileContentsWrapper};

pub fn is_breakpad_file<T: FileContents>(file_contents: &FileContentsWrapper<T>) -> bool {
    const MAGIC_BYTES: &[u8] = b"MODULE ";
    matches!(
        file_contents.read_bytes_at(0, MAGIC_BYTES.len() as u64),
        Ok(MAGIC_BYTES)
    )
}
