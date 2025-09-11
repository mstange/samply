use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use debugid::DebugId;
use object::{File, FileKind};
use pdb::PDB;
use pdb_addr2line::pdb;
use yoke::Yoke;
use yoke_derive::Yokeable;

use crate::debugid_util::debug_id_for_object;
use crate::dwarf::Addr2lineContextData;
use crate::error::{Context, Error};
use crate::generation::SymbolMapGeneration;
use crate::mapped_path::UnparsedMappedPath;
use crate::shared::{
    FileAndPathHelper, FileContents, FileContentsWrapper, FileLocation, FrameDebugInfo,
    FramesLookupResult, LookupAddress, SymbolInfo,
};
use crate::symbol_map::{GetInnerSymbolMap, SymbolMap, SymbolMapTrait};
use crate::symbol_map_object::{
    ObjectSymbolMap, ObjectSymbolMapInnerWrapper, ObjectSymbolMapOuter,
};
use crate::{demangle, PathInterner, SourceFilePath, SourceFilePathHandle, SyncAddressInfo};

pub async fn load_symbol_map_for_pdb_corresponding_to_binary<H: FileAndPathHelper>(
    file_kind: FileKind,
    file_contents: &FileContentsWrapper<H::F>,
    file_location: H::FL,
    helper: &H,
) -> Result<SymbolMap<H>, Error> {
    use object::Object;
    let pe =
        object::File::parse(file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    let info = match pe.pdb_info() {
        Ok(Some(info)) => info,
        _ => return Err(Error::NoDebugInfoInPeBinary(file_location.to_string())),
    };
    let binary_debug_id = debug_id_for_object(&pe).expect("we checked pdb_info above");

    let pdb_path_str = std::str::from_utf8(info.path())
        .map_err(|_| Error::PdbPathNotUtf8(file_location.to_string()))?;
    let pdb_location = file_location
        .location_for_pdb_from_binary(pdb_path_str)
        .ok_or(Error::FileLocationRefusedPdbLocation)?;
    let pdb_file = helper
        .load_file(pdb_location)
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(pdb_path_str.to_string(), e))?;
    let symbol_map = get_symbol_map_for_pdb(FileContentsWrapper::new(pdb_file), file_location)?;
    if symbol_map.debug_id() != binary_debug_id {
        return Err(Error::UnmatchedDebugId(
            binary_debug_id,
            symbol_map.debug_id(),
        ));
    }
    Ok(symbol_map)
}

pub fn get_symbol_map_for_pe<H: FileAndPathHelper>(
    file_contents: FileContentsWrapper<H::F>,
    file_kind: FileKind,
    file_location: H::FL,
    helper: Arc<H>,
) -> Result<SymbolMap<H>, Error> {
    let owner = PeSymbolMapDataAndObject::new(file_contents, file_kind)?;
    let symbol_map = ObjectSymbolMap::new(owner)?;
    Ok(SymbolMap::new_with_external_file_support(
        file_location,
        Box::new(symbol_map),
        helper,
    ))
}

#[derive(Yokeable)]
struct PeObject<'data, T: FileContents> {
    file_data: &'data FileContentsWrapper<T>,
    object: File<'data, &'data FileContentsWrapper<T>>,
    addr2line_context: Addr2lineContextData,
}

impl<'data, T: FileContents> PeObject<'data, T> {
    pub fn new(
        file_data: &'data FileContentsWrapper<T>,
        object: File<'data, &'data FileContentsWrapper<T>>,
    ) -> Self {
        Self {
            file_data,
            object,
            addr2line_context: Addr2lineContextData::new(),
        }
    }
}

struct PeSymbolMapDataAndObject<T: FileContents + 'static>(
    Yoke<PeObject<'static, T>, Box<FileContentsWrapper<T>>>,
);
impl<T: FileContents + 'static> PeSymbolMapDataAndObject<T> {
    pub fn new(file_data: FileContentsWrapper<T>, file_kind: FileKind) -> Result<Self, Error> {
        let data_and_object = Yoke::try_attach_to_cart(
            Box::new(file_data),
            move |file_data| -> Result<PeObject<'_, T>, Error> {
                let object =
                    File::parse(file_data).map_err(|e| Error::ObjectParseError(file_kind, e))?;
                Ok(PeObject::new(file_data, object))
            },
        )?;
        Ok(Self(data_and_object))
    }
}

