use std::{collections::HashMap, sync::Mutex};

use object::{read::archive::ArchiveFile, File, FileKind, ReadRef};
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::dwarf::{get_frames, Addr2lineContextData};
use crate::error::Error;
use crate::path_mapper::PathMapper;
use crate::shared::{
    ExternalFileAddressInFileRef, ExternalFileRef, FileAndPathHelper, FileContents,
    FileContentsWrapper, FileLocation, FrameDebugInfo,
};

pub async fn load_external_file<H>(
    helper: &H,
    original_file_location: &H::FL,
    external_file_ref: &ExternalFileRef,
) -> Result<ExternalFileSymbolMap<H::F>, Error>
where
    H: FileAndPathHelper,
{
    let file = helper
        .load_file(
            original_file_location
                .location_for_external_object_file(&external_file_ref.file_name)
                .ok_or(Error::FileLocationRefusedExternalObjectLocation)?,
        )
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(external_file_ref.file_name.clone(), e))?;
    let symbol_map = ExternalFileSymbolMap::new(&external_file_ref.file_name, file)?;
    Ok(symbol_map)
}

struct ExternalFileOuter<F: FileContents> {
    name: String,
    file_contents: FileContentsWrapper<F>,
    addr2line_context_data: Addr2lineContextData,
}

impl<F: FileContents> ExternalFileOuter<F> {
    pub fn new(file_name: &str, file: F) -> Self {
        let file_contents = FileContentsWrapper::new(file);
        Self {
            name: file_name.to_owned(),
            file_contents,
            addr2line_context_data: Addr2lineContextData::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn make_member_context(
        &self,
        offset_and_size: (u64, u64),
    ) -> Result<ExternalFileMemberContext<'_>, Error> {
        let (start, size) = offset_and_size;
        let data = self.file_contents.range(start, size);
        let object_file = File::parse(data).map_err(Error::MachOHeaderParseError)?;
        self.make_single_context(data, object_file)
    }

    fn make_single_context<'s, R: ReadRef<'s>>(
        &'s self,
        data: R,
        object_file: object::read::File<'s, R>,
    ) -> Result<ExternalFileMemberContext<'s>, Error> {
        use object::{Object, ObjectSymbol};
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
        let member_context = ExternalFileMemberContext {
            context: context.ok(),
            symbol_addresses,
        };
        Ok(member_context)
    }

    pub fn make_inner(&self) -> Result<ExternalFileInner<'_, F>, Error> {
        let file_kind = FileKind::parse(&self.file_contents)
            .map_err(|_| Error::CouldNotDetermineExternalFileFileKind)?;
        let member_contexts = match file_kind {
            FileKind::MachO32 | FileKind::MachO64 => {
                let data = self.file_contents.full_range();
                let object_file = File::parse(data).map_err(Error::MachOHeaderParseError)?;
                let context = self.make_single_context(data, object_file)?;
                ExternalFileMemberContexts::SingleObject(context)
            }
            FileKind::Archive => {
                let archive = ArchiveFile::parse(&self.file_contents)
                    .map_err(Error::ParseErrorInExternalArchive)?;
                let mut member_ranges = HashMap::new();
                for member in archive.members() {
                    let member = member.map_err(Error::ParseErrorInExternalArchive)?;
                    let name = member.name().to_owned();
                    member_ranges.insert(name, member.file_range());
                }
                ExternalFileMemberContexts::Archive {
                    member_ranges,
                    contexts: Mutex::new(HashMap::new()),
                }
            }
            FileKind::MachOFat32 | FileKind::MachOFat64 => {
                return Err(Error::UnexpectedExternalFileFileKind(file_kind));
            }
            _ => {
                return Err(Error::UnexpectedExternalFileFileKind(file_kind));
            }
        };
        Ok(ExternalFileInner {
            external_file: self,
            member_contexts,
            path_mapper: Mutex::new(PathMapper::new()),
        })
    }
}

