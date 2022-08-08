use crate::debugid_util::debug_id_for_object;
use crate::demangle;
use crate::error::{Context, Error};
use crate::path_mapper::{ExtraPathMapper, PathMapper};
use crate::shared::{
    get_symbolication_result_for_addresses_from_object, object_to_map, AddressDebugInfo, BasePath,
    FileAndPathHelper, FileContents, FileContentsWrapper, FileLocation, InlineStackFrame,
    SymbolicationQuery, SymbolicationResult, SymbolicationResultKind,
};
use debugid::DebugId;
use pdb::PDB;
use pdb_addr2line::pdb;
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_symbolication_result_via_binary<'h, R>(
    file_kind: object::FileKind,
    file_contents: FileContentsWrapper<impl FileContents>,
    query: SymbolicationQuery<'_>,
    file_location: &FileLocation,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<R, Error>
where
    R: SymbolicationResult,
{
    let SymbolicationQuery {
        debug_name,
        debug_id,
        ..
    } = query.clone();
    use object::{Object, ObjectSection};
    let pe =
        object::File::parse(&file_contents).map_err(|e| Error::ObjectParseError(file_kind, e))?;
    let info = match pe.pdb_info() {
        Ok(Some(info)) => info,
        _ => {
            return Err(Error::NoDebugInfoInPeBinary(
                file_location.to_string_lossy(),
            ))
        }
    };

    // We could check the binary's signature here against debug_id, but we don't really
    // care whether we have the right binary. As long as we find a PDB file with the right
    // signature, that's all we need, and we'll happily accept correct PDB files even when
    // we found them via incorrect binaries.

    let pdb_path =
        std::ffi::CString::new(info.path()).expect("info.path() should have stripped the nul byte");

    let candidate_paths_for_pdb = helper
        .get_candidate_paths_for_pdb(debug_name, &debug_id, &pdb_path, file_location)
        .map_err(|e| {
            Error::HelperErrorDuringGetCandidatePathsForPdb(debug_name.to_string(), debug_id, e)
        })?;

    for pdb_location in candidate_paths_for_pdb {
        if &pdb_location == file_location {
            continue;
        }
        if let Ok(table) =
            try_get_symbolication_result_from_pdb_location(query.clone(), &pdb_location, helper)
                .await
        {
            return Ok(table);
        }
    }

    // Fallback: If no PDB file is present, make a symbol table with just the exports.
    // Now it's time to check the debug ID!

    let file_debug_id = debug_id_for_object(&pe).expect("we checked pdb_info above");

    if debug_id != file_debug_id {
        return Err(Error::UnmatchedDebugId(file_debug_id, debug_id));
    }

    // Get function start and end addresses from the function list in .pdata.
    let mut function_starts = None;
    let mut function_ends = None;
    if let Some(pdata) = pe
        .section_by_name_bytes(b".pdata")
        .and_then(|s| s.data().ok())
    {
        let (s, e) = function_start_and_end_addresses(pdata);
        function_starts = Some(s);
        function_ends = Some(e);
    }

    let r = match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            let map = object_to_map(&pe, function_starts.as_deref());
            R::from_full_map(map)
        }
        SymbolicationResultKind::SymbolsForAddresses(addresses) => {
            get_symbolication_result_for_addresses_from_object(
                addresses,
                &pe,
                function_starts.as_deref(),
                function_ends.as_deref(),
            )
        }
    };
    Ok(r)
}

async fn try_get_symbolication_result_from_pdb_location<'h, R>(
    query: SymbolicationQuery<'_>,
    file_location: &FileLocation,
    helper: &'h impl FileAndPathHelper<'h>,
) -> Result<R, Error>
where
    R: SymbolicationResult,
{
    let file_contents = FileContentsWrapper::new(
        helper
            .open_file(file_location)
            .await
            .map_err(|e| Error::HelperErrorDuringOpenFile(file_location.to_string_lossy(), e))?,
    );
    let pdb = PDB::open(&file_contents)?;
    get_symbolication_result(&file_location.to_base_path(), pdb, query)
}