impl<T: FileContents + 'static> ObjectSymbolMapOuter<T> for PeSymbolMapDataAndObject<T> {
    fn make_symbol_map_inner(&self) -> Result<ObjectSymbolMapInnerWrapper<'_, T>, Error> {
        let PeObject {
            file_data,
            object,
            addr2line_context,
        } = &self.0.get();
        let debug_id = debug_id_for_object(object)
            .ok_or(Error::InvalidInputError("debug ID cannot be read"))?;
        let (function_starts, function_ends) = compute_function_addresses_pe(object);
        let symbol_map = ObjectSymbolMapInnerWrapper::new(
            object,
            addr2line_context
                .make_context(*file_data, object, None, None)
                .ok(),
            None,
            debug_id,
            function_starts.as_deref(),
            function_ends.as_deref(),
            &(),
        );

        Ok(symbol_map)
    }
}

fn compute_function_addresses_pe<'data, O: object::Object<'data>>(
    object_file: &O,
) -> (Option<Vec<u32>>, Option<Vec<u32>>) {
    // Get function start and end addresses from the function list in .pdata.
    use object::ObjectSection;
    if let Some(pdata) = object_file
        .section_by_name_bytes(b".pdata")
        .and_then(|s| s.data().ok())
    {
        let (s, e) = function_start_and_end_addresses(pdata);
        (Some(s), Some(e))
    } else {
        (None, None)
    }
}

pub fn is_pdb_file<F: FileContents>(file: &FileContentsWrapper<F>) -> bool {
    PDB::open(file).is_ok()
}

struct PdbObject<'data, FC: FileContents + 'static> {
    context_data: pdb_addr2line::ContextPdbData<'data, 'data, &'data FileContentsWrapper<FC>>,
    debug_id: DebugId,
    srcsrv_stream: Option<Box<dyn Deref<Target = [u8]> + Send + 'data>>,
}

trait PdbObjectTrait {
    fn make_pdb_symbol_map(&self) -> Result<PdbSymbolMapInner<'_>, Error>;
}

#[derive(Yokeable)]
pub struct PdbObjectWrapper<'data>(Box<dyn PdbObjectTrait + Send + 'data>);

impl<FC: FileContents + 'static> PdbObjectTrait for PdbObject<'_, FC> {
    fn make_pdb_symbol_map(&self) -> Result<PdbSymbolMapInner<'_>, Error> {
        let context = self.make_context()?;

        let path_mapper = match &self.srcsrv_stream {
            Some(srcsrv_stream) => Some(SrcSrvPathMapper::new(srcsrv::SrcSrvStream::parse(
                srcsrv_stream.deref(),
            )?)),
            None => None,
        };

        let symbol_map = PdbSymbolMapInner {
            context,
            debug_id: self.debug_id,
            path_mapper: Mutex::new(path_mapper),
            path_interner: Mutex::new(PathInterner::new(SymbolMapGeneration::new())),
        };
        Ok(symbol_map)
    }
}

impl<FC: FileContents + 'static> PdbObject<'_, FC> {
    fn make_context<'object>(
        &'object self,
    ) -> Result<Box<dyn PdbAddr2lineContextTrait + Send + 'object>, Error> {
        let context = self.context_data.make_context().context("make_context()")?;
        Ok(Box::new(context))
    }
}

trait PdbAddr2lineContextTrait {
    fn find_frames(
        &self,
        probe: u32,
    ) -> Result<Option<pdb_addr2line::FunctionFrames<'_>>, pdb_addr2line::Error>;
    fn function_count(&self) -> usize;
    fn functions(&self) -> Box<dyn Iterator<Item = pdb_addr2line::Function> + '_>;
}

