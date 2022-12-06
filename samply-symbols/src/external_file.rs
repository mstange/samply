use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use object::{read::archive::ArchiveFile, File, ReadRef};
use yoke::{Yoke, Yokeable};

use crate::{
    dwarf::{get_frames, Addr2lineContextData},
    path_mapper::PathMapper,
    shared::{
        BasePath, ExternalFileAddressRef, ExternalFileRef, FileContentsWrapper, RangeReadRef,
    },
    Error, FileAndPathHelper, FileContents, FileLocation, InlineStackFrame,
};

pub async fn get_external_file<'h, H, F>(
    helper: &'h H,
    external_file_ref: &ExternalFileRef,
) -> Result<ExternalFileSymbolMap<F>, Error>
where
    F: FileContents + 'static,
    H: FileAndPathHelper<'h, F = F>,
{
    let file = helper
        .open_file(&FileLocation::Path(
            external_file_ref.file_name.as_str().into(),
        ))
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(external_file_ref.file_name.clone(), e))?;
    Ok(ExternalFileSymbolMap::new(
        &external_file_ref.file_name,
        file,
    ))
}

// Disabled due to "higher-ranked lifetime error"
#[cfg(any())]
#[test]
fn test_future_send() {
    fn assert_is_send<T: Send>(_f: T) {}
    fn wrapper<'a, 'b, F, H, R>(
        base_path: &BasePath,
        file_contents: FileContentsWrapper<F>,
        file_range: Option<(u64, u64)>,
        query: SymbolicationQuery<'a>,
        helper: &'static H,
    ) where
        F: FileContents + Send + Sync,
        H: FileAndPathHelper<'static, F = F>,
        R: SymbolicationResult + Send,
        <H as FileAndPathHelper<'static>>::OpenFileFuture: Send,
        H: Sync,
    {
        let f = get_symbolication_result::<F, H, R>(
            base_path,
            file_contents,
            file_range,
            query,
            helper,
        );
        assert_is_send(f);
    }
}

struct ExternalFileMemberContext<'a> {
    context: Option<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    symbol_addresses: HashMap<&'a [u8], u64>,
}

impl<'a> ExternalFileMemberContext<'a> {
    pub fn lookup_address(
        &self,
        symbol_name: &[u8],
        offset_from_symbol: u32,
        path_mapper: &mut PathMapper<()>,
    ) -> Option<Vec<InlineStackFrame>> {
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
    fn lookup_address(
        &self,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>>;
}

impl<'a, F: FileContents> ExternalFileContextTrait for ExternalFileContext<'a, F> {
    fn lookup_address(
        &self,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        let member_key = external_file_address
            .name_in_archive
            .as_deref()
            .unwrap_or("");
        let mut member_contexts = self.member_contexts.lock().unwrap();
        let mut path_mapper = self.path_mapper.lock().unwrap();
        match member_contexts.get(member_key) {
            Some(member_context) => member_context.lookup_address(
                &external_file_address.symbol_name,
                external_file_address.offset_from_symbol,
                &mut path_mapper,
            ),
            None => {
                let member_context = self
                    .external_file
                    .make_member_context(external_file_address.name_in_archive.as_deref())
                    .ok()?;
                let res = member_context.lookup_address(
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

pub struct ExternalFileSymbolMap<F: FileContents>(
    Yoke<ExternalFileContextWrapper<'static>, Box<ExternalFileData<F>>>,
);

impl<F: FileContents> ExternalFileSymbolMap<F> {
    pub fn new(file_name: &str, file: F) -> Self {
        let external_file = Box::new(ExternalFileData::new(file_name, file));
        let inner =
            Yoke::<ExternalFileContextWrapper<'static>, Box<ExternalFileData<F>>>::attach_to_cart(
                external_file,
                |external_file| {
                    let uplooker = external_file.make_type_erased_file_context();
                    ExternalFileContextWrapper(uplooker)
                },
            );
        Self(inner)
    }

    pub fn name(&self) -> &str {
        self.0.backing_cart().name()
    }

    pub fn lookup_address(
        &self,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        self.0.get().0.lookup_address(external_file_address)
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
        let context = self.addr2line_context_data.make_context(data, &object_file);
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
    base_path: BasePath,
    /// name in bytes -> (start, size) in file_contents
    archive_members_by_name: HashMap<Vec<u8>, (u64, u64)>,
    addr2line_context_data: Addr2lineContextData,
}

impl<F: FileContents> ExternalFileData<F> {
    pub fn new(file_name: &str, file: F) -> Self {
        let base_path = BasePath::CanReferToLocalFiles(PathBuf::from(file_name));
        let file_contents = FileContentsWrapper::new(file);
        let archive_members_by_name: HashMap<Vec<u8>, (u64, u64)> =
            match ArchiveFile::parse(&file_contents) {
                Ok(archive) => archive
                    .members()
                    .filter_map(|member| match member {
                        Ok(member) => Some((member.name().to_owned(), member.file_range())),
                        Err(_) => None,
                    })
                    .collect(),
                Err(_) => HashMap::new(),
            };
        Self {
            name: file_name.to_owned(),
            file_contents,
            base_path,
            archive_members_by_name,
            addr2line_context_data: Addr2lineContextData::new(),
        }
    }

    fn get_archive_member<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ArchiveMemberObject<'s, RangeReadRef<&'s FileContentsWrapper<F>>>, Error> {
        let data = &self.file_contents;
        let data = match name_in_archive {
            Some(name_in_archive) => {
                let (start, size) = self
                    .archive_members_by_name
                    .get(name_in_archive.as_bytes())
                    .ok_or_else(|| Error::FileNotInArchive(name_in_archive.to_owned()))?;
                RangeReadRef::new(data, *start, *size)
            }
            None => RangeReadRef::new(data, 0, data.len()),
        };
        let object_file = File::parse(data).map_err(Error::MachOHeaderParseError)?;
        Ok(ArchiveMemberObject { data, object_file })
    }

    pub fn make_file_context(&self) -> ExternalFileContext<'_, F> {
        let path_mapper = PathMapper::new(&self.base_path);
        ExternalFileContext {
            external_file: self,
            member_contexts: Mutex::new(HashMap::new()),
            path_mapper: Mutex::new(path_mapper),
        }
    }
}
