use std::{collections::HashMap, sync::Mutex, path::PathBuf};

use object::{read::archive::ArchiveFile, File};
use yoke::{Yoke, Yokeable};

use crate::{shared::{ExternalFileRef, FileContentsWrapper, BasePath, ExternalFileAddressRef, RangeReadRef}, Error, FileContents, FileAndPathHelper, FileLocation, path_mapper::PathMapper, InlineStackFrame, dwarf::{get_frames, Addr2lineContextData}};


pub async fn get_external_file<'h, H, F>(
    helper: &'h H,
    external_file_ref: &ExternalFileRef,
) -> Result<ExternalFileWithUplooker<F>, Error>
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
    Ok(ExternalFileWithUplooker::new(
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

struct ExternalObjectUplooker<'a> {
    context: Option<addr2line::Context<gimli::EndianSlice<'a, gimli::RunTimeEndian>>>,
    symbol_addresses: HashMap<&'a [u8], u64>,
}

impl<'a> ExternalObjectUplooker<'a> {
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

struct ExternalFileUplooker<'a, F: FileContents> {
    external_file: &'a ExternalFile<F>,
    object_uplookers: Mutex<HashMap<String, ExternalObjectUplooker<'a>>>,
    path_mapper: Mutex<PathMapper<()>>,
}

struct ExternalFile<F: FileContents> {
    name: String,
    file_contents: FileContentsWrapper<F>,
    base_path: BasePath,
    /// name in bytes -> (start, size) in file_contents
    archive_members_by_name: HashMap<Vec<u8>, (u64, u64)>,
    addr2line_context_data: Addr2lineContextData,
}

trait ExternalFileTrait {
    #[cfg(feature = "send_futures")]
    fn make_type_erased_uplooker(&self) -> Box<dyn ExternalFileUplookerTrait + '_ + Send + Sync>;
    #[cfg(not(feature = "send_futures"))]
    fn make_type_erased_uplooker(&self) -> Box<dyn ExternalFileUplookerTrait + '_>;

    fn make_object_uplooker<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalObjectUplooker<'s>, Error>;
    fn name(&self) -> &str;
}

trait ExternalFileUplookerTrait {
    fn lookup_address(
        &self,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>>;
}

impl<'a, F: FileContents> ExternalFileUplookerTrait for ExternalFileUplooker<'a, F> {
    fn lookup_address(
        &self,
        external_file_address: &ExternalFileAddressRef,
    ) -> Option<Vec<InlineStackFrame>> {
        let member_key = external_file_address
            .name_in_archive
            .as_deref()
            .unwrap_or("");
        let mut uplookers = self.object_uplookers.lock().unwrap();
        let mut path_mapper = self.path_mapper.lock().unwrap();
        match uplookers.get(member_key) {
            Some(uplooker) => uplooker.lookup_address(
                &external_file_address.symbol_name,
                external_file_address.offset_from_symbol,
                &mut path_mapper,
            ),
            None => {
                let uplooker = self
                    .external_file
                    .make_object_uplooker(external_file_address.name_in_archive.as_deref())
                    .ok()?;
                let res = uplooker.lookup_address(
                    &external_file_address.symbol_name,
                    external_file_address.offset_from_symbol,
                    &mut path_mapper,
                );
                uplookers.insert(member_key.to_string(), uplooker);
                res
            }
        }
    }
}

pub struct ExternalFileWithUplooker<F: FileContents>(
    Yoke<ExternalFileUplookerTypeErased<'static>, Box<ExternalFile<F>>>,
);

impl<F: FileContents> ExternalFileWithUplooker<F> {
    pub fn new(file_name: &str, file: F) -> Self {
        let external_file = Box::new(ExternalFile::new(file_name, file));
        let inner =
            Yoke::<ExternalFileUplookerTypeErased<'static>, Box<ExternalFile<F>>>::attach_to_cart(
                external_file,
                |external_file| {
                    let uplooker = external_file.make_type_erased_uplooker();
                    ExternalFileUplookerTypeErased(uplooker)
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
struct ExternalFileUplookerTypeErased<'a>(Box<dyn ExternalFileUplookerTrait + 'a + Send + Sync>);

#[cfg(not(feature = "send_futures"))]
#[derive(Yokeable)]
struct ExternalFileUplookerTypeErased<'a>(Box<dyn ExternalFileUplookerTrait + 'a>);

impl<F: FileContents> ExternalFileTrait for ExternalFile<F> {
    #[cfg(feature = "send_futures")]
    fn make_type_erased_uplooker(&self) -> Box<dyn ExternalFileUplookerTrait + '_ + Send + Sync> {
        Box::new(self.make_uplooker())
    }
    #[cfg(not(feature = "send_futures"))]
    fn make_type_erased_uplooker(&self) -> Box<dyn ExternalFileUplookerTrait + '_> {
        Box::new(self.make_uplooker())
    }
    fn make_object_uplooker<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalObjectUplooker<'s>, Error> {
        self.make_object_uplooker_impl(name_in_archive)
    }
    fn name(&self) -> &str {
        &self.name
    }
}

impl<F: FileContents> ExternalFile<F> {
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
    ) -> Result<
        (
            RangeReadRef<'s, &'s FileContentsWrapper<F>>,
            File<'s, RangeReadRef<'s, &'s FileContentsWrapper<F>>>,
        ),
        Error,
    > {
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
        Ok((data, object_file))
    }

    pub fn make_object_uplooker_impl<'s>(
        &'s self,
        name_in_archive: Option<&str>,
    ) -> Result<ExternalObjectUplooker<'s>, Error> {
        use object::{Object, ObjectSymbol};
        let (data, object_file) = self.get_archive_member(name_in_archive)?;
        let context = self.addr2line_context_data.make_context(data, &object_file);
        let symbol_addresses = object_file
            .symbols()
            .filter_map(|symbol| {
                let name = symbol.name_bytes().ok()?;
                let address = symbol.address();
                Some((name, address))
            })
            .collect();
        let uplooker = ExternalObjectUplooker {
            context: context.ok(),
            symbol_addresses,
        };
        Ok(uplooker)
    }

    pub fn make_uplooker(&self) -> ExternalFileUplooker<'_, F> {
        let path_mapper = PathMapper::new(&self.base_path);
        ExternalFileUplooker {
            external_file: self,
            object_uplookers: Mutex::new(HashMap::new()),
            path_mapper: Mutex::new(path_mapper),
        }
    }
}