impl PdbAddr2lineContextTrait for pdb_addr2line::Context<'_, '_> {
    fn find_frames(
        &self,
        probe: u32,
    ) -> Result<Option<pdb_addr2line::FunctionFrames<'_>>, pdb_addr2line::Error> {
        self.find_frames(probe)
    }

    fn function_count(&self) -> usize {
        self.function_count()
    }

    fn functions(&self) -> Box<dyn Iterator<Item = pdb_addr2line::Function> + '_> {
        Box::new(self.functions())
    }
}

#[derive(Yokeable)]
pub struct PdbSymbolMapInnerWrapper<'data>(Box<dyn SymbolMapTrait + Send + 'data>);

struct PdbSymbolMapInner<'object> {
    context: Box<dyn PdbAddr2lineContextTrait + Send + 'object>,
    debug_id: DebugId,
    path_mapper: Mutex<Option<SrcSrvPathMapper<'object>>>,
    path_interner: Mutex<PathInterner<'object>>,
}

impl<'object> SymbolMapTrait for PdbSymbolMapInner<'object> {
    fn debug_id(&self) -> DebugId {
        self.debug_id
    }

    fn symbol_count(&self) -> usize {
        self.context.function_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let iter = self.context.functions().map(|f| {
            let start_rva = f.start_rva;
            (
                start_rva,
                Cow::Owned(f.name.unwrap_or_else(|| format!("fun_{start_rva:x}"))),
            )
        });
        Box::new(iter)
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        let rva = match address {
            LookupAddress::Relative(rva) => rva,
            LookupAddress::Svma(_) => {
                // TODO: Convert svma into rva by subtracting the image base address.
                // Does the PDB know about the image base address?
                return None;
            }
            LookupAddress::FileOffset(_) => {
                // TODO
                // Does the PDB know at which file offsets the sections are stored in the binary?
                return None;
            }
        };
        let function_frames = self.context.find_frames(rva).ok()??;
        let symbol_address = function_frames.start_rva;
        let symbol_name = match &function_frames.frames.last().unwrap().function {
            Some(name) => demangle::demangle_any(name),
            None => "unknown".to_string(),
        };
        let function_size = function_frames
            .end_rva
            .map(|end_rva| end_rva - function_frames.start_rva);

        let symbol = SymbolInfo {
            address: symbol_address,
            size: function_size,
            name: symbol_name,
        };
        let frames = if has_debug_info(&function_frames) {
            let mut path_interner = self.path_interner.lock().unwrap();
            let frames: Vec<_> = function_frames
                .frames
                .into_iter()
                .map(|frame| FrameDebugInfo {
                    function: frame.function,
                    file_path: frame.file.map(|p| path_interner.intern_owned(&p)),
                    line_number: frame.line,
                })
                .collect();
            Some(FramesLookupResult::Available(frames))
        } else {
            None
        };

        Some(SyncAddressInfo { symbol, frames })
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'object> {
        let mut path_mapper = self.path_mapper.lock().unwrap();
        let path_interner = self.path_interner.lock().unwrap();
        let path = path_interner.resolve(handle).expect("unknown handle?");

        if let Some(path_mapper) = &mut *path_mapper {
            if let Some(UnparsedMappedPath::Url(url)) = path_mapper.map_path(&path) {
                return SourceFilePath::RawPathAndUrl(path.clone(), url.clone());
            }
        }
        SourceFilePath::RawPath(path.clone())
    }
}

fn box_stream<'data, T>(stream: T) -> Box<dyn Deref<Target = [u8]> + Send + 'data>
where
    T: Deref<Target = [u8]> + Send + 'data,
{
    Box::new(stream)
}

struct PdbFileData<T: FileContents + 'static>(FileContentsWrapper<T>);

pub struct PdbObjectWithFileData<T: FileContents + 'static>(
    Yoke<PdbObjectWrapper<'static>, Box<PdbFileData<T>>>,
);

