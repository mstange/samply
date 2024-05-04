use std::fmt::Debug;

use object::{Object, ObjectSection, ObjectSegment, SectionKind};

// A file range in an object file, such as a segment or a section,
// for which we know the corresponding Stated Virtual Memory Address (SVMA).
#[derive(Clone)]
pub struct SvmaFileRange {
    pub svma: u64,
    pub file_offset: u64,
    pub size: u64,
}

impl SvmaFileRange {
    pub fn from_segment<'data, S: ObjectSegment<'data>>(segment: S) -> Self {
        let svma = segment.address();
        let (file_offset, size) = segment.file_range();
        SvmaFileRange {
            svma,
            file_offset,
            size,
        }
    }

    pub fn from_section<'data, S: ObjectSection<'data>>(section: S) -> Option<Self> {
        let svma = section.address();
        let (file_offset, size) = section.file_range()?;
        Some(SvmaFileRange {
            svma,
            file_offset,
            size,
        })
    }

    pub fn encompasses_file_range(&self, other_file_offset: u64, other_file_size: u64) -> bool {
        let self_file_range_end = self.file_offset + self.size;
        let other_file_range_end = other_file_offset + other_file_size;
        self.file_offset <= other_file_offset && other_file_range_end <= self_file_range_end
    }

    pub fn is_encompassed_by_file_range(
        &self,
        other_file_offset: u64,
        other_file_size: u64,
    ) -> bool {
        let self_file_range_end = self.file_offset + self.size;
        let other_file_range_end = other_file_offset + other_file_size;
        other_file_offset <= self.file_offset && self_file_range_end <= other_file_range_end
    }
}

impl Debug for SvmaFileRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvmaFileRange")
            .field("svma", &format!("{:#x}", &self.svma))
            .field("file_offset", &format!("{:#x}", &self.file_offset))
            .field("size", &format!("{:#x}", &self.size))
            .finish()
    }
}

/// Compute the bias from the stated virtual memory address (SVMA), the VMA defined in the file's
/// section table, to the actual virtual memory address (AVMA), the VMA the file is actually mapped
/// at.
///
/// We have the section + segment information of the mapped object file, and we know the file offset
/// and size of the mapping, as well as the AVMA at the mapping start.
///
/// An image is mapped into memory using ELF load commands ("segments"). Usually there are multiple
/// ELF load commands, resulting in multiple mappings. Some of these mappings will be read-only,
/// and some will be executable.
///
/// Commonly, the executable mapping is the second of four mappings.
///
/// If we know about all mappings of an image, then the base AVMA is the start AVMA of the first mapping.
/// But sometimes we only have one mapping of an image, for example only the second mapping. This
/// happens if the `perf.data` file only contains mmap information of the executable mappings. In that
/// case, we have to look at the "page offset" of that mapping and find out which part of the file
/// was mapped in that range.
///
/// In that case it's tempting to say "The AVMA is the address at which the file would start, if
/// the entire file was mapped contiguously into memory around our mapping." While this works in
/// many cases, it doesn't work if there are "SVMA gaps" between the segments which have been elided
/// in the file, i.e. it doesn't work for files where the file offset <-> SVMA translation
/// is different for each segment.
///
/// Easy case A:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:         0x0 |----------------| |-------------|
/// AVMA:   0x1750000 |----------------| |-------------|
/// ```
///
/// Easy case B:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:     0x40000 |----------------| |-------------|
/// AVMA:   0x1750000 |----------------| |-------------|
/// ```
///
/// Hard case:
/// ```plain
/// File offset:  0x0 |----------------| |-------------|
/// SVMA:         0x0 |----------------|         |-------------|
/// AVMA:   0x1750000 |----------------|         |-------------|
/// ```
///
/// One example of the hard case has been observed in `libxul.so`: The `.text` section
/// was in the second segment. In the first segment, SVMAs were equal to their
/// corresponding file offsets. In the second segment, SMAs were 0x1000 bytes higher
/// than their corresponding file offsets. In other words, there was a 0x1000-wide gap
/// between the segments in virtual address space, but this gap was omitted in the file.
/// The SVMA gap exists in the AVMAs too - the "bias" between SVMAs and AVMAs is the same
/// for all segments of an image. So we have to find the SVMA for the mapping by finding
/// a segment or section which overlaps the mapping in file offset space, and then use
/// the matching segment's / section's SVMA to find the SVMA-to-AVMA "bias" for the
/// mapped bytes.
///
/// Another interesting edge case we observed was a case where a mapping was seemingly
/// not initiated by an ELF LOAD command: Part of the d8 binary (the V8 shell) was mapped
/// into memory with a mapping that covered only a small part of the `.text` section.
/// Usually, you'd expect a section to be mapped in its entirety, but this was not the
/// case here. So the segment finding code below checks for containment both ways: Whether
/// the mapping is contained in the segment, or whether the segment is contained in the
/// mapping. We also tried a solution where we just check for overlap between the segment
/// and the mapping, but this sometimes got the wrong segment, because the mapping is
/// larger than the segment due to alignment, and can extend into other segments.
pub fn compute_vma_bias<'data, O: Object<'data>>(
    file: &O,
    mapping_start_file_offset: u64,
    mapping_start_avma: u64,
    mapping_size: u64,
) -> Option<u64> {
    let mut contributions: Vec<SvmaFileRange> =
        file.segments().map(SvmaFileRange::from_segment).collect();

    if contributions.is_empty() {
        // If no segment is found, fall back to using section information.
        // This fallback only exists for the synthetic .so files created by `perf inject --jit`
        // - those don't have LOAD commands.
        contributions = file
            .sections()
            .filter(|s| s.kind() == SectionKind::Text)
            .filter_map(SvmaFileRange::from_section)
            .collect();
    }

    compute_vma_bias_impl(
        &contributions,
        mapping_start_file_offset,
        mapping_start_avma,
        mapping_size,
    )
}

