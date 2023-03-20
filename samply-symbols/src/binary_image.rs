use debugid::DebugId;
use linux_perf_data::{jitdump::JitDumpHeader, linux_perf_event_reader::RawData};
use object::{
    read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile, PeFile32, PeFile64},
    FileKind, Object, ReadRef,
};

use crate::{
    debug_id_and_code_id_for_jitdump, debug_id_for_object,
    debugid_util::code_id_for_object,
    macho::{DyldCacheFileData, MachOData, MachOFatArchiveMemberData, ObjectAndMachOData},
    relative_address_base,
    shared::{FileContentsWrapper, LibraryInfo, PeCodeId, RangeReadRef},
    CodeId, ElfBuildId, Error, FileAndPathHelperError, FileContents,
};

#[derive(thiserror::Error, Debug)]
pub enum CodeByteReadingError {
    #[error("The requested address was not found in any section in the binary.")]
    AddressNotFound,

    #[error("object parse error: {0}")]
    ObjectParseError(#[from] object::Error),

    #[error("Could not read the requested address range from the section (might be out of bounds or the section might not have any bytes in the file)")]
    ByteRangeNotInSection,

    #[error("Could not read the requested address range from the file: {0}")]
    FileIO(#[from] FileAndPathHelperError),
}

pub struct BinaryImage<F: FileContents + 'static> {
    inner: BinaryImageInner<F>,
    info: LibraryInfo,
}

impl<F: FileContents + 'static> BinaryImage<F> {
    pub(crate) fn new(
        inner: BinaryImageInner<F>,
        name: Option<String>,
        path: Option<String>,
    ) -> Result<Self, Error> {
        let info = inner.make_library_info(name, path)?;
        Ok(Self { inner, info })
    }

    pub fn library_info(&self) -> LibraryInfo {
        self.info.clone()
    }

    pub fn debug_name(&self) -> Option<&str> {
        self.info.debug_name.as_deref()
    }

    pub fn debug_id(&self) -> Option<DebugId> {
        self.info.debug_id
    }

    pub fn debug_path(&self) -> Option<&str> {
        self.info.debug_path.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.info.name.as_deref()
    }

    pub fn code_id(&self) -> Option<CodeId> {
        self.info.code_id.clone()
    }

    pub fn path(&self) -> Option<&str> {
        self.info.path.as_deref()
    }

    pub fn arch(&self) -> Option<&str> {
        self.info.arch.as_deref()
    }

    pub fn make_object(
        &self,
    ) -> Option<object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>>> {
        self.inner
            .make_object()
            .expect("We already parsed this before, why is it not parsing now?")
    }

    pub fn read_bytes_at_relative_address(
        &self,
        start_address: u32,
        size: u32,
    ) -> Result<&[u8], CodeByteReadingError> {
        self.inner
            .read_bytes_at_relative_address(start_address, size)
    }
}

pub enum BinaryImageInner<F: FileContents + 'static> {
    Normal(FileContentsWrapper<F>, FileKind),
    MemberOfFatArchive(MachOFatArchiveMemberData<F>, FileKind),
    MemberOfDyldSharedCache(DyldCacheFileData<F>),
    JitDump(FileContentsWrapper<F>),
}