impl<T: FileContents + 'static> PdbObjectWithFileData<T> {
    fn new(file_data: PdbFileData<T>) -> Result<Self, Error> {
        let data_and_object = Yoke::try_attach_to_cart(Box::new(file_data), |file_data| {
            let mut pdb = PDB::open(&file_data.0)?;
            let info = pdb.pdb_information().context("pdb_information")?;
            let dbi = pdb.debug_information()?;
            let age = dbi.age().unwrap_or(info.age);
            let debug_id = DebugId::from_parts(info.guid, age);

            let srcsrv_stream = match pdb.named_stream(b"srcsrv") {
                Ok(stream) => Some(box_stream(stream)),
                Err(pdb::Error::StreamNameNotFound | pdb::Error::StreamNotFound(_)) => None,
                Err(e) => return Err(Error::PdbError("pdb.named_stream(srcsrv)", e)),
            };

            let context_data = pdb_addr2line::ContextPdbData::try_from_pdb(pdb)
                .context("ContextConstructionData::try_from_pdb")?;

            let pdb_object = PdbObject {
                context_data,
                debug_id,
                srcsrv_stream,
            };

            Ok(PdbObjectWrapper(Box::new(pdb_object)))
        })?;
        Ok(PdbObjectWithFileData(data_and_object))
    }
}

pub struct PdbSymbolMap<T: FileContents + 'static>(
    Mutex<Yoke<PdbSymbolMapInnerWrapper<'static>, Box<PdbObjectWithFileData<T>>>>,
);

impl<T: FileContents> PdbSymbolMap<T> {
    pub fn new(outer: PdbObjectWithFileData<T>) -> Result<Self, Error> {
        let outer_and_inner = Yoke::try_attach_to_cart(
            Box::new(outer),
            |outer| -> Result<PdbSymbolMapInnerWrapper<'_>, Error> {
                let maker = outer.0.get().0.as_ref();
                let symbol_map = maker.make_pdb_symbol_map()?;
                Ok(PdbSymbolMapInnerWrapper(Box::new(symbol_map)))
            },
        )?;
        Ok(PdbSymbolMap(Mutex::new(outer_and_inner)))
    }

    fn with_inner<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&dyn SymbolMapTrait) -> R,
    {
        f(&*self.0.lock().unwrap().get().0)
    }
}

impl<T: FileContents> GetInnerSymbolMap for PdbSymbolMap<T> {
    fn get_inner_symbol_map<'a>(&'a self) -> &'a (dyn SymbolMapTrait + 'a) {
        self
    }
}

impl<T: FileContents> SymbolMapTrait for PdbSymbolMap<T> {
    fn debug_id(&self) -> debugid::DebugId {
        self.with_inner(|inner| inner.debug_id())
    }

    fn symbol_count(&self) -> usize {
        self.with_inner(|inner| inner.symbol_count())
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        let vec = self.with_inner(|inner| {
            let vec: Vec<_> = inner
                .iter_symbols()
                .map(|(addr, s)| (addr, s.to_string()))
                .collect();
            vec
        });
        Box::new(vec.into_iter().map(|(addr, s)| (addr, Cow::Owned(s))))
    }

    fn lookup_sync(&self, address: LookupAddress) -> Option<SyncAddressInfo> {
        self.with_inner(|inner| inner.lookup_sync(address))
    }

    fn resolve_source_file_path(&self, handle: SourceFilePathHandle) -> SourceFilePath<'_> {
        self.with_inner(|inner| inner.resolve_source_file_path(handle).to_owned())
    }
}

pub fn get_symbol_map_for_pdb<H: FileAndPathHelper>(
    file_contents: FileContentsWrapper<H::F>,
    debug_file_location: H::FL,
) -> Result<SymbolMap<H>, Error> {
    let file_data_and_object = PdbObjectWithFileData::new(PdbFileData(file_contents))?;
    let symbol_map = PdbSymbolMap::new(file_data_and_object)?;
    Ok(SymbolMap::new_plain(
        debug_file_location,
        Box::new(symbol_map),
    ))
}

