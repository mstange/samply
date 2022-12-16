use crate::debugid_util::debug_id_for_object;
use crate::demangle;
use crate::error::{Context, Error};
use crate::path_mapper::{ExtraPathMapper, PathMapper};
use crate::shared::{
    AddressInfo, BasePath, FileAndPathHelper, FileContents, FileContentsWrapper, FileLocation,
    FramesLookupResult, InlineStackFrame, SymbolInfo,
};
use crate::symbol_map::{
    GenericSymbolMap, SymbolMap, SymbolMapDataMidTrait, SymbolMapDataOuterTrait,
    SymbolMapInnerWrapper, SymbolMapTrait,
};
use crate::symbol_map_object::{FunctionAddressesComputer, ObjectSymbolMapDataMid};
use debugid::DebugId;
use object::{File, FileKind};
use pdb::PDB;
use pdb_addr2line::pdb;
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Mutex;

pub async fn load_symbol_map_for_pdb_corresponding_to_binary<'h>(
    file_kind: FileKind,
    file_contents: &FileContentsWrapper<impl FileContents + 'static>,
    file_location: &FileLocation,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<SymbolMap, Error> {
    use object::Object;
    let pe =
        object::File::parse(file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;

    let info = match pe.pdb_info() {
        Ok(Some(info)) => info,
        _ => {
            return Err(Error::NoDebugInfoInPeBinary(
                file_location.to_string_lossy(),
            ))
        }
    };
    let binary_debug_id = debug_id_for_object(&pe).expect("we checked pdb_info above");

    let pdb_path_str = std::str::from_utf8(info.path())
        .map_err(|_| Error::PdbPathNotUtf8(file_location.to_string_lossy()))?;
    let pdb_path = PathBuf::from(pdb_path_str);
    let pdb_location = FileLocation::Path(pdb_path);
    let pdb_file = helper
        .open_file(&pdb_location)
        .await
        .map_err(|e| Error::HelperErrorDuringOpenFile(pdb_path_str.to_string(), e))?;
    let symbol_map = get_symbol_map_for_pdb(
        FileContentsWrapper::new(pdb_file),
        &pdb_location.to_base_path(),
    )?;
    if symbol_map.debug_id() != binary_debug_id {
        return Err(Error::UnmatchedDebugId(
            binary_debug_id,
            symbol_map.debug_id(),
        ));
    }
    Ok(symbol_map)
}

pub fn get_symbol_map_for_pe<F>(
    file_contents: FileContentsWrapper<F>,
    file_kind: FileKind,
    base_path: &BasePath,
) -> Result<SymbolMap, Error>
where
    F: FileContents + 'static,
{
    let owner = PeSymbolMapData::new(file_contents, file_kind);
    let symbol_map = GenericSymbolMap::new(owner, base_path)?;
    Ok(SymbolMap(Box::new(symbol_map)))
}

struct PeSymbolMapData<T>
where
    T: FileContents,
{
    file_data: FileContentsWrapper<T>,
    file_kind: FileKind,
}

impl<T: FileContents> PeSymbolMapData<T> {
    pub fn new(file_data: FileContentsWrapper<T>, file_kind: FileKind) -> Self {
        Self {
            file_data,
            file_kind,
        }
    }
}

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for PeSymbolMapData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let object =
            File::parse(&self.file_data).map_err(|e| Error::ObjectParseError(self.file_kind, e))?;
        let object =
            ObjectSymbolMapDataMid::new(object, PeFunctionAddressesComputer, &self.file_data, None);

        Ok(Box::new(object))
    }
}

struct PeFunctionAddressesComputer;

impl<'data> FunctionAddressesComputer<'data> for PeFunctionAddressesComputer {
    fn compute_function_addresses<'file, O>(
        &'file self,
        object_file: &'file O,
    ) -> (Option<Vec<u32>>, Option<Vec<u32>>)
    where
        'data: 'file,
        O: object::Object<'data, 'file>,
    {
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
}

pub fn is_pdb_file<F: FileContents>(file: &FileContentsWrapper<F>) -> bool {
    PDB::open(file).is_ok()
}

struct PdbObject<'data, FC: FileContents + 'static> {
    context_data: pdb_addr2line::ContextPdbData<'data, 'data, &'data FileContentsWrapper<FC>>,
    debug_id: DebugId,
    srcsrv_stream: Option<Box<dyn Deref<Target = [u8]> + 'data>>,
}

impl<'data, FC: FileContents + 'static> SymbolMapDataMidTrait for PdbObject<'data, FC> {
    fn make_symbol_map_inner<'object>(
        &'object self,
        base_path: &BasePath,
    ) -> Result<SymbolMapInnerWrapper<'object>, Error> {
        let context = self.make_context()?;

        let path_mapper = match &self.srcsrv_stream {
            Some(srcsrv_stream) => Some(SrcSrvPathMapper::new(srcsrv::SrcSrvStream::parse(
                srcsrv_stream.deref(),
            )?)),
            None => None,
        };
        let path_mapper = PathMapper::new_with_maybe_extra_mapper(base_path, path_mapper);

        let symbol_map = PdbSymbolMapInner {
            context,
            debug_id: self.debug_id,
            path_mapper: Mutex::new(path_mapper),
        };
        Ok(SymbolMapInnerWrapper(Box::new(symbol_map)))
    }
}

