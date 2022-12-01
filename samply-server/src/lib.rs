use flate2::read::GzDecoder;
use hyper::body::Bytes;
use hyper::server::conn::AddrIncoming;
use hyper::server::Builder;
use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Request, Response, Server};
use hyper::{Method, StatusCode};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use rand::RngCore;
use samply_api::debugid::{CodeId, DebugId};
use samply_api::query_api;
use samply_api::samply_symbols::{
    CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    OptionallySendFuture,
};
use serde_derive::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use symsrv::{FileContents, NtSymbolPathEntry, SymbolCache};
use tokio::io::AsyncReadExt;

mod moria_mac;

#[cfg(target_os = "macos")]
mod moria_mac_spotlight;

pub use symsrv;

const BAD_CHARS: &AsciiSet = &CONTROLS.add(b':').add(b'/');

#[test]
fn test_is_send_and_sync() {
    fn assert_is_send<T: Send>() {}
    fn assert_is_sync<T: Sync>() {}
    assert_is_send::<FileContents>();
    assert_is_sync::<FileContents>();
}

#[derive(Clone, Debug)]
pub enum PortSelection {
    OnePort(u16),
    TryMultiple(Range<u16>),
}

impl PortSelection {
    pub fn try_from_str(s: &str) -> std::result::Result<Self, <u16 as FromStr>::Err> {
        if s.ends_with('+') {
            let start = s.trim_end_matches('+').parse()?;
            let end = start + 100;
            Ok(PortSelection::TryMultiple(start..end))
        } else {
            Ok(PortSelection::OnePort(s.parse()?))
        }
    }
}

pub async fn start_server(
    profile_filename: Option<&Path>,
    port_selection: PortSelection,
    symbol_path: Vec<NtSymbolPathEntry>,
    verbose: bool,
    open_in_browser: bool,
) {
    let libinfo_map = if let Some(profile_filename) = profile_filename {
        // Read the profile.json file and parse it as JSON.
        // Build a map (debugName, breakpadID) -> debugPath from the information
        // in profile(\.processes\[\d+\])*(\.threads\[\d+\])?\.libs.
        let file = std::fs::File::open(profile_filename).expect("couldn't read file");
        let reader = BufReader::new(file);

        // Handle .gz profiles
        if profile_filename.extension() == Some(&OsString::from("gz")) {
            let decoder = GzDecoder::new(reader);
            let reader = BufReader::new(decoder);
            parse_libinfo_map_from_profile(reader).expect("couldn't parse json")
        } else {
            parse_libinfo_map_from_profile(reader).expect("couldn't parse json")
        }
    } else {
        HashMap::new()
    };

    let (builder, addr) = make_builder_at_port(port_selection);

    let token = generate_token();
    let path_prefix = format!("/{}", token);
    let server_origin = format!("http://{}", addr);
    let symbol_server_url = format!("{}{}", server_origin, path_prefix);
    let mut template_values: HashMap<&'static str, String> = HashMap::new();
    template_values.insert("SERVER_URL", server_origin.clone());
    template_values.insert("PATH_PREFIX", path_prefix.clone());

    let profiler_url = if profile_filename.is_some() {
        let profile_url = format!("{}/profile.json", symbol_server_url);

        let env_profiler_override = std::env::var("PROFILER_URL").ok();
        let profiler_origin = match &env_profiler_override {
            Some(s) => s.trim_end_matches('/'),
            None => "https://profiler.firefox.com",
        };

        let encoded_profile_url = utf8_percent_encode(&profile_url, BAD_CHARS).to_string();
        let encoded_symbol_server_url =
            utf8_percent_encode(&symbol_server_url, BAD_CHARS).to_string();
        let profiler_url = format!(
            "{}/from-url/{}/?symbolServer={}",
            profiler_origin, encoded_profile_url, encoded_symbol_server_url
        );
        template_values.insert("PROFILER_URL", profiler_url.clone());
        template_values.insert("PROFILE_URL", profile_url);
        Some(profiler_url)
    } else {
        None
    };

    let template_values = Arc::new(template_values);

    let helper = Arc::new(Helper::with_libinfo_map(libinfo_map, symbol_path, verbose));
    let new_service = make_service_fn(move |_conn| {
        let helper = helper.clone();
        let profile_filename = profile_filename.map(PathBuf::from);
        let template_values = template_values.clone();
        let path_prefix = path_prefix.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |req| {
                symbolication_service(
                    req,
                    template_values.clone(),
                    helper.clone(),
                    profile_filename.clone(),
                    path_prefix.clone(),
                )
            }))
        }
    });

    let server = builder.serve(new_service);

    eprintln!("Local server listening at {}", server_origin);
    if !open_in_browser {
        if let Some(profiler_url) = &profiler_url {
            eprintln!("  Open the profiler at {}", profiler_url);
        }
    }
    eprintln!("Press Ctrl+C to stop.");

    if open_in_browser {
        if let Some(profiler_url) = &profiler_url {
            let _ = webbrowser::open(profiler_url);
        }
    }

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