/// Map raw file paths to special "permalink" paths, using the srcsrv stream.
/// This allows finding source code for applications that were not compiled on this
/// machine, for example when using PDBs that were downloaded from a symbol server.
/// The special paths produced here have the following formats:
///   - "hg:<repo>:<path>:<rev>"
///   - "git:<repo>:<path>:<rev>"
///   - "s3:<bucket>:<digest_and_path>:"
struct SrcSrvPathMapper<'a> {
    srcsrv_stream: srcsrv::SrcSrvStream<'a>,
    cache: HashMap<String, Option<UnparsedMappedPath<'a>>>,
    command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5: bool,
}

impl<'a> SrcSrvPathMapper<'a> {
    fn map_path(&mut self, path: &str) -> Option<UnparsedMappedPath<'a>> {
        if let Some(value) = self.cache.get(path) {
            return value.clone();
        }

        let value = match self
            .srcsrv_stream
            .source_and_raw_var_values_for_path(path, "C:\\Dummy")
        {
            Ok(Some((srcsrv::SourceRetrievalMethod::Download { url }, _map))) => {
                Some(UnparsedMappedPath::Url(Cow::Owned(url)))
            }
            Ok(Some((srcsrv::SourceRetrievalMethod::ExecuteCommand { .. }, map))) => {
                // We're not going to execute a command here.
                // Instead, we have special handling for a few known cases (well, only one case for now).
                self.gitiles_to_mapped_path(map)
            }
            _ => None,
        };
        self.cache.insert(path.to_string(), value.clone());
        value
    }

    pub fn new(srcsrv_stream: srcsrv::SrcSrvStream<'a>) -> Self {
        let command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5 =
            Self::matches_chrome_gitiles_workaround(&srcsrv_stream);

        SrcSrvPathMapper {
            srcsrv_stream,
            cache: HashMap::new(),
            command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5,
        }
    }

    /// Chrome PDBs contain a workaround for the fact that googlesource.com (which runs
    /// a "gitiles" instance) does not have URLs to request raw source files.
    /// Instead, there is only a URL to request base64-encoded files. But srcsrv doesn't
    /// have built-in support for base64-encoded files. So, as a workaround, the Chrome
    /// PDBs contain a command which uses python to manually download and decode the
    /// base64-encoded files.
    ///
    /// We do not want to execute any commands here. Instead, we try to detect this
    /// workaround and get the raw URL back out.
    fn matches_chrome_gitiles_workaround(srcsrv_stream: &srcsrv::SrcSrvStream<'a>) -> bool {
        // old (python 2):
        // SRC_EXTRACT_TARGET_DIR=%targ%\%fnbksl%(%var2%)\%var3%
        // SRC_EXTRACT_TARGET=%SRC_EXTRACT_TARGET_DIR%\%fnfile%(%var1%)
        // SRC_EXTRACT_CMD=cmd /c "mkdir "%SRC_EXTRACT_TARGET_DIR%" & python -c "import urllib2, base64;url = \"%var4%\";u = urllib2.urlopen(url);open(r\"%SRC_EXTRACT_TARGET%\", \"wb\").write(%var5%(u.read()))"
        // c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt.cpp*core/fdrm/fx_crypt.cpp*dab1161c861cc239e48a17e1a5d729aa12785a53*https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT*base64.b64decode
        //
        // new (python 3):
        // SRC_EXTRACT_TARGET_DIR=%targ%\%fnbksl%(%var2%)\%var3%
        // SRC_EXTRACT_TARGET=%SRC_EXTRACT_TARGET_DIR%\%fnfile%(%var1%)
        // SRC_EXTRACT_CMD=cmd /c "mkdir "%SRC_EXTRACT_TARGET_DIR%" & python3 -c "import urllib.request, base64;url = \"%var4%\";u = urllib.request.urlopen(url);open(r\"%SRC_EXTRACT_TARGET%\", \"wb\").write(%var5%(u.read()))"
        // c:\b\s\w\ir\cache\builder\src\third_party\pdfium\core\fdrm\fx_crypt.cpp*core/fdrm/fx_crypt.cpp*dab1161c861cc239e48a17e1a5d729aa12785a53*https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT*base64.b64decode
        let cmd = srcsrv_stream.get_raw_var("SRC_EXTRACT_CMD");
        srcsrv_stream.get_raw_var("SRCSRVCMD") == Some("%SRC_EXTRACT_CMD%")
            && (cmd
                == Some(
                    r#"cmd /c "mkdir "%SRC_EXTRACT_TARGET_DIR%" & python3 -c "import urllib.request, base64;url = \"%var4%\";u = urllib.request.urlopen(url);open(r\"%SRC_EXTRACT_TARGET%\", \"wb\").write(%var5%(u.read()))""#,
                )
                || cmd
                    == Some(
                        r#"SRC_EXTRACT_CMD=cmd /c "mkdir "%SRC_EXTRACT_TARGET_DIR%" & python3 -c "import urllib.request, base64;url = \"%var4%\";u = urllib.request.urlopen(url);open(r\"%SRC_EXTRACT_TARGET%\", \"wb\").write(%var5%(u.read()))""#,
                    ))
    }

    /// Gitiles is the git source hosting service used by Google projects like android, chromium
    /// and pdfium. The chromium instance is at https://chromium.googlesource.com/chromium/src.git.
    ///
    /// There is *no* way to get raw source files over HTTP from this service.
    /// See https://github.com/google/gitiles/issues/7.
    ///
    /// Instead, you can get base64-encoded files, using the ?format=TEXT modifier.
    ///
    /// Due to this limitation, the Chrome PDBs contain a workaround which uses python to do the
    /// base64 decoding. We detect this workaround and try to obtain the original paths.
    fn gitiles_to_mapped_path(
        &self,
        mut map: HashMap<String, String>,
    ) -> Option<UnparsedMappedPath<'a>> {
        if !self.command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5 {
            return None;
        }
        if map.remove("var5").as_deref() != Some("base64.b64decode") {
            return None;
        }

        let url = map.remove("var4")?;
        Some(UnparsedMappedPath::Url(Cow::Owned(url)))
    }
}

