use std::{collections::HashMap, sync::Mutex};

use object::{read::archive::ArchiveFile, File, FileKind, ReadRef};
use yoke::{Yoke, Yokeable};

use crate::{
    dwarf::{get_frames, Addr2lineContextData},
    macho,
    path_mapper::PathMapper,
    shared::{ExternalFileAddressInFileRef, ExternalFileRef, FileContentsWrapper, RangeReadRef},
    Error, FileAndPathHelper, FileContents, FileLocation, FrameDebugInfo, MultiArchDisambiguator,
};

pub async fn load_external_file<'h, H>(
    helper: &'h H,
    original_file_location: &H::FL,
    external_file_ref: &ExternalFileRef,
) -> Result<ExternalFileSymbolMap, Error>
where
    H: FileAndPathHelper<'h>,
{
    let file = helper
        .load_file(
            original_file_location
                .location_for_external_object_file(&external_file_ref.file_name)
                .ok_or(Error::FileLocationRefusedExternalObjectLocation)?,
        )
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(external_file_ref.file_name.clone(), e))?;
    let symbol_map = ExternalFileSymbolMapImpl::new(
        &external_file_ref.file_name,
        file,
        external_file_ref.arch.as_deref(),
    )?;
    Ok(ExternalFileSymbolMap(Box::new(symbol_map)))
}

struct ExternalFileMemberContext<'a> {
    context: Option<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    symbol_addresses: HashMap<&'a [u8], u64>,
}

impl<'a> ExternalFileMemberContext<'a> {
    pub fn lookup(
        &self,
        symbol_name: &[u8],
        offset_from_symbol: u32,
        path_mapper: &mut PathMapper<()>,
    ) -> Option<Vec<FrameDebugInfo>> {
        let symbol_address = self.symbol_addresses.get(symbol_name)?;
        let address = symbol_address + offset_from_symbol as u64;
        get_frames(address, self.context.as_ref(), path_mapper)
    }
}

struct ExternalFileContext<'a, F: FileContents> {
    external_file: &'a ExternalFileData<F>,
    member_contexts: Mutex<HashMap<String, ExternalFileMemberContext<'a>>>,
    path_mapper: Mutex<PathMapper<()>>,
}

trait ExternalFileDataOuterTrait {
    #[cfg(feature = "send_futures")]
    fn make_type_erased_file_context(&self)
        -> Box<dyn ExternalFileContextTrait + '_ + Send + Sync>;
    #[cfg(not(feature = "send_futures"))]
    fn make_type_erased_file_context(&self) -> Box<dyn ExternalFileContextTrait + '_>;

    fn make_member_context<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalFileMemberContext<'s>, Error>;
    fn name(&self) -> &str;
}

trait ExternalFileContextTrait {
    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>>;
}

impl<'a, F: FileContents> ExternalFileContextTrait for ExternalFileContext<'a, F> {
    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        let member_key = external_file_address
            .name_in_archive
            .as_deref()
            .unwrap_or("");
        let mut member_contexts = self.member_contexts.lock().unwrap();
        let mut path_mapper = self.path_mapper.lock().unwrap();
        match member_contexts.get(member_key) {
            Some(member_context) => member_context.lookup(
                &external_file_address.symbol_name,
                external_file_address.offset_from_symbol,
                &mut path_mapper,
            ),
            None => {
                let member_context = self
                    .external_file
                    .make_member_context(external_file_address.name_in_archive.as_deref())
                    .ok()?;
                let res = member_context.lookup(
                    &external_file_address.symbol_name,
                    external_file_address.offset_from_symbol,
                    &mut path_mapper,
                );
                member_contexts.insert(member_key.to_string(), member_context);
                res
            }
        }
    }
}

struct ExternalFileSymbolMapImpl<F: FileContents + 'static>(
    Yoke<ExternalFileContextWrapper<'static>, Box<ExternalFileData<F>>>,
);

trait ExternalFileSymbolMapTrait {
    fn name(&self) -> &str;
    fn is_same_file(&self, external_file_ref: &ExternalFileRef) -> bool;
    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>>;
}

impl<F: FileContents + 'static> ExternalFileSymbolMapImpl<F> {
    pub fn new(file_name: &str, file: F, arch: Option<&str>) -> Result<Self, Error> {
        let external_file = Box::new(ExternalFileData::new(file_name, file, arch)?);
        let inner =
            Yoke::<ExternalFileContextWrapper<'static>, Box<ExternalFileData<F>>>::attach_to_cart(
                external_file,
                |external_file| {
                    let uplooker = external_file.make_type_erased_file_context();
                    ExternalFileContextWrapper(uplooker)
                },
            );
        Ok(Self(inner))
    }
}

impl<F: FileContents + 'static> ExternalFileSymbolMapTrait for ExternalFileSymbolMapImpl<F> {
    fn name(&self) -> &str {
        self.0.backing_cart().name()
    }

    fn is_same_file(&self, external_file_ref: &ExternalFileRef) -> bool {
        self.name() == external_file_ref.file_name
    }

    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        self.0.get().0.lookup(external_file_address)
    }
}

/// A symbol map for an external object file. You usually don't need this because
/// you usually call `SymbolManager::lookup_external`.
#[cfg(feature = "send_futures")]
pub struct ExternalFileSymbolMap(Box<dyn ExternalFileSymbolMapTrait + Send + Sync>);

/// A symbol map for an external object file. You usually don't need this because
/// you usually call `SymbolManager::lookup_external`.
#[cfg(not(feature = "send_futures"))]
pub struct ExternalFileSymbolMap(Box<dyn ExternalFileSymbolMapTrait>);