impl<'data, FC: FileContents + 'static> PdbObject<'data, FC> {
    fn make_context<'object>(
        &'object self,
    ) -> Result<Box<dyn PdbAddr2lineContextTrait + 'object>, Error> {
        let context = self.context_data.make_context().context("make_context()")?;
        Ok(Box::new(context))
    }
}

trait PdbAddr2lineContextTrait {
    fn find_frames(
        &self,
        probe: u32,
    ) -> Result<Option<pdb_addr2line::FunctionFrames>, pdb_addr2line::Error>;
    fn function_count(&self) -> usize;
    fn functions(&self) -> Box<dyn Iterator<Item = pdb_addr2line::Function> + '_>;
}

impl<'a, 's> PdbAddr2lineContextTrait for pdb_addr2line::Context<'a, 's> {
    fn find_frames(
        &self,
        probe: u32,
    ) -> Result<Option<pdb_addr2line::FunctionFrames>, pdb_addr2line::Error> {
        self.find_frames(probe)
    }

    fn function_count(&self) -> usize {
        self.function_count()
    }

    fn functions(&self) -> Box<dyn Iterator<Item = pdb_addr2line::Function> + '_> {
        Box::new(self.functions())
    }
}

struct PdbSymbolMapInner<'object> {
    context: Box<dyn PdbAddr2lineContextTrait + 'object>,
    debug_id: DebugId,
    path_mapper: Mutex<PathMapper<SrcSrvPathMapper<'object>>>,
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
                Cow::Owned(f.name.unwrap_or_else(|| format!("fun_{:x}", start_rva))),
            )
        });
        Box::new(iter)
    }

    fn to_map(&self) -> Vec<(u32, String)> {
        self.context
            .functions()
            .map(|func| {
                let symbol_name = match func.name {
                    Some(name) => name,
                    None => "unknown".to_string(),
                };
                (func.start_rva, symbol_name)
            })
            .collect()
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        let function_frames = self.context.find_frames(address).ok()??;
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
            let mut path_mapper = self.path_mapper.lock().unwrap();
            let mut map_path = |path: Cow<str>| path_mapper.map_path(&path);
            let frames: Vec<_> = function_frames
                .frames
                .into_iter()
                .map(|frame| InlineStackFrame {
                    function: frame.function,
                    file_path: frame.file.map(&mut map_path),
                    line_number: frame.line,
                })
                .collect();
            FramesLookupResult::Available(frames)
        } else {
            FramesLookupResult::Unavailable
        };

        Some(AddressInfo { symbol, frames })
    }
}

fn box_stream<'data, T>(stream: T) -> Box<dyn Deref<Target = [u8]> + 'data>
where
    T: Deref<Target = [u8]> + 'data,
{
    Box::new(stream)
}

struct PdbSymbolData<T: FileContents + 'static>(FileContentsWrapper<T>);

impl<T: FileContents + 'static> SymbolMapDataOuterTrait for PdbSymbolData<T> {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error> {
        let mut pdb = PDB::open(&self.0)?;
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

        Ok(Box::new(PdbObject {
            context_data,
            debug_id,
            srcsrv_stream,
        }))
    }
}

pub fn get_symbol_map_for_pdb<F>(
    file_contents: FileContentsWrapper<F>,
    base_path: &BasePath,
) -> Result<SymbolMap, Error>
where
    F: FileContents + 'static,
{
    let symbol_map = GenericSymbolMap::new(PdbSymbolData(file_contents), base_path)?;
    Ok(SymbolMap(Box::new(symbol_map)))
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
    cache: HashMap<String, Option<String>>,
    github_regex: Regex,
    hg_regex: Regex,
    s3_regex: Regex,
    gitiles_regex: Regex,
    command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5: bool,
}

impl<'a> ExtraPathMapper for SrcSrvPathMapper<'a> {
    fn map_path(&mut self, path: &str) -> Option<String> {
        if let Some(value) = self.cache.get(path) {
            return value.clone();
        }

        let value = match self
            .srcsrv_stream
            .source_and_raw_var_values_for_path(path, "C:\\Dummy")
        {
            Ok(Some((srcsrv::SourceRetrievalMethod::Download { url }, _map))) => {
                Some(self.url_to_special_path(&url))
            }
            Ok(Some((srcsrv::SourceRetrievalMethod::ExecuteCommand { .. }, map))) => {
                // We're not going to execute a command here.
                // Instead, we have special handling for a few known cases (well, only one case for now).
                self.gitiles_to_special_path(&map)
            }
            _ => None,
        };
        self.cache.insert(path.to_string(), value.clone());
        value
    }
}