#[derive(Debug, Clone, Default)]
struct LibInfo {
    pub path: Option<String>,
    pub debug_path: Option<String>,
    #[allow(unused)]
    pub code_id: Option<CodeId>,
}

fn parse_libinfo_map_from_profile(
    reader: impl std::io::Read,
) -> Result<HashMap<(String, DebugId), LibInfo>, std::io::Error> {
    let profile: ProfileJsonProcess = serde_json::from_reader(reader)?;
    let mut libinfo_map = HashMap::new();
    add_to_libinfo_map_recursive(&profile, &mut libinfo_map);
    Ok(libinfo_map)
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonProcess {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
    #[serde(default)]
    pub threads: Vec<ProfileJsonThread>,
    #[serde(default)]
    pub processes: Vec<ProfileJsonProcess>,
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonThread {
    #[serde(default)]
    pub libs: Vec<ProfileJsonLib>,
}

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileJsonLib {
    pub debug_name: Option<String>,
    pub debug_path: Option<String>,
    pub path: Option<String>,
    pub breakpad_id: Option<String>,
    pub code_id: Option<String>,
}

// Returns a base32 string for 24 random bytes.
fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    nix_base32::to_nix_base32(&bytes)
}

fn make_builder_at_port(port_selection: PortSelection) -> (Builder<AddrIncoming>, SocketAddr) {
    match port_selection {
        PortSelection::OnePort(port) => {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            match Server::try_bind(&addr) {
                Ok(builder) => (builder, addr),
                Err(e) => {
                    eprintln!("Could not bind to port {}: {}", port, e);
                    std::process::exit(1)
                }
            }
        }
        PortSelection::TryMultiple(range) => {
            let mut error = None;
            for port in range.clone() {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                match Server::try_bind(&addr) {
                    Ok(builder) => return (builder, addr),
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
            }
            match error {
                Some(error) => {
                    eprintln!(
                        "Could not bind to any port in the range {:?}: {}",
                        range, error,
                    );
                }
                None => {
                    eprintln!("Binding failed, port range empty? {:?}", range);
                }
            }
            std::process::exit(1)
        }
    }
}

const TEMPLATE_WITH_PROFILE: &str = r#"
<!DOCTYPE html>
<html lang="en">
<meta charset="utf-8">
<title>Profiler Symbol Server</title>
<body>

<p>This is the profiler symbol server, running at <code>SERVER_URL</code>. You can:</p>
<ul>
    <li><a href="PROFILER_URL">Open the profile in the profiler UI</a></li>
    <li><a download href="PROFILE_URL">Download the raw profile JSON</a></li>
    <li>Obtain symbols by POSTing to <code>PATH_PREFIX/symbolicate/v5</code>, with the format specified by the <a href="https://tecken.readthedocs.io/en/latest/symbolication.html">Mozilla symbolication API documentation</a>.</li>
    <li>Obtain source code by POSTing to <code>PATH_PREFIX/source/v1</code>, with the format specified in this <a href="https://github.com/mstange/profiler-get-symbols/issues/24#issuecomment-989985588">github comment</a>.</li>
</ul>
"#;

const TEMPLATE_WITHOUT_PROFILE: &str = r#"
<!DOCTYPE html>
<html lang="en">
<meta charset="utf-8">
<title>Profiler Symbol Server</title>
<body>

<p>This is the profiler symbol server, running at <code>SERVER_URL</code>. You can:</p>
<ul>
    <li>Obtain symbols by POSTing to <code>PATH_PREFIX/symbolicate/v5</code>, with the format specified by the <a href="https://tecken.readthedocs.io/en/latest/symbolication.html">Mozilla symbolication API documentation</a>.</li>
    <li>Obtain source code by POSTing to <code>PATH_PREFIX/source/v1</code>, with the format specified in this <a href="https://github.com/mstange/profiler-get-symbols/issues/24#issuecomment-989985588">github comment</a>.</li>
</ul>
"#;

async fn symbolication_service(
    req: Request<Body>,
    template_values: Arc<HashMap<&'static str, String>>,
    helper: Arc<Helper>,
    profile_filename: Option<PathBuf>,
    path_prefix: String,
) -> Result<Response<Body>, hyper::Error> {
    let has_profile = profile_filename.is_some();
    let method = req.method();
    let path = req.uri().path();
    let mut response = Response::new(Body::empty());

    let path_without_prefix = match path.strip_prefix(&path_prefix) {
        None => {
            // The secret prefix was not part of the URL. Do not send CORS headers.
            match (method, path) {
                (&Method::GET, "/") => {
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("text/html"),
                    );
                    let template = match has_profile {
                        true => TEMPLATE_WITH_PROFILE,
                        false => TEMPLATE_WITHOUT_PROFILE,
                    };
                    *response.body_mut() =
                        Body::from(substitute_template(template, &*template_values));
                }
                _ => {
                    *response.status_mut() = StatusCode::NOT_FOUND;
                }
            }
            return Ok(response);
        }
        Some(path_without_prefix) => path_without_prefix,
    };

    // If we get here, then the secret prefix was part of the URL.
    // This part is open to the public: we allow requests across origins.
    // For background on CORS, see this document:
    // https://w3c.github.io/webappsec-cors-for-developers/#cors
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("*"),
    );

    match (method, path_without_prefix, profile_filename) {
        (&Method::OPTIONS, _, _) => {
            // https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/OPTIONS
            *response.status_mut() = StatusCode::NO_CONTENT;
            if req
                .headers()
                .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
            {
                // This is a CORS preflight request.
                // Reassure the client that we are CORS-aware and that it's free to request whatever.
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    header::HeaderValue::from_static("POST, GET, OPTIONS"),
                );
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    header::HeaderValue::from(86400),
                );
                if let Some(req_headers) = req.headers().get(header::ACCESS_CONTROL_REQUEST_HEADERS)
                {
                    // All headers are fine.
                    response
                        .headers_mut()
                        .insert(header::ACCESS_CONTROL_ALLOW_HEADERS, req_headers.clone());
                }
            } else {
                // This is a regular OPTIONS request. Just send an Allow header with the allowed methods.
                response.headers_mut().insert(
                    header::ALLOW,
                    header::HeaderValue::from_static("POST, GET, OPTIONS"),
                );
            }
        }
        (&Method::GET, "/profile.json", Some(profile_filename)) => {
            if profile_filename.extension() == Some(OsStr::new("gz")) {
                response.headers_mut().insert(
                    header::CONTENT_ENCODING,
                    header::HeaderValue::from_static("gzip"),
                );
            }
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json; charset=UTF-8"),
            );
            let (mut sender, body) = Body::channel();
            *response.body_mut() = body;

            // Stream the file out to the response body, asynchronously, after this function has returned.
            tokio::spawn(async move {
                let mut file = tokio::fs::File::open(&profile_filename)
                    .await
                    .expect("couldn't open profile file");
                let mut contents = vec![0; 1024 * 1024];
                loop {
                    let data_len = file
                        .read(&mut contents)
                        .await
                        .expect("couldn't read profile file");
                    if data_len == 0 {
                        break;
                    }
                    sender
                        .send_data(Bytes::copy_from_slice(&contents[..data_len]))
                        .await
                        .expect("couldn't send data");
                }
            });
        }
        (&Method::POST, path, _) => {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            let path = path.to_string();
            // Await the full body to be concatenated into a single `Bytes`...
            let full_body = hyper::body::to_bytes(req.into_body()).await?;
            let full_body = String::from_utf8(full_body.to_vec()).expect("invalid utf-8");
            let response_json = query_api(&path, &full_body, &*helper).await;

            *response.body_mut() = response_json.into();
        }
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    };

    Ok(response)
}

