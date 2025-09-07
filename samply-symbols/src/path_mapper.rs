use crate::mapped_path::UnparsedMappedPath;

pub trait ExtraPathMapper {
    fn map_path(&mut self, path: &str) -> Option<UnparsedMappedPath>;
}

impl ExtraPathMapper for () {
    fn map_path(&mut self, _path: &str) -> Option<UnparsedMappedPath> {
        None
    }
}

pub struct PathMapper<E: ExtraPathMapper> {
    extra_mapper: Option<E>,
}

impl<E: ExtraPathMapper> PathMapper<E> {
    pub fn new() -> Self {
        Self::new_with_maybe_extra_mapper(None)
    }

    pub fn new_with_maybe_extra_mapper(extra_mapper: Option<E>) -> Self {
        PathMapper { extra_mapper }
    }

    /// Compute the mapped path for a raw path.
    pub fn map_path(&mut self, raw_path: &str) -> UnparsedMappedPath {
        if let Some(extra_mapper) = &mut self.extra_mapper {
            if let Some(mapped_path) = extra_mapper.map_path(raw_path) {
                return mapped_path;
            }
        }

        UnparsedMappedPath::RawPath(raw_path.to_string())
    }
}
