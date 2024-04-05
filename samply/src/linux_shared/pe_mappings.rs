//! This is a terrible hack to get binary correlation working with apps on Wine.
//!
//! Unlike ELF, PE has the notion of "file alignment" that is different from page alignment.
//! Hence, even if the virtual address is page aligned, its on-disk offset may not be. This
//! leads to obvious trouble with using mmap, since mmap requires the file offset to be page
//! aligned. Wine's workaround is straightforward: for misaligned sections, Wine will simply
//! copy the image from disk instead of mmapping them. For example, `/proc/<pid>/maps` can look
//! like this:
//!
//! ```plain
//! <PE header> 140000000-140001000 r--p 00000000 00:25 272185   game.exe
//! <.text>     140001000-143be8000 r-xp 00000000 00:00 0
//!             143be8000-144c0c000 r--p 00000000 00:00 0
//! ```
//!
//! When this misalignment happens, most of the sections in the memory will not be a file
//! mapping. However, the PE header is always mapped, and it resides at the beginning of the
//! file, which means it's also always *aligned*. Finally, it's always mapped first, because
//! the information from the header is required to determine the load address of the other
//! sections. Hence, if we find a mapping that seems to pointing to a PE file, and has a file
//! offset of 0, we'll add it to the list of "suspected PE images". When we see a later mapping
//! that belongs to one of the suspected PE ranges, we'll match the mapping with the file,
//! which allows binary correlation and unwinding to work.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use object::pe::{ImageNtHeaders32, ImageNtHeaders64};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile};
use object::FileKind;
use wholesym::{CodeId, PeCodeId};

use super::avma_range::AvmaRange;

/// See [`Converter::check_for_pe_mapping`].
#[derive(Debug, Clone)]
pub struct SuspectedPeMapping {
    pub path: PathBuf,
    pub code_id: CodeId,
    pub avma_range: AvmaRange,
}

pub struct PeMappings {
    /// Mapping of start address to potential mapped PE binaries.
    /// The key is equal to the start field of the value.
    suspected_pe_mappings: BTreeMap<u64, SuspectedPeMapping>,
}

impl PeMappings {
    pub fn new() -> Self {
        Self {
            suspected_pe_mappings: BTreeMap::new(),
        }
    }

    pub fn find_mapping(&self, avma_range: &AvmaRange) -> Option<&SuspectedPeMapping> {
        let (_, mapping) = self
            .suspected_pe_mappings
            .range(..=avma_range.start())
            .next_back()?;
        if !avma_range.encompasses(&mapping.avma_range) {
            return None;
        }
        Some(mapping)
    }

    pub fn check_mmap(&mut self, path_slice: &[u8], mapping_start_avma: u64) {
        // Do a quick extension check first, to avoid end up trying to parse every mmapped file.
        let filename_is_pe = path_slice.ends_with(b".exe")
            || path_slice.ends_with(b".dll")
            || path_slice.ends_with(b".EXE")
            || path_slice.ends_with(b".DLL");
        if !filename_is_pe {
            return;
        }

        let Ok(path) = std::str::from_utf8(path_slice) else {
            return;
        };
        let path = Path::new(path);

        // There are a few assumptions here:
        // - The SizeOfImage field in the PE header is defined to be a multiple of SectionAlignment.
        //   SectionAlignment is usually the page size. When it's not the page size, additional
        //   layout restrictions apply and Wine will always map the file in its entirety, which
        //   means we're safe without the workaround. So we can safely assume it to be page aligned
        //   here.
        // - VirtualAddress of the sections are defined to be adjacent after page-alignment. This
        //   means that we can treat the image as a contiguous region.
        if let Some((size, code_id)) = get_pe_mapping_size_and_codeid(path) {
            let mapping = SuspectedPeMapping {
                path: path.to_owned(),
                code_id,
                avma_range: AvmaRange::with_start_size(mapping_start_avma, size),
            };
            self.suspected_pe_mappings
                .insert(mapping_start_avma, mapping);
        }
    }
}

fn get_pe_mapping_size_and_codeid(path: &Path) -> Option<(u64, CodeId)> {
    fn inner<T: ImageNtHeaders>(data: &[u8]) -> Option<(u64, CodeId)> {
        let file = PeFile::<T>::parse(data).ok()?;
        let image_size = file.nt_headers().optional_header().size_of_image();
        let timestamp = file
            .nt_headers()
            .file_header()
            .time_date_stamp
            .get(object::LittleEndian);
        let code_id = CodeId::PeCodeId(PeCodeId {
            timestamp,
            image_size,
        });

        Some((image_size as u64, code_id))
    }

    let file = std::fs::File::open(path).ok()?;
    let mmap = unsafe { Mmap::map(&file).ok()? };

    match FileKind::parse(&mmap[..]).ok()? {
        FileKind::Pe32 => inner::<ImageNtHeaders32>(&mmap),
        FileKind::Pe64 => inner::<ImageNtHeaders64>(&mmap),
        _ => None,
    }
}