fn substitute_template(template: &str, template_values: &HashMap<&'static str, String>) -> String {
    let mut s = template.to_string();
    for (key, value) in template_values {
        s = s.replace(key, value);
    }
    s
}

struct Helper {
    libinfo_map: HashMap<(String, DebugId), LibInfo>,
    symbol_cache: SymbolCache,
    verbose: bool,
}

fn add_libs_to_libinfo_map(
    libs: &[ProfileJsonLib],
    libinfo_map: &mut HashMap<(String, DebugId), LibInfo>,
) {
    for lib in libs {
        if let Some(((debug_name, debug_id), libinfo)) = libinfo_map_entry_for_lib(lib) {
            libinfo_map.insert((debug_name, debug_id), libinfo);
        }
    }
}

fn libinfo_map_entry_for_lib(lib: &ProfileJsonLib) -> Option<((String, DebugId), LibInfo)> {
    let debug_name = lib.debug_name.clone()?;
    let breakpad_id = lib.breakpad_id.as_ref()?;
    let debug_path = lib.debug_path.clone();
    let path = lib.path.clone();
    let debug_id = DebugId::from_breakpad(breakpad_id).ok()?;
    let code_id = lib
        .code_id
        .as_deref()
        .and_then(|ci| CodeId::from_str(ci).ok());
    let libinfo = LibInfo {
        path,
        debug_path,
        code_id,
    };
    Some(((debug_name, debug_id), libinfo))
}

