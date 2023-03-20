use crate::binary_image::BinaryImageInner;
use crate::error::Error;
use crate::shared::{FileAndPathHelper, FileContents, FileContentsWrapper, RangeReadRef};
use crate::symbol_map::{
    GenericSymbolMap, SymbolMap, SymbolMapDataMidTrait, SymbolMapDataOuterTrait,
};
use crate::symbol_map_object::{FunctionAddressesComputer, ObjectSymbolMapDataMid};
use crate::{debug_id_for_object, BinaryImage, FileLocation, MultiArchDisambiguator};
use debugid::DebugId;
use macho_unwind_info::UnwindInfo;
use object::macho::{self, FatHeader, LinkeditDataCommand, MachHeader32, MachHeader64};
use object::read::macho::{FatArch, LoadCommandIterator, MachHeader};
use object::read::{File, Object, ObjectSection};
use object::{Endianness, FileKind, ReadRef};
use std::marker::PhantomData;
use uuid::Uuid;

/// Converts a cpu type/subtype pair into the architecture name.
///
/// For example, this converts `CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64E` to `Some("arm64e")`.
fn macho_arch_name_for_cpu_type(cputype: u32, cpusubtype: u32) -> Option<&'static str> {
    use object::macho::*;
    let s = match (cputype, cpusubtype) {
        (CPU_TYPE_X86, _) => "i386",
        (CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_H) => "x86_64h",
        (CPU_TYPE_X86_64, _) => "x86_64",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64E) => "arm64e",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_V8) => "arm64v8",
        (CPU_TYPE_ARM64, _) => "arm64",
        (CPU_TYPE_ARM64_32, CPU_SUBTYPE_ARM64_32_V8) => "arm64_32v8",
        (CPU_TYPE_ARM64_32, _) => "arm64_32",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V5TEJ) => "armv5",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V6) => "armv6",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V6M) => "armv6m",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7) => "armv7",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7F) => "armv7f",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7S) => "armv7s",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7K) => "armv7k",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7M) => "armv7m",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7EM) => "armv7em",
        (CPU_TYPE_ARM, _) => "arm",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_ALL) => "ppc",
        (CPU_TYPE_POWERPC64, CPU_SUBTYPE_POWERPC_ALL) => "ppc64",
        _ => return None,
    };
    Some(s)
}

/// Returns the (offset, size, arch) in the fat binary file for the object that matches
/// `disambiguator`, if found.
///
/// If `disambiguator` is `None`, this will always return [`Error::NoDisambiguatorForFatArchive`].
pub fn get_fat_archive_member(
    file_contents: &FileContentsWrapper<impl FileContents>,
    archive_kind: FileKind,
    disambiguator: Option<MultiArchDisambiguator>,
) -> Result<FatArchiveMember, Error> {
    let mut members = get_fat_archive_members(file_contents, archive_kind)?;

    if members.is_empty() {
        return Err(Error::EmptyFatArchive);
    }

    if members.len() == 1 && disambiguator.is_none() {
        return Ok(members.remove(0));
    }

    let disambiguator = match disambiguator {
        Some(disambiguator) => disambiguator,
        None => return Err(Error::NoDisambiguatorForFatArchive(members)),
    };

    match members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            m.match_score_for_disambiguator(&disambiguator)
                .map(|score| (i, score))
        })
        .min_by_key(|(_i, score)| *score)
    {
        Some((i, _score)) => Ok(members.remove(i)),
        None => Err(Error::NoMatchMultiArch(members)),
    }
}