impl<F: FileContents> BinaryImageInner<F> {
    fn make_library_info(
        &self,
        name: Option<String>,
        path: Option<String>,
    ) -> Result<LibraryInfo, Error> {
        let (debug_id, code_id, debug_path, debug_name, arch) = match self {
            BinaryImageInner::Normal(file, file_kind) => {
                let data = file.full_range();
                let object = object::File::parse(data)
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                let debug_id = debug_id_for_object(&object);
                match file_kind {
                    FileKind::Pe32 | FileKind::Pe64 => {
                        let (code_id, debug_path, debug_name) =
                            if let Ok(pe) = PeFile64::parse(file) {
                                pe_info(&pe).into_tuple()
                            } else if let Ok(pe) = PeFile32::parse(file) {
                                pe_info(&pe).into_tuple()
                            } else {
                                (None, None, None)
                            };
                        let arch =
                            object_arch_to_string(object.architecture()).map(ToOwned::to_owned);
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                    FileKind::MachO32 | FileKind::MachO64 => {
                        let macho_data = MachOData::new(file, 0, *file_kind == FileKind::MachO64);
                        let code_id = code_id_for_object(&object);
                        let arch = macho_data.get_arch().map(ToOwned::to_owned);
                        let (debug_path, debug_name) = (path.clone(), name.clone());
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                    _ => {
                        let code_id = code_id_for_object(&object);
                        let (debug_path, debug_name) = (path.clone(), name.clone());
                        let arch =
                            object_arch_to_string(object.architecture()).map(ToOwned::to_owned);
                        (debug_id, code_id, debug_path, debug_name, arch)
                    }
                }
            }
            BinaryImageInner::MemberOfFatArchive(member, file_kind) => {
                let data = member.data();
                let object = object::File::parse(data)
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                let debug_id = debug_id_for_object(&object);
                let code_id = code_id_for_object(&object);
                let (debug_path, debug_name) = (path.clone(), name.clone());
                let arch = member.arch.clone();
                (debug_id, code_id, debug_path, debug_name, arch)
            }
            BinaryImageInner::MemberOfDyldSharedCache(dyld_cache_file_data) => {
                let ObjectAndMachOData { object, macho_data } =
                    dyld_cache_file_data.make_object()?;
                let debug_id = debug_id_for_object(&object);
                let code_id = code_id_for_object(&object);
                let (debug_path, debug_name) = (path.clone(), name.clone());
                let arch = macho_data.get_arch().map(ToOwned::to_owned);
                (debug_id, code_id, debug_path, debug_name, arch)
            }
            BinaryImageInner::JitDump(file) => {
                let header_bytes =
                    file.read_bytes_at(0, JitDumpHeader::SIZE as u64)
                        .map_err(|e| {
                            Error::HelperErrorDuringFileReading(path.clone().unwrap_or_default(), e)
                        })?;
                let header = JitDumpHeader::parse(RawData::Single(header_bytes))
                    .map_err(Error::JitDumpParsing)?;
                let (debug_id, code_id_bytes) = debug_id_and_code_id_for_jitdump(
                    header.pid,
                    header.timestamp,
                    header.elf_machine_arch,
                );
                let code_id = CodeId::ElfBuildId(ElfBuildId::from_bytes(&code_id_bytes));
                let (debug_path, debug_name) = (path.clone(), name.clone());
                let arch =
                    elf_machine_arch_to_string(header.elf_machine_arch).map(ToOwned::to_owned);
                (Some(debug_id), Some(code_id), debug_path, debug_name, arch)
            }
        };
        let info = LibraryInfo {
            debug_id,
            debug_name,
            debug_path,
            name,
            code_id,
            path,
            arch,
        };
        Ok(info)
    }

    fn make_object(
        &self,
    ) -> Result<Option<object::File<'_, RangeReadRef<'_, &'_ FileContentsWrapper<F>>>>, Error> {
        match self {
            BinaryImageInner::Normal(file, file_kind) => {
                let obj = object::File::parse(file.full_range())
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                Ok(Some(obj))
            }
            BinaryImageInner::MemberOfFatArchive(member, file_kind) => {
                let obj = object::File::parse(member.data())
                    .map_err(|e| Error::ObjectParseError(*file_kind, e))?;
                Ok(Some(obj))
            }
            BinaryImageInner::MemberOfDyldSharedCache(dyld_cache_file_data) => {
                let obj_and_macho_data = dyld_cache_file_data.make_object()?;
                Ok(Some(obj_and_macho_data.object))
            }
            BinaryImageInner::JitDump(_file) => Ok(None),
        }
    }

    pub fn read_bytes_at_relative_address(
        &self,
        start_address: u32,
        size: u32,
    ) -> Result<&[u8], CodeByteReadingError> {
        let object = match self.make_object().expect("We've succeeded before") {
            Some(obj) => obj,
            None => {
                // No object. This must be JITDUMP.
                if let BinaryImageInner::JitDump(data) = self {
                    return Ok(data.read_bytes_at(start_address.into(), size.into())?);
                } else {
                    panic!()
                }
            }
        };

        // Translate start_address from a "relative address" into an
        // SVMA ("stated virtual memory address").
        let image_base = relative_address_base(&object);
        let start_svma = image_base + u64::from(start_address);

        // Find the section and segment which contains our start_svma.
        use object::{ObjectSection, ObjectSegment};
        let (section, section_end_svma) = object
            .sections()
            .find_map(|section| {
                let section_start_svma = section.address();
                let section_end_svma = section_start_svma.checked_add(section.size())?;
                if !(section_start_svma..section_end_svma).contains(&start_svma) {
                    return None;
                }

                Some((section, section_end_svma))
            })
            .ok_or(CodeByteReadingError::AddressNotFound)?;

        let segment = object.segments().find(|segment| {
            let segment_start_svma = segment.address();
            if let Some(segment_end_svma) = segment_start_svma.checked_add(segment.size()) {
                (segment_start_svma..segment_end_svma).contains(&start_svma)
            } else {
                false
            }
        });

        let max_read_len = section_end_svma - start_svma;
        let read_len = u64::from(size).min(max_read_len);

        // Now read the instruction bytes from the file.
        let bytes = if let Some(segment) = segment {
            segment
                .data_range(start_svma, read_len)?
                .ok_or(CodeByteReadingError::ByteRangeNotInSection)?
        } else {
            // We don't have a segment, try reading via the section.
            // We hit this path with synthetic .so files created by `perf inject --jit`;
            // those only have sections, no segments (i.e. no ELF LOAD commands).
            // For regular files, we prefer to read the data via the segment, because
            // the segment is more likely to have correct file offset information.
            // Specifically, incorrect section file offset information was observed in
            // the arm64e dyld cache on macOS 13.0.1, FB11929250.
            section
                .data_range(start_svma, read_len)?
                .ok_or(CodeByteReadingError::ByteRangeNotInSection)?
        };
        Ok(bytes)
    }
}

struct PeInfo {
    code_id: CodeId,
    pdb_path: Option<String>,
    pdb_name: Option<String>,
}

impl PeInfo {
    pub fn into_tuple(self) -> (Option<CodeId>, Option<String>, Option<String>) {
        (Some(self.code_id), self.pdb_path, self.pdb_name)
    }
}

fn pe_info<'a, Pe: ImageNtHeaders, R: ReadRef<'a>>(pe: &PeFile<'a, Pe, R>) -> PeInfo {
    // The code identifier consists of the `time_date_stamp` field id the COFF header, followed by
    // the `size_of_image` field in the optional header. If the optional PE header is not present,
    // this identifier is `None`.
    let header = pe.nt_headers();
    let timestamp = header
        .file_header()
        .time_date_stamp
        .get(object::LittleEndian);
    let image_size = header.optional_header().size_of_image();
    let code_id = CodeId::PeCodeId(PeCodeId {
        timestamp,
        image_size,
    });

    let pdb_path: Option<String> = pe.pdb_info().ok().and_then(|pdb_info| {
        let pdb_path = std::str::from_utf8(pdb_info?.path()).ok()?;
        Some(pdb_path.to_string())
    });

    let pdb_name = pdb_path
        .as_deref()
        .map(|pdb_path| match pdb_path.rsplit_once(['/', '\\']) {
            Some((_base, file_name)) => file_name.to_string(),
            None => pdb_path.to_string(),
        });

    PeInfo {
        code_id,
        pdb_path,
        pdb_name,
    }
}

fn object_arch_to_string(arch: object::Architecture) -> Option<&'static str> {
    let s = match arch {
        object::Architecture::Aarch64 => "arm64",
        object::Architecture::Arm => "arm",
        object::Architecture::I386 => "x86",
        object::Architecture::X86_64 => "x86_64",
        _ => return None,
    };
    Some(s)
}

fn elf_machine_arch_to_string(elf_machine_arch: u32) -> Option<&'static str> {
    let s = match elf_machine_arch as u16 {
        object::elf::EM_ARM => "arm64",
        object::elf::EM_AARCH64 => "arm",
        object::elf::EM_386 => "x86",
        object::elf::EM_X86_64 => "x86_64",
        _ => return None,
    };
    Some(s)
}