enum ExternalFileMemberContexts<'a> {
    SingleObject(ExternalFileMemberContext<'a>),
    /// member name -> context
    Archive {
        member_ranges: HashMap<Vec<u8>, (u64, u64)>,
        contexts: Mutex<HashMap<String, ExternalFileMemberContext<'a>>>,
    },
}

#[derive(Yokeable)]
struct ExternalFileInnerWrapper<'a>(Box<dyn ExternalFileInnerTrait + Send + 'a>);

trait ExternalFileInnerTrait {
    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>>;
}

struct ExternalFileInner<'a, T: FileContents> {
    external_file: &'a ExternalFileOuter<T>,
    member_contexts: ExternalFileMemberContexts<'a>,
    path_mapper: Mutex<PathMapper<()>>,
}

impl<'a, F: FileContents> ExternalFileInnerTrait for ExternalFileInner<'a, F> {
    fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        let mut path_mapper = self.path_mapper.lock().unwrap();
        match (&self.member_contexts, external_file_address) {
            (
                ExternalFileMemberContexts::SingleObject(context),
                ExternalFileAddressInFileRef::MachoOsoObject {
                    symbol_name,
                    offset_from_symbol,
                },
            ) => context.lookup(symbol_name, *offset_from_symbol, &mut path_mapper),
            (
                ExternalFileMemberContexts::Archive {
                    member_ranges,
                    contexts,
                },
                ExternalFileAddressInFileRef::MachoOsoArchive {
                    name_in_archive,
                    symbol_name,
                    offset_from_symbol,
                },
            ) => {
                let mut member_contexts = contexts.lock().unwrap();
                match member_contexts.get(name_in_archive) {
                    Some(member_context) => {
                        member_context.lookup(symbol_name, *offset_from_symbol, &mut path_mapper)
                    }
                    None => {
                        let range = *member_ranges.get(name_in_archive.as_bytes())?;
                        // .ok_or_else(|| Error::FileNotInArchive(name_in_archive.to_owned()))?;
                        let member_context = self.external_file.make_member_context(range).ok()?;
                        let res = member_context.lookup(
                            symbol_name,
                            *offset_from_symbol,
                            &mut path_mapper,
                        );
                        member_contexts.insert(name_in_archive.to_string(), member_context);
                        res
                    }
                }
            }
            (
                ExternalFileMemberContexts::SingleObject(_),
                ExternalFileAddressInFileRef::MachoOsoArchive { .. },
            )
            | (
                ExternalFileMemberContexts::Archive { .. },
                ExternalFileAddressInFileRef::MachoOsoObject { .. },
            ) => None,
        }
    }
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

pub struct ExternalFileSymbolMap<F: FileContents + 'static>(
    Yoke<ExternalFileInnerWrapper<'static>, Box<ExternalFileOuter<F>>>,
);

impl<F: FileContents + 'static> ExternalFileSymbolMap<F> {
    fn new(file_name: &str, file: F) -> Result<Self, Error> {
        let outer = ExternalFileOuter::new(file_name, file);
        let inner = Yoke::try_attach_to_cart(
            Box::new(outer),
            |outer| -> Result<ExternalFileInnerWrapper<'_>, Error> {
                let inner = outer.make_inner()?;
                Ok(ExternalFileInnerWrapper(Box::new(inner)))
            },
        )?;
        Ok(Self(inner))
    }

    /// The string which identifies this external file. This is usually an absolute
    /// path.
    pub fn name(&self) -> &str {
        self.0.backing_cart().name()
    }

    /// Checks whether `external_file_ref` refers to this external file.
    ///
    /// Used to avoid repeated loading of the same external file.
    pub fn is_same_file(&self, external_file_ref: &ExternalFileRef) -> bool {
        self.name() == external_file_ref.file_name
    }

    /// Look up the debug info for the given [`ExternalFileAddressInFileRef`].
    pub fn lookup(
        &self,
        external_file_address: &ExternalFileAddressInFileRef,
    ) -> Option<Vec<FrameDebugInfo>> {
        self.0.get().0.lookup(external_file_address)
    }
}
