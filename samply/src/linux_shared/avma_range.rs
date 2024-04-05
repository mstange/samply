#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvmaRange {
    start: u64,
    end: u64,
}

impl AvmaRange {
    pub fn with_start_size(start: u64, size: u64) -> Self {
        Self {
            start,
            end: start + size,
        }
    }
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn end(&self) -> u64 {
        self.end
    }
    pub fn size(&self) -> u64 {
        self.end - self.start
    }
    pub fn encompasses(&self, other: &AvmaRange) -> bool {
        self.start <= other.start && self.end >= other.end
    }
}
