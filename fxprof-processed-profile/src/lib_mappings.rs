use std::collections::BTreeMap;

/// Keeps track of mapped libraries in an address space. Stores a value
/// for each mapping, and allows efficient lookup of that value based on
/// an address.
///
/// A "library" here is a loose term; it could be a normal shared library,
/// or the main binary, but it could also be a synthetic library for JIT
/// code. For normal libraries, there's usually just one mapping per library.
/// For JIT code, you could have many small mappings, one per JIT function,
/// all pointing to the synthetic JIT "library".
#[derive(Debug, Clone)]
pub struct LibMappings<T> {
    /// A BTreeMap of non-overlapping Mappings. The key is the start_avma of the mapping.
    ///
    /// When a new mapping is added, overlapping mappings are removed.
    map: BTreeMap<u64, Mapping<T>>,
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
            map: BTreeMap::new(),
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
        let removal_avma_range_start =
            if let Some(mapping_overlapping_with_start_avma) = self.lookup_impl(start_avma) {
                mapping_overlapping_with_start_avma.start_avma
            } else {
                start_avma
            };
        // self.map.drain(removal_avma_range_start..end_avma);
        let overlapping_keys: Vec<u64> = self
            .map
            .range(removal_avma_range_start..end_avma)
            .map(|(start_avma, _)| *start_avma)
            .collect();
        for key in overlapping_keys {
            self.map.remove(&key);
        }

        self.map.insert(
            start_avma,
            Mapping {
                start_avma,
                end_avma,
                relative_address_at_start,
                value,
            },
        );
    }

    /// Remove a mapping which starts at the given address. If found, this returns
    /// the `relative_address_at_start` and the associated value of the mapping.
    pub fn remove_mapping(&mut self, start_avma: u64) -> Option<(u32, T)> {
        self.map
            .remove(&start_avma)
            .map(|m| (m.relative_address_at_start, m.value))
    }

    /// Clear all mappings.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Look up the mapping which covers the given address and return
    /// the stored value.
    pub fn lookup(&self, avma: u64) -> Option<&T> {
        self.lookup_impl(avma).map(|m| &m.value)
    }

    /// Look up the mapping which covers the given address and return
    /// its `Mapping<T>``.
    fn lookup_impl(&self, avma: u64) -> Option<&Mapping<T>> {
        let (_start_avma, last_mapping_starting_at_or_before_avma) =
            self.map.range(..=avma).next_back()?;
        if avma < last_mapping_starting_at_or_before_avma.end_avma {
            Some(last_mapping_starting_at_or_before_avma)
        } else {
            None
        }
    }

    /// Converts an absolute address (AVMA, actual virtual memory address) into
    /// a relative address and the mapping's associated value.
    pub fn convert_address(&self, avma: u64) -> Option<(u32, &T)> {
        let mapping = self.lookup_impl(avma)?;
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_lib_mappings() {
        let mut m = LibMappings::new();
        m.add_mapping(100, 200, 100, "100..200");
        m.add_mapping(200, 250, 200, "200..250");
        assert_eq!(m.lookup(200), Some(&"200..250"));
        m.add_mapping(180, 220, 180, "180..220");
        assert_eq!(m.lookup(200), Some(&"180..220"));
        assert_eq!(m.lookup(170), None);
        assert_eq!(m.lookup(220), None);
        m.add_mapping(225, 250, 225, "225..250");
        m.add_mapping(255, 270, 255, "255..270");
        m.add_mapping(100, 150, 100, "100..150");
        assert_eq!(m.lookup(90), None);
        assert_eq!(m.lookup(150), None);
        assert_eq!(m.lookup(149), Some(&"100..150"));
        assert_eq!(m.lookup(200), Some(&"180..220"));
        assert_eq!(m.lookup(260), Some(&"255..270"));
    }
}
