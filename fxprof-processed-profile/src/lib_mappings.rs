/// Keeps track of mapped libraries in an address space. Stores a value
/// for each mapping, and allows efficient lookup of that value based on
/// an address.
#[derive(Debug, Clone)]
pub struct LibMappings<T> {
    sorted_mappings: Vec<Mapping<T>>,
}

impl<T> Default for LibMappings<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LibMappings<T> {
    /// Creates a new empty instance.
    pub fn new() -> Self {
        Self {
            sorted_mappings: Vec::new(),
        }
    }

    /// Add a mapping to this address space. Any existing mappings which overlap with the
    /// new mapping are removed.
    ///
    /// `start_avma` and `end_avma` describe the address range that this mapping
    /// occupies.
    ///
    /// AVMA = "actual virtual memory address"
    ///
    /// `relative_address_at_start` is the "relative address" which corresponds
    /// to `start_avma`, in the library that is mapped in this mapping. This is zero if
    /// `start_avm` is the base address of the library.
    ///
    /// A relative address is a `u32` value which is relative to the library base address.
    /// So you will usually set `relative_address_at_start` to `start_avma - base_avma`.
    ///
    /// For ELF binaries, the base address is the AVMA of the first segment, i.e. the
    /// start_avma of the mapping created by the first ELF `LOAD` command.
    ///
    /// For mach-O binaries, the base address is the vmaddr of the `__TEXT` segment.
    ///
    /// For Windows binaries, the base address is the image load address.
    pub fn add_mapping(
        &mut self,
        start_avma: u64,
        end_avma: u64,
        relative_address_at_start: u32,
        value: T,
    ) {
        let remove_range_begin = match self
            .sorted_mappings
            .binary_search_by_key(&start_avma, |r| r.start_avma)
        {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => {
                // start_avma falls between the start_avmas of `i - 1` and `i`.
                if start_avma < self.sorted_mappings[i - 1].end_avma {
                    i - 1
                } else {
                    i
                }
            }
        };

        let mut remove_range_end = remove_range_begin;
        for mapping in &self.sorted_mappings[remove_range_begin..] {
            if mapping.start_avma < end_avma {
                remove_range_end += 1;
            } else {
                break;
            }
        }

        self.sorted_mappings.splice(
            remove_range_begin..remove_range_end,
            [Mapping {
                start_avma,
                end_avma,
                relative_address_at_start,
                value,
            }],
        );
    }

    /// Remove a mapping which starts at the given address. If found, this returns
    /// the `relative_address_at_start` and the associated value of the mapping.
    pub fn remove_mapping(&mut self, start_avma: u64) -> Option<(u32, T)> {
        self.sorted_mappings
            .binary_search_by_key(&start_avma, |m| m.start_avma)
            .ok()
            .map(|i| self.sorted_mappings.remove(i))
            .map(|m| (m.relative_address_at_start, m.value))
    }

    /// Clear all mappings.
    pub fn clear(&mut self) {
        self.sorted_mappings.clear();
        self.sorted_mappings.shrink_to_fit();
    }

    /// Look up the mapping which covers the given address.
    fn lookup(&self, avma: u64) -> Option<&Mapping<T>> {
        let mappings = &self.sorted_mappings[..];
        let index = match mappings.binary_search_by_key(&avma, |r| r.start_avma) {
            Err(0) => return None,
            Ok(exact_match) => exact_match,
            Err(insertion_index) => {
                let mapping_index = insertion_index - 1;
                if avma < mappings[mapping_index].end_avma {
                    mapping_index
                } else {
                    return None;
                }
            }
        };
        Some(&mappings[index])
    }

    /// Converts an absolute address (AVMA, actual virtual memory address) into
    /// a relative address and the mapping's associated value.
    pub fn convert_address(&self, avma: u64) -> Option<(u32, &T)> {
        let mapping = match self.lookup(avma) {
            Some(mapping) => mapping,
            None => return None,
        };
        let offset_from_mapping_start = (avma - mapping.start_avma) as u32;
        let relative_address = mapping.relative_address_at_start + offset_from_mapping_start;
        Some((relative_address, &mapping.value))
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
struct Mapping<T> {
    start_avma: u64,
    end_avma: u64,
    relative_address_at_start: u32,
    value: T,
}