impl ExternalFileSymbolMap {
    /// The string which identifies this external file. This is usually an absolute
    /// path. (XXX does this contain the `archive.a(membername)` stuff or no?)
    pub fn name(&self) -> &str {
        self.0.name()
    }

    /// Checks whether `external_file_ref` refers to this external file.
    ///
    /// Used to avoid repeated loading of the same external file.
    pub fn is_same_file(&self, external_file_ref: &ExternalFileRef) -> bool {
        self.0.is_same_file(external_file_ref)
    }

    /// Look up the debug info for the given [`ExternalFileAddressInFileRef`].
    pub fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        self.0.lookup(external_file_address)
    }
}

#[cfg(feature = "send_futures")]
#[derive(Yokeable)]
struct ExternalFileContextWrapper<'a>(Box<dyn ExternalFileContextTrait + 'a + Send + Sync>);

#[cfg(not(feature = "send_futures"))]
#[derive(Yokeable)]
struct ExternalFileContextWrapper<'a>(Box<dyn ExternalFileContextTrait + 'a>);

impl<F: FileContents> ExternalFileDataOuterTrait for ExternalFileData<F> {
    #[cfg(feature = "send_futures")]
    fn make_type_erased_file_context(
        &self,
    ) -> Box<dyn ExternalFileContextTrait + '_ + Send + Sync> {
        Box::new(self.make_file_context())
    }
    #[cfg(not(feature = "send_futures"))]
    fn make_type_erased_file_context(&self) -> Box<dyn ExternalFileContextTrait + '_> {
        Box::new(self.make_file_context())
    }
    fn make_member_context<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalFileMemberContext<'s>, Error> {
        use object::{Object, ObjectSymbol};
        let ArchiveMemberObject { data, object_file } = self.get_archive_member(name_in_archive)?;
        let context = self
            .addr2line_context_data
            .make_context(data, &object_file, None, None);
        let symbol_addresses = object_file
            .symbols()
            .filter_map(|symbol| {
                let name = symbol.name_bytes().ok()?;
                let address = symbol.address();
                Some((name, address))
            })
            .collect();
        let uplooker = ExternalFileMemberContext {
            context: context.ok(),
            symbol_addresses,
        };
        Ok(uplooker)
    }
    fn name(&self) -> &str {
        &self.name
    }
}

struct ArchiveMemberObject<'a, R: ReadRef<'a>> {
    data: R,
    object_file: object::read::File<'a, R>,
}

struct ExternalFileData<F: FileContents> {
    name: String,
    file_contents: FileContentsWrapper<F>,
    /// name in bytes -> (start, size) in file_contents
    archive_members_by_name: HashMap<Vec<u8>, (u64, u64)>,
    fat_archive_range: Option<(u64, u64)>,
    addr2line_context_data: Addr2lineContextData,
}

impl<F: FileContents> ExternalFileData<F> {
    pub fn new(file_name: &str, file: F, arch: Option<&str>) -> Result<Self, Error> {
        let mut archive_members_by_name: HashMap<Vec<u8>, (u64, u64)> = HashMap::new();
        let file_contents = FileContentsWrapper::new(file);
        let mut fat_archive_range = None;
        let file_kind = FileKind::parse(&file_contents)
            .map_err(|_| Error::CouldNotDetermineExternalFileFileKind)?;
        match file_kind {
            FileKind::Archive => {
                if let Ok(archive) = ArchiveFile::parse(&file_contents) {
                    for member in archive.members().flatten() {
                        archive_members_by_name
                            .insert(member.name().to_owned(), member.file_range());
                    }
                }
            }
            FileKind::MachO32 | FileKind::MachO64 => {
                // Good
            }
            FileKind::MachOFat32 | FileKind::MachOFat64 => {
                let disambiguator = arch.map(|arch| MultiArchDisambiguator::Arch(arch.to_string()));
                let member =
                    macho::get_fat_archive_member(&file_contents, file_kind, disambiguator)?;
                fat_archive_range = Some(member.offset_and_size);
            }
            _ => {
                return Err(Error::UnexpectedExternalFileFileKind(file_kind));
            }
        }
        Ok(Self {
            name: file_name.to_owned(),
            file_contents,
            archive_members_by_name,
            fat_archive_range,
            addr2line_context_data: Addr2lineContextData::new(),
        })
    }

    fn get_archive_member<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ArchiveMemberObject<'s, RangeReadRef<&'s FileContentsWrapper<F>>>, Error> {
        let data = &self.file_contents;
        let data = match (name_in_archive, self.fat_archive_range) {
            (Some(name_in_archive), _) => {
                let (start, size) = self
                    .archive_members_by_name
                    .get(name_in_archive.as_bytes())
                    .ok_or_else(|| Error::FileNotInArchive(name_in_archive.to_owned()))?;
                RangeReadRef::new(data, *start, *size)
            }
            (None, Some((offset, size))) => RangeReadRef::new(data, offset, size),
            (None, None) => RangeReadRef::new(data, 0, data.len()),
        };
        let object_file = File::parse(data).map_err(Error::MachOHeaderParseError)?;
        Ok(ArchiveMemberObject { data, object_file })
    }

    pub fn make_file_context(&self) -> ExternalFileContext<'_, F> {
        let path_mapper = PathMapper::new();
        ExternalFileContext {
            external_file: self,
            member_contexts: Mutex::new(HashMap::new()),
            path_mapper: Mutex::new(path_mapper),
        }
    }
}
