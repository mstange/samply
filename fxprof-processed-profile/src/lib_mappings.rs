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
    pub fn new() -> Self {
        Self {
            sorted_mappings: Vec::new(),
        }
    }

    /// Add a mapping to this process.
    ///
    /// `start_avma..end_avma` describe the address mapping that this mapping
    /// occupies in the virtual memory address space of the process.
    /// AVMA = "actual virtual memory address"
    ///
    /// `relative_address_at_start` is the "relative address" which corresponds
    /// to `start_avma`, in the library that is mapped in this mapping. A relative
    /// address is a `u32` value which is relative to the library base address.
    /// So you will usually set `relative_address_at_start` to `start_avma - base_avma`.
    ///
    /// For ELF binaries, the base address is AVMA of the first segment, i.e. the
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
        let insertion_index = match self
            .sorted_mappings
            .binary_search_by_key(&start_avma, |r| r.start_avma)
        {
            Ok(i) => {
                // We already have a library mapping at this address.
                // Not sure how to best deal with it. Ideally it wouldn't happen. Let's just remove this mapping.
                self.sorted_mappings.remove(i);
                i
            }
            Err(i) => i,
        };

        self.sorted_mappings.insert(
            insertion_index,
            Mapping {
                start_avma,
                end_avma,
                relative_address_at_start,
                value,
            },
        );
    }

    pub fn remove_mapping(&mut self, start_avma: u64) -> Option<(u32, T)> {
        self.sorted_mappings
            .binary_search_by_key(&start_avma, |m| m.start_avma)
            .ok()
            .map(|i| self.sorted_mappings.remove(i))
            .map(|m| (m.relative_address_at_start, m.value))
    }

    pub fn clear(&mut self) {
        self.sorted_mappings.clear();
        self.sorted_mappings.shrink_to_fit();
    }

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