impl<'a> SrcSrvPathMapper<'a> {
    pub fn new(srcsrv_stream: srcsrv::SrcSrvStream<'a>) -> Self {
        let command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5 =
            Self::matches_chrome_gitiles_workaround(&srcsrv_stream);

        SrcSrvPathMapper {
            srcsrv_stream,
            cache: HashMap::new(),
            github_regex: Regex::new(r"^https://raw\.githubusercontent\.com/(?P<repo>[^/]+/[^/]+)/(?P<rev>[^/]+)/(?P<path>.*)$").unwrap(),
            hg_regex: Regex::new(r"^https://(?P<repo>hg\..+)/raw-file/(?P<rev>[0-9a-f]+)/(?P<path>.*)$").unwrap(),
            s3_regex: Regex::new(r"^https://(?P<bucket>[^/]+).s3.amazonaws.com/(?P<digest_and_path>.*)$").unwrap(),
            gitiles_regex: Regex::new(r"^https://(?P<repo>.+)\.git/\+/(?P<rev>[^/]+)/(?P<path>.*)\?format=TEXT$").unwrap(),
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
    fn gitiles_to_special_path(&self, map: &HashMap<String, String>) -> Option<String> {
        if !self.command_is_file_download_with_url_in_var4_and_uncompress_function_in_var5 {
            return None;
        }
        if map.get("var5").map(String::as_str) != Some("base64.b64decode") {
            return None;
        }

        // https://pdfium.googlesource.com/pdfium.git/+/dab1161c861cc239e48a17e1a5d729aa12785a53/core/fdrm/fx_crypt.cpp?format=TEXT
        // -> "git:pdfium.googlesource.com/pdfium:core/fdrm/fx_crypt.cpp:dab1161c861cc239e48a17e1a5d729aa12785a53"
        // https://chromium.googlesource.com/chromium/src.git/+/c15858db55ed54c230743eaa9678117f21d5517e/third_party/blink/renderer/core/svg/svg_point.cc?format=TEXT
        // -> "git:chromium.googlesource.com/chromium/src:third_party/blink/renderer/core/svg/svg_point.cc:c15858db55ed54c230743eaa9678117f21d5517e"
        let url = map.get("var4")?;
        let captures = self.gitiles_regex.captures(url)?;
        let repo = captures.name("repo").unwrap().as_str();
        let path = captures.name("path").unwrap().as_str();
        let rev = captures.name("rev").unwrap().as_str();
        Some(format!("git:{}:{}:{}", repo, path, rev))
    }

    fn url_to_special_path(&self, url: &str) -> String {
        if let Some(captures) = self.github_regex.captures(url) {
            // https://raw.githubusercontent.com/baldurk/renderdoc/v1.15/renderdoc/data/glsl/gl_texsample.h
            // -> "git:github.com/baldurk/renderdoc:renderdoc/data/glsl/gl_texsample.h:v1.15"
            let repo = captures.name("repo").unwrap().as_str();
            let path = captures.name("path").unwrap().as_str();
            let rev = captures.name("rev").unwrap().as_str();
            format!("git:github.com/{}:{}:{}", repo, path, rev)
        } else if let Some(captures) = self.hg_regex.captures(url) {
            // "https://hg.mozilla.org/mozilla-central/raw-file/1706d4d54ec68fae1280305b70a02cb24c16ff68/mozglue/baseprofiler/core/ProfilerBacktrace.cpp"
            // -> "hg:hg.mozilla.org/mozilla-central:mozglue/baseprofiler/core/ProfilerBacktrace.cpp:1706d4d54ec68fae1280305b70a02cb24c16ff68"
            let repo = captures.name("repo").unwrap().as_str();
            let path = captures.name("path").unwrap().as_str();
            let rev = captures.name("rev").unwrap().as_str();
            format!("hg:{}:{}:{}", repo, path, rev)
        } else if let Some(captures) = self.s3_regex.captures(url) {
            // "https://gecko-generated-sources.s3.amazonaws.com/7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h"
            // -> "s3:gecko-generated-sources:7a1db5dfd0061d0e0bcca227effb419a20439aef4f6c4e9cd391a9f136c6283e89043d62e63e7edbd63ad81c339c401092bcfeff80f74f9cae8217e072f0c6f3/x86_64-pc-windows-msvc/release/build/swgl-59e3a0e09f56f4ea/out/brush_solid_DEBUG_OVERDRAW.h:"
            let bucket = captures.name("bucket").unwrap().as_str();
            let digest_and_path = captures.name("digest_and_path").unwrap().as_str();
            format!("s3:{}:{}:", bucket, digest_and_path)
        } else {
            url.to_string()
        }
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
    ) -> std::result::Result<Box<dyn pdb::SourceView<'s>>, std::io::Error> {
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