fn compute_vma_bias_impl(
    contributions: &[SvmaFileRange],
    mapping_file_offset: u64,
    mapping_avma: u64,
    mapping_size: u64,
) -> Option<u64> {
    // Find a contribution which either fully contains the mapping, or which is fully contained by the mapping.
    // Linux perf simply always uses the .text section as the reference contribution.
    let ref_contribution = if let Some(contribution) = contributions.iter().find(|contribution| {
        contribution.encompasses_file_range(mapping_file_offset, mapping_size)
            || contribution.is_encompassed_by_file_range(mapping_file_offset, mapping_size)
    }) {
        contribution
    } else {
        println!(
            "Could not find segment or section overlapping the file offset range 0x{:x}..0x{:x}",
            mapping_file_offset,
            mapping_file_offset + mapping_size,
        );
        return None;
    };

    // Compute the AVMA at which the reference contribution is located in process memory.
    let ref_avma = if ref_contribution.file_offset > mapping_file_offset {
        mapping_avma + (ref_contribution.file_offset - mapping_file_offset)
    } else {
        mapping_avma - (mapping_file_offset - ref_contribution.file_offset)
    };

    // We have everything we need now.
    let bias = ref_avma.wrapping_sub(ref_contribution.svma);
    Some(bias)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_compute_base_avma_impl() {
        // From a local build of the Spidermonkey shell ("js")
        let js_segments = &[
            SvmaFileRange {
                svma: 0x0,
                file_offset: 0x0,
                size: 0x14bd0bc,
            },
            SvmaFileRange {
                svma: 0x14be0c0,
                file_offset: 0x14bd0c0,
                size: 0xf5bf60,
            },
            SvmaFileRange {
                svma: 0x241b020,
                file_offset: 0x2419020,
                size: 0x08e920,
            },
            SvmaFileRange {
                svma: 0x24aa940,
                file_offset: 0x24a7940,
                size: 0x002d48,
            },
        ];
        assert_eq!(
            compute_vma_bias_impl(js_segments, 0x14bd0c0, 0x100014be0c0, 0xf5bf60),
            Some(0x10000000000)
        );
        assert_eq!(
            compute_vma_bias_impl(js_segments, 0x14bd000, 0x55d605384000, 0xf5d000),
            Some(0x55d603ec6000)
        );

        // From a local build of the V8 shell ("d8")
        let d8_segments = &[
            SvmaFileRange {
                svma: 0x0,
                file_offset: 0x0,
                size: 0x3c8ed8,
            },
            SvmaFileRange {
                svma: 0x03ca000,
                file_offset: 0x3c9000,
                size: 0xfec770,
            },
            SvmaFileRange {
                svma: 0x13b7770,
                file_offset: 0x13b5770,
                size: 0x0528d0,
            },
            SvmaFileRange {
                svma: 0x140c000,
                file_offset: 0x1409000,
                size: 0x0118f0,
            },
        ];
        assert_eq!(
            compute_vma_bias_impl(d8_segments, 0x1056000, 0x55d15fe80000, 0x180000),
            Some(0x55d15ee29000)
        );
    }
}