pub fn get_fat_archive_members_impl<FC: FileContents, FA: FatArch>(
    file_contents: &FileContentsWrapper<FC>,
    arches: &[FA],
) -> Result<Vec<FatArchiveMember>, Error> {
    let mut members = Vec::new();

    for fat_arch in arches {
        let (cputype, cpusubtype) = (fat_arch.cputype(), fat_arch.cpusubtype());
        let arch = macho_arch_name_for_cpu_type(cputype, cpusubtype).map(ToString::to_string);
        let (start, size) = fat_arch.file_range();
        let file =
            File::parse(file_contents.range(start, size)).map_err(Error::MachOHeaderParseError)?;
        let uuid = file.mach_uuid().ok().flatten().map(Uuid::from_bytes);
        members.push(FatArchiveMember {
            offset_and_size: (start, size),
            cputype,
            cpusubtype,
            arch,
            uuid,
        });
    }

    Ok(members)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatArchiveMember {
    pub offset_and_size: (u64, u64),
    pub cputype: u32,
    pub cpusubtype: u32,
    pub arch: Option<String>,
    pub uuid: Option<Uuid>,
}

impl FatArchiveMember {
    /// Returns `None` if it doesn't match.
    /// Returns `Some(_)` if there is a match, and lower values are better.
    pub fn match_score_for_disambiguator(
        &self,
        disambiguator: &MultiArchDisambiguator,
    ) -> Option<usize> {
        match disambiguator {
            MultiArchDisambiguator::Arch(expected_arch) => {
                if self.arch.as_deref() == Some(expected_arch) {
                    Some(0)
                } else {
                    None
                }
            }
            MultiArchDisambiguator::BestMatch(expected_archs) => {
                if let Some(arch) = self.arch.as_deref() {
                    expected_archs.iter().position(|ea| ea == arch)
                } else {
                    None
                }
            }
            MultiArchDisambiguator::BestMatchForNative => {
                if let Some(arch) = self.arch.as_deref() {
                    #[cfg(target_arch = "x86_64")]
                    match arch {
                        "x86_64h" => Some(0),
                        "x86_64" => Some(1),
                        _ => None,
                    }
                    #[cfg(target_arch = "aarch64")]
                    match arch {
                        "arm64e" => Some(0),
                        "arm64" => Some(1),
                        _ => None,
                    }
                    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                    None
                } else {
                    None
                }
            }
            MultiArchDisambiguator::DebugId(expected_debug_id) => {
                if self.uuid.map(DebugId::from_uuid) == Some(*expected_debug_id) {
                    Some(0)
                } else {
                    None
                }
            }
        }
    }
}

pub fn get_fat_archive_members(
    file_contents: &FileContentsWrapper<impl FileContents>,
    archive_kind: FileKind,
) -> Result<Vec<FatArchiveMember>, Error> {
    if archive_kind == FileKind::MachOFat64 {
        let arches = FatHeader::parse_arch64(file_contents)
            .map_err(|e| Error::ObjectParseError(archive_kind, e))?;
        get_fat_archive_members_impl(file_contents, arches)
    } else {
        let arches = FatHeader::parse_arch32(file_contents)
            .map_err(|e| Error::ObjectParseError(archive_kind, e))?;
        get_fat_archive_members_impl(file_contents, arches)
    }
}

struct DyldCacheLoader<'a, 'h, H>
where
    H: FileAndPathHelper<'h>,
{
    helper: &'h H,
    dyld_cache_path: &'a H::FL,
}

impl<'a, 'h, H, F> DyldCacheLoader<'a, 'h, H>
where
    H: FileAndPathHelper<'h, F = F>,
{
    pub fn new(helper: &'h H, dyld_cache_path: &'a H::FL) -> Self {
        Self {
            helper,
            dyld_cache_path,
        }
    }

    pub async fn load_cache(&self) -> Result<F, Error> {
        self.helper
            .load_file(self.dyld_cache_path.clone())
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(self.dyld_cache_path.to_string(), e))
    }

    pub async fn load_subcache(&self, suffix: &str) -> Result<F, Error> {
        let subcache_location = self
            .dyld_cache_path
            .location_for_dyld_subcache(suffix)
            .ok_or(Error::FileLocationRefusedSubcacheLocation)?;
        self.helper
            .load_file(subcache_location)
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(self.dyld_cache_path.to_string(), e))
    }
}

async fn load_file_data_for_dyld_cache<'h, H, F>(
    dyld_cache_path: H::FL,
    dylib_path: String,
    helper: &'h H,
) -> Result<DyldCacheFileData<F>, Error>
where
    H: FileAndPathHelper<'h, F = F>,
    F: FileContents + 'static,
{
    let dcl = DyldCacheLoader::new(helper, &dyld_cache_path);
    let root_contents = dcl.load_cache().await?;
    let root_contents = FileContentsWrapper::new(root_contents);

    let mut subcache_contents = Vec::new();
    for subcache_index in 1.. {
        // Find the subcache at dyld_shared_cache_arm64e.1 or dyld_shared_cache_arm64e.01
        let suffix = format!(".{subcache_index}");
        let suffix2 = format!(".{subcache_index:02}");
        let subcache = match dcl.load_subcache(&suffix).await {
            Ok(subcache) => subcache,
            Err(_) => match dcl.load_subcache(&suffix2).await {
                Ok(subcache) => subcache,
                Err(_) => break,
            },
        };
        subcache_contents.push(FileContentsWrapper::new(subcache));
    }
    if let Ok(subcache) = dcl.load_subcache(".symbols").await {
        subcache_contents.push(FileContentsWrapper::new(subcache));
    };

    Ok(DyldCacheFileData::new(
        root_contents,
        subcache_contents,
        dylib_path,
    ))
}