fn add_to_libinfo_map_recursive(
    profile: &ProfileJsonProcess,
    libinfo_map: &mut HashMap<(String, DebugId), LibInfo>,
) {
    add_libs_to_libinfo_map(&profile.libs, libinfo_map);
    for thread in &profile.threads {
        add_libs_to_libinfo_map(&thread.libs, libinfo_map);
    }
    for process in &profile.processes {
        add_to_libinfo_map_recursive(process, libinfo_map);
    }
}

impl Helper {
    pub fn with_libinfo_map(
        libinfo_map: HashMap<(String, DebugId), LibInfo>,
        symbol_path: Vec<NtSymbolPathEntry>,
        verbose: bool,
    ) -> Self {
        let symbol_cache = SymbolCache::new(symbol_path, verbose);
        Helper {
            libinfo_map,
            symbol_cache,
            verbose,
        }
    }

    async fn open_file_impl(
        &self,
        location: FileLocation,
    ) -> FileAndPathHelperResult<FileContents> {
        match location {
            FileLocation::Path(path) => {
                if self.verbose {
                    eprintln!("Opening file {:?}", path.to_string_lossy());
                }
                let file = File::open(&path)?;
                Ok(FileContents::Mmap(unsafe {
                    memmap2::MmapOptions::new().map(&file)?
                }))
            }
            FileLocation::Custom(custom) => {
                assert!(custom.starts_with("symbolserver:"));
                let path = custom.trim_start_matches("symbolserver:");
                if self.verbose {
                    eprintln!("Trying to get file {:?} from symbol cache", path);
                }
                Ok(self.symbol_cache.get_file(Path::new(path)).await?)
            }
        }
    }
}

impl<'h> FileAndPathHelper<'h> for Helper {
    type F = FileContents;
    type OpenFileFuture =
        Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>>;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        debug_id: &DebugId,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let mut paths = vec![];

        // Look up (debugName, breakpadId) in the path map.
        let libinfo = self
            .libinfo_map
            .get(&(debug_name.to_string(), *debug_id))
            .cloned()
            .unwrap_or_default();

        let mut got_dsym = false;