pub fn get_symbolication_result<'a, 's, S, R>(
    base_path: &BasePath,
    mut pdb: PDB<'s, S>,
    query: SymbolicationQuery<'a>,
) -> Result<R, Error>
where
    R: SymbolicationResult,
    S: pdb::Source<'s> + 's,
{
    // Check against the expected debug_id.
    let info = pdb.pdb_information().context("pdb_information")?;
    let dbi = pdb.debug_information()?;
    let age = dbi.age().unwrap_or(info.age);

    // HACK: Convert uuid 0.8.2 Uuid to uuid 1.0.0 Uuid.
    // Needs https://github.com/willglynn/pdb/pull/115
    let guid = Uuid::from_bytes(*info.guid.as_bytes());

    let file_debug_id = DebugId::from_parts(guid, age);

    let SymbolicationQuery { debug_id, .. } = query;

    if file_debug_id != debug_id {
        return Err(Error::UnmatchedDebugId(file_debug_id, debug_id));
    }

    let srcsrv_stream = match pdb.named_stream(b"srcsrv") {
        Ok(stream) => Some(stream),
        Err(pdb::Error::StreamNameNotFound | pdb::Error::StreamNotFound(_)) => None,
        Err(e) => return Err(Error::PdbError("pdb.named_stream(srcsrv)", e)),
    };

    let context_data = pdb_addr2line::ContextPdbData::try_from_pdb(pdb)
        .context("ContextConstructionData::try_from_pdb")?;
    let context = context_data.make_context().context("make_context()")?;

    match query.result_kind {
        SymbolicationResultKind::AllSymbols => {
            // Gather the symbols into a map.
            let symbol_map = context
                .functions()
                .map(|func| {
                    let symbol_name = match func.name {
                        Some(name) => name,
                        None => "unknown".to_string(),
                    };
                    (func.start_rva, Cow::from(symbol_name))
                })
                .collect();
            let symbolication_result = R::from_full_map(symbol_map);
            Ok(symbolication_result)
        }
        SymbolicationResultKind::SymbolsForAddresses(addresses) => {
            let path_mapper = match &srcsrv_stream {
                Some(srcsrv_stream) => Some(SrcSrvPathMapper::new(srcsrv::SrcSrvStream::parse(
                    srcsrv_stream.as_slice(),
                )?)),
                None => None,
            };
            let mut path_mapper = PathMapper::new_with_maybe_extra_mapper(base_path, path_mapper);
            let mut map_path = |path: Cow<str>| path_mapper.map_path(&path);

            let mut symbolication_result = R::for_addresses(addresses);
            for &address in addresses {
                if let Some(function_frames) = context.find_frames(address)? {
                    let symbol_address = function_frames.start_rva;
                    let symbol_name = match &function_frames.frames.last().unwrap().function {
                        Some(name) => demangle::demangle_any(name),
                        None => "unknown".to_string(),
                    };
                    let function_size = function_frames
                        .end_rva
                        .map(|end_rva| end_rva - function_frames.start_rva);
                    symbolication_result.add_address_symbol(
                        address,
                        symbol_address,
                        symbol_name,
                        function_size,
                    );
                    if has_debug_info(&function_frames) {
                        let frames: Vec<_> = function_frames
                            .frames
                            .into_iter()
                            .map(|frame| InlineStackFrame {
                                function: frame.function,
                                file_path: frame.file.map(&mut map_path),
                                line_number: frame.line,
                            })
                            .collect();
                        if !frames.is_empty() {
                            symbolication_result
                                .add_address_debug_info(address, AddressDebugInfo { frames });
                        }
                    }
                }
            }

            symbolication_result.set_total_symbol_count(context.function_count() as u32);

            Ok(symbolication_result)
        }
    }
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