fn has_debug_info(func: &pdb_addr2line::FunctionFrames) -> bool {
    if func.frames.len() > 1 {
        true
    } else if func.frames.is_empty() {
        false
    } else {
        func.frames[0].file.is_some() || func.frames[0].line.is_some()
    }
}

#[derive(Clone)]
struct ReadView {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for ReadView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReadView({} bytes)", self.bytes.len())
    }
}

impl pdb::SourceView<'_> for ReadView {
    fn as_slice(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

impl<'s, F: FileContents> pdb::Source<'s> for &'s FileContentsWrapper<F> {
    fn view(
        &mut self,
        slices: &[pdb::SourceSlice],
    ) -> std::result::Result<Box<dyn pdb::SourceView<'s> + Send + Sync>, std::io::Error> {
        let len = slices.iter().fold(0, |acc, s| acc + s.size);

        let mut bytes = Vec::with_capacity(len);

        for slice in slices {
            self.read_bytes_into(&mut bytes, slice.offset, slice.size)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }

        Ok(Box::new(ReadView { bytes }))
    }
}

/// Get the function start addresses (in rva form) from the .pdata section.
/// This section has the addresses for functions with unwind info. That means
/// it only covers a subset of functions; it does not include entries for
/// leaf functions which don't allocate any stack space.
fn function_start_and_end_addresses(pdata: &[u8]) -> (Vec<u32>, Vec<u32>) {
    let mut start_addresses = Vec::new();
    let mut end_addresses = Vec::new();
    for entry in pdata.chunks_exact(3 * std::mem::size_of::<u32>()) {
        let start_address = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let end_address = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
        start_addresses.push(start_address);
        end_addresses.push(end_address);
    }
    (start_addresses, end_addresses)
}