        if let Some(debug_path) = &libinfo.debug_path {
            // First, see if we can find a dSYM file for the binary.
            if let Some(dsym_path) =
                moria_mac::locate_dsym_fastpath(Path::new(debug_path), debug_id.uuid())
            {
                got_dsym = true;
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    dsym_path.clone(),
                )));
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    dsym_path
                        .join("Contents")
                        .join("Resources")
                        .join("DWARF")
                        .join(debug_name),
                )));
            }

            // Also consider .so.dbg files in the same directory.
            if debug_name.ends_with(".so") {
                let dbg_name = format!("{}.dbg", debug_name);
                let debug_path = PathBuf::from(debug_path);
                if let Some(dir) = debug_path.parent() {
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dir.join(dbg_name),
                    )));
                }
            }
        }

        if libinfo.debug_path != libinfo.path {
            if let Some(debug_path) = &libinfo.debug_path {
                // Get symbols from the debug file.
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    debug_path.into(),
                )));
            }
        }

        if !got_dsym {
            // Try a little harder to find a dSYM, just from the UUID. We can do this
            // even if we don't have an entry for this library in the libinfo map.
            if let Ok(dsym_path) = moria_mac::locate_dsym_using_spotlight(debug_id.uuid()) {
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    dsym_path.clone(),
                )));
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    dsym_path
                        .join("Contents")
                        .join("Resources")
                        .join("DWARF")
                        .join(debug_name),
                )));
            }
        }

        // Find debuginfo in /usr/lib/debug/.build-id/ etc.
        // <https://sourceware.org/gdb/onlinedocs/gdb/Separate-Debug-Files.html>
        if let Some(code_id) = &libinfo.code_id {
            let code_id = code_id.as_str();
            if code_id.len() > 2 {
                let (two_chars, rest) = code_id.split_at(2);
                let path = format!("/usr/lib/debug/.build-id/{}/{}.debug", two_chars, rest);
                paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                    PathBuf::from(path),
                )));
            }
        }

        // Fake "debug link" support. We hardcode a "debug link name" of
        // `{debug_name}.debug`.
        // It would be better to get the actual debug link name from the binary.
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            PathBuf::from(format!("/usr/bin/{}.debug", &debug_name)),
        )));
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            PathBuf::from(format!("/usr/bin/.debug/{}.debug", &debug_name)),
        )));
        paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
            PathBuf::from(format!("/usr/lib/debug/usr/bin/{}.debug", &debug_name)),
        )));

        if debug_name.ends_with(".pdb") {
            // We might find this pdb file with the help of a symbol server.
            // Construct a custom string to identify this pdb.
            let custom = format!(
                "symbolserver:{}/{}/{}",
                debug_name,
                debug_id.breakpad(),
                debug_name
            );
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(custom)));
        }

        if let Some(path) = &libinfo.path {
            // Fall back to getting symbols from the binary itself.
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                path.into(),
            )));

            // For macOS system libraries, also consult the dyld shared cache.
            if path.starts_with("/usr/") || path.starts_with("/System/") {
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e")
                        .to_path_buf(),
                    dylib_path: path.clone(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_x86_64")
                        .to_path_buf(),
                    dylib_path: path.clone(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_arm64e")
                        .to_path_buf(),
                    dylib_path: path.clone(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64h")
                        .to_path_buf(),
                    dylib_path: path.clone(),
                });
                paths.push(CandidatePathInfo::InDyldCache {
                    dyld_cache_path: Path::new("/System/Library/dyld/dyld_shared_cache_x86_64")
                        .to_path_buf(),
                    dylib_path: path.clone(),
                });
            }
        }

        Ok(paths)
    }

    fn open_file(
        &'h self,
        location: &FileLocation,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>> {
        Box::pin(self.open_file_impl(location.clone()))
    }
}

#[cfg(test)]
mod test {
    use crate::{ProfileJsonLib, ProfileJsonProcess};

    #[test]
    fn deserialize_profile_json() {
        let p: ProfileJsonProcess = serde_json::from_str("{}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonProcess = serde_json::from_str("{\"unknown_field\":[1, 2, 3]}").unwrap();
        assert!(p.libs.is_empty());
        assert!(p.threads.is_empty());
        assert!(p.processes.is_empty());

        let p: ProfileJsonProcess =
            serde_json::from_str("{\"threads\":[{\"libs\":[{}]}]}").unwrap();
        assert!(p.libs.is_empty());
        assert_eq!(p.threads.len(), 1);
        assert_eq!(p.threads[0].libs.len(), 1);
        assert_eq!(p.threads[0].libs[0], ProfileJsonLib::default());
        assert!(p.processes.is_empty());
    }
}
