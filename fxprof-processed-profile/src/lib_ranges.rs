#[derive(Debug, Clone)]
pub struct LibRanges<I> {
    sorted_lib_ranges: Vec<LibRange<I>>,
}

impl<I> Default for LibRanges<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> LibRanges<I> {
    pub fn new() -> Self {
        Self {
            sorted_lib_ranges: Vec::new(),
        }
    }

    pub fn insert(&mut self, range: LibRange<I>) {
        let insertion_index = match self
            .sorted_lib_ranges
            .binary_search_by_key(&range.start, |r| r.start)
        {
            Ok(i) => {
                // We already have a library mapping at this address.
                // Not sure how to best deal with it. Ideally it wouldn't happen. Let's just remove this mapping.
                self.sorted_lib_ranges.remove(i);
                i
            }
            Err(i) => i,
        };

        self.sorted_lib_ranges.insert(insertion_index, range);
    }

    pub fn remove(&mut self, base_address: u64) {
        self.sorted_lib_ranges.retain(|r| r.base != base_address);
    }

    pub fn lookup(&self, address: u64) -> Option<&LibRange<I>> {
        let ranges = &self.sorted_lib_ranges[..];
        let index = match ranges.binary_search_by_key(&address, |r| r.start) {
            Err(0) => return None,
            Ok(exact_match) => exact_match,
            Err(insertion_index) => {
                let range_index = insertion_index - 1;
                if address < ranges[range_index].end {
                    range_index
                } else {
                    return None;
                }
            }
        };
        Some(&ranges[index])
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
pub struct LibRange<I> {
    pub start: u64,
    pub end: u64,
    pub lib_index: I,
    pub base: u64,
}