pub async fn load_symbol_map_for_dyld_cache<'h, H, FL>(
    dyld_cache_path: H::FL,
    dylib_path: String,
    helper: &'h H,
) -> Result<SymbolMap<FL>, Error>
where
    H: FileAndPathHelper<'h, FL = FL>,
    FL: FileLocation,
{
    let owner = load_file_data_for_dyld_cache(dyld_cache_path.clone(), dylib_path, helper).await?;
    let symbol_map = GenericSymbolMap::new(owner)?;
    Ok(SymbolMap::new(dyld_cache_path, Box::new(symbol_map)))
}

pub struct DyldCacheFileData<T>
where
    T: FileContents + 'static,
{
    pub(crate) root_file_data: FileContentsWrapper<T>,
    pub(crate) subcache_file_data: Vec<FileContentsWrapper<T>>,
    pub(crate) dylib_path: String,
}

pub struct ObjectAndMachOData<'data, T: FileContents + 'static> {
    pub object: File<'data, RangeReadRef<'data, &'data FileContentsWrapper<T>>>,
    pub macho_data: MachOData<'data, RangeReadRef<'data, &'data FileContentsWrapper<T>>>,
}

impl<T: FileContents + 'static> DyldCacheFileData<T> {
    pub fn new(
        root_file_data: FileContentsWrapper<T>,
        subcache_file_data: Vec<FileContentsWrapper<T>>,
        dylib_path: String,
    ) -> Self {
        Self {
            root_file_data,
            subcache_file_data,
            dylib_path,
        }
    }

    pub fn make_object(&self) -> Result<ObjectAndMachOData<'_, T>, Error> {
        let rootcache_range = self.root_file_data.full_range();
        let subcache_ranges: Vec<_> = self
            .subcache_file_data
            .iter()
            .map(FileContentsWrapper::full_range)
            .collect();
        let cache = object::read::macho::DyldCache::<Endianness, _>::parse(
            rootcache_range,
            &subcache_ranges,
        )
        .map_err(Error::DyldCacheParseError)?;

        let image = match cache
            .images()
            .find(|image| image.path() == Ok(&self.dylib_path))
        {
            Some(image) => image,
            None => return Err(Error::NoMatchingDyldCacheImagePath(self.dylib_path.clone())),
        };
        let object = image.parse_object().map_err(Error::MachOHeaderParseError)?;
        let (data, header_offset) = image
            .image_data_and_offset()
            .map_err(Error::MachOHeaderParseError)?;
        let macho_data = MachOData::new(data, header_offset, object.is_64());
        Ok(ObjectAndMachOData { object, macho_data })
    }
}

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for DyldCacheFileData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let ObjectAndMachOData { object, macho_data } = self.make_object()?;
        let arch = macho_data.get_arch();
        let function_addresses_computer = MachOFunctionAddressesComputer { macho_data };
        let debug_id = debug_id_for_object(&object)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;
        let object = ObjectSymbolMapDataMid::new(
            object,
            None,
            function_addresses_computer,
            self.root_file_data.full_range(),
            None,
            arch,
            debug_id,
        );

        Ok(Box::new(object))
    }
}

pub fn get_symbol_map_for_macho<F: FileContents + 'static, FL: FileLocation>(
    debug_file_location: FL,
    file_contents: FileContentsWrapper<F>,
) -> Result<SymbolMap<FL>, Error> {
    let owner = MachSymbolMapData::new(file_contents);
    let symbol_map = GenericSymbolMap::new(owner)?;
    Ok(SymbolMap::new(debug_file_location, Box::new(symbol_map)))
}

