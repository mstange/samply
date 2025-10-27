use super::types::MarkerFieldKind;

/// Used in [`MarkerFieldsTrait::FIELD_KIND_COUNTS`].
#[derive(Default, Debug, Clone)]
pub struct MarkerFieldKindCounts {
    pub string_field_count: usize,
    pub number_field_count: usize,
    pub flow_field_count: usize,
}

impl MarkerFieldKindCounts {
    pub const fn new() -> Self {
        Self {
            string_field_count: 0,
            number_field_count: 0,
            flow_field_count: 0,
        }
    }

    pub const fn add(&mut self, kind: MarkerFieldKind) {
        match kind {
            MarkerFieldKind::String => self.string_field_count += 1,
            MarkerFieldKind::Number => self.number_field_count += 1,
            MarkerFieldKind::Flow => self.flow_field_count += 1,
        }
    }

    pub const fn from_kind(kind: MarkerFieldKind) -> Self {
        let mut counts = Self::new();
        counts.add(kind);
        counts
    }

    pub const fn from_kinds(kinds: &[MarkerFieldKind]) -> Self {
        let mut counts = Self::new();
        let mut i = 0;
        let len = kinds.len();
        while i < len {
            counts.add(kinds[i]);
            i += 1;
        }
        counts
    }
}