pub fn get_symbol_map_for_fat_archive_member<F: FileContents + 'static, FL: FileLocation>(
    debug_file_location: FL,
    file_contents: FileContentsWrapper<F>,
    member: FatArchiveMember,
) -> Result<SymbolMap<FL>, Error> {
    let (start_offset, range_size) = member.offset_and_size;
    let owner =
        MachOFatArchiveMemberData::new(file_contents, start_offset, range_size, member.arch);
    let symbol_map = GenericSymbolMap::new(owner)?;
    Ok(SymbolMap::new(debug_file_location, Box::new(symbol_map)))
}

struct MachSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
}

impl<T: FileContents> MachSymbolMapData<T> {
    pub fn new(file_data: FileContentsWrapper<T>) -> Self {
        Self { file_data }
    }
}

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for MachSymbolMapData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let macho_file = File::parse(&self.file_data).map_err(Error::MachOHeaderParseError)?;
        let macho_data = MachOData::new(&self.file_data, 0, macho_file.is_64());
        let arch = macho_data.get_arch();
        let function_addresses_computer = MachOFunctionAddressesComputer { macho_data };
        let debug_id = debug_id_for_object(&macho_file)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;
        let object = ObjectSymbolMapDataMid::new(
            macho_file,
            None,
            function_addresses_computer,
            &self.file_data,
            None,
            arch,
            debug_id,
        );
        Ok(Box::new(object))
    }
}

pub struct MachOFatArchiveMemberData<T>
where
    T: FileContents,
{
    pub(crate) file_data: FileContentsWrapper<T>,
    pub(crate) start_offset: u64,
    pub(crate) range_size: u64,
    pub(crate) arch: Option<String>,
}

impl<T: FileContents> MachOFatArchiveMemberData<T> {
    pub fn new(
        file_data: FileContentsWrapper<T>,
        start_offset: u64,
        range_size: u64,
        arch: Option<String>,
    ) -> Self {
        Self {
            file_data,
            start_offset,
            range_size,
            arch,
        }
    }

    pub fn data(&self) -> RangeReadRef<&'_ FileContentsWrapper<T>> {
        let file_contents_ref = &self.file_data;
        file_contents_ref.range(self.start_offset, self.range_size)
    }
}

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for MachOFatArchiveMemberData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let range_data = self.data();
        let macho_file = File::parse(range_data).map_err(Error::MachOHeaderParseError)?;
        let macho_data = MachOData::new(range_data, 0, macho_file.is_64());
        let arch = macho_data.get_arch();
        let function_addresses_computer = MachOFunctionAddressesComputer { macho_data };
        let debug_id = debug_id_for_object(&macho_file)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;
        let object = ObjectSymbolMapDataMid::new(
            macho_file,
            None,
            function_addresses_computer,
            range_data,
            None,
            arch,
            debug_id,
        );
        Ok(Box::new(object))
    }
}

pub async fn load_binary_from_dyld_cache<'h, F, H>(
    dyld_cache_path: H::FL,
    dylib_path: String,
    helper: &'h H,
) -> Result<BinaryImage<F>, Error>
where
    F: FileContents + 'static,
    H: FileAndPathHelper<'h, F = F>,
{
    let file_data =
        load_file_data_for_dyld_cache(dyld_cache_path, dylib_path.clone(), helper).await?;
    let inner = BinaryImageInner::MemberOfDyldSharedCache(file_data);
    let name = match dylib_path.rfind('/') {
        Some(index) => dylib_path[index + 1..].to_owned(),
        None => dylib_path.to_owned(),
    };
    let image = BinaryImage::new(inner, Some(name), Some(dylib_path))?;
    Ok(image)
}

struct MachOFunctionAddressesComputer<'data, R: ReadRef<'data>> {
    macho_data: MachOData<'data, R>,
}

impl<'data, R: ReadRef<'data>> FunctionAddressesComputer<'data>
    for MachOFunctionAddressesComputer<'data, R>
{
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>,
    {
        // Get function start addresses from LC_FUNCTION_STARTS
        let mut function_starts = self.macho_data.get_function_starts().ok().flatten();

        // and from __unwind_info.
        if let Some(unwind_info) = object_file
            .section_by_name_bytes(b"__unwind_info")
            .and_then(|s| s.data().ok())
            .and_then(|d| UnwindInfo::parse(d).ok())
        {
            let function_starts = function_starts.get_or_insert_with(Vec::new);
            let mut iter = unwind_info.functions();
            while let Ok(Some(function)) = iter.next() {
                function_starts.push(function.start_address);
            }
        }

        (function_starts, None)
    }
}

pub struct MachOData<'data, R: ReadRef<'data>> {
    data: R,
    header_offset: u64,
    is_64: bool,
    _phantom: PhantomData<&'data ()>,
}

impl<'data, R: ReadRef<'data>> MachOData<'data, R> {
    pub fn new(data: R, header_offset: u64, is_64: bool) -> Self {
        Self {
            data,
            header_offset,
            is_64,
            _phantom: PhantomData,
        }
    }

    /// Read the list of function start addresses from the LC_FUNCTION_STARTS mach-O load command.
    /// This information is usually present even in stripped binaries. It's a uleb128 encoded list
    /// of deltas between the function addresses, with a zero delta terminator.
    /// We use this information to improve symbolication for stripped binaries: It allows us to
    /// group addresses from the same function into the same (synthesized) "symbol". It also allows
    /// better results for binaries with partial symbol tables, because it tells us where the
    /// functions with symbols end. This means that those symbols don't "overreach" to cover
    /// addresses after their function - instead, they get correctly terminated by a symbol-less
    /// function's start address.
    pub fn get_function_starts(&self) -> Result<Option<Vec<u32>>, Error> {
        let data = self
            .function_start_data()
            .map_err(Error::MachOHeaderParseError)?;
        let data = if let Some(data) = data {
            data
        } else {
            return Ok(None);
        };
        let mut function_starts = Vec::new();
        let mut prev_address = 0;
        let mut bytes = data;
        while let Some((delta, rest)) = read_uleb128(bytes) {
            if delta == 0 {
                break;
            }
            bytes = rest;
            let address = prev_address + delta;
            function_starts.push(address as u32);
            prev_address = address;
        }

        Ok(Some(function_starts))
    }

    pub fn get_arch(&self) -> Option<&'static str> {
        if self.is_64 {
            self.get_arch_impl::<MachHeader64<Endianness>>()
        } else {
            self.get_arch_impl::<MachHeader32<Endianness>>()
        }
    }

    fn get_arch_impl<M: MachHeader>(&self) -> Option<&'static str> {
        let header = M::parse(self.data, self.header_offset).ok()?;
        let endian = header.endian().ok()?;
        macho_arch_name_for_cpu_type(header.cputype(endian), header.cpusubtype(endian))
    }

    fn load_command_iter<M: MachHeader>(
        &self,
    ) -> object::read::Result<(M::Endian, LoadCommandIterator<M::Endian>)> {
        let header = M::parse(self.data, self.header_offset)?;
        let endian = header.endian()?;
        let load_commands = header.load_commands(endian, self.data, self.header_offset)?;
        Ok((endian, load_commands))
    }

    fn function_start_data(&self) -> object::read::Result<Option<&'data [u8]>> {
        let (endian, mut commands) = if self.is_64 {
            self.load_command_iter::<MachHeader64<Endianness>>()?
        } else {
            self.load_command_iter::<MachHeader32<Endianness>>()?
        };
        while let Ok(Some(command)) = commands.next() {
            if command.cmd() == macho::LC_FUNCTION_STARTS {
                let command: &LinkeditDataCommand<_> = command.data()?;
                let dataoff: u64 = command.dataoff.get(endian).into();
                let datasize: u64 = command.datasize.get(endian).into();
                let data = self.data.read_bytes_at(dataoff, datasize).ok();
                return Ok(data);
            }
        }
        Ok(None)
    }
}

fn read_uleb128(mut bytes: &[u8]) -> Option<(u64, &[u8])> {
    const CONTINUATION_BIT: u8 = 1 << 7;

    let mut result = 0;
    let mut shift = 0;

    while !bytes.is_empty() {
        let byte = bytes[0];
        bytes = &bytes[1..];
        if shift == 63 && byte != 0x00 && byte != 0x01 {
            return None;
        }

        let low_bits = u64::from(byte & !CONTINUATION_BIT);
        result |= low_bits << shift;

        if byte & CONTINUATION_BIT == 0 {
            return Some((result, bytes));
        }

        shift += 7;
    }
    None
}
