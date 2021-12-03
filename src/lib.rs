use flate2::read::GzDecoder;
use hyper::server::conn::AddrIncoming;
use hyper::server::Builder;
use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Request, Response, Server};
use hyper::{Method, StatusCode};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use profiler_get_symbols::{
    self, query_api, CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    OptionallySendFuture,
};
use serde_json::{self, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::ffi::OsString;
use std::fs::File;
use std::io::Cursor;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

mod moria_mac;

#[cfg(target_os = "macos")]
mod moria_mac_spotlight;

mod symsrv;

pub use symsrv::{
    get_default_downstream_store, get_symbol_path_from_environment, parse_nt_symbol_path,
    NtSymbolPathEntry,
};
use symsrv::{FileContents, SymbolCache};

const BAD_CHARS: &AsciiSet = &CONTROLS.add(b':').add(b'/');

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
    let (profile, buffer) = if let Some(profile_filename) = profile_filename {
        // Read the profile.json file and parse it as JSON.
        let mut buffer = std::fs::read(profile_filename).expect("couldn't read file");

        // Handle .gz profiles
        if profile_filename.extension() == Some(&OsString::from("gz")) {
            use std::io::Read;
            let mut decompressed_buffer = Vec::new();
            let cursor = Cursor::new(&buffer);
            GzDecoder::new(cursor)
                .read_to_end(&mut decompressed_buffer)
                .expect("couldn't decompress gzip");
            buffer = decompressed_buffer
        }

        let profile: Value = serde_json::from_slice(&buffer).expect("couldn't parse json");
        let buffer = Arc::new(buffer);
        (Some(profile), Some(buffer))
    } else {
        (None, None)
    };

    let (builder, addr) = make_builder_at_port(port_selection);

    let server_origin = format!("http://{}", addr);
    let mut template_values: HashMap<&'static str, String> = HashMap::new();
    template_values.insert("SERVER_URL", server_origin.clone());

    let profiler_url = if profile_filename.is_some() {
        let profile_url = format!("{}/profile.json", server_origin);

        let env_profiler_override = std::env::var("PROFILER_URL").ok();
        let profiler_origin = match &env_profiler_override {
            Some(s) => s.trim_end_matches('/'),
            None => "https://profiler.firefox.com",
        };

        let encoded_profile_url = utf8_percent_encode(&profile_url, BAD_CHARS).to_string();
        let encoded_symbol_server_url = utf8_percent_encode(&server_origin, BAD_CHARS).to_string();
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

    let helper = Arc::new(Helper::for_profile(profile, symbol_path, verbose));
    let new_service = make_service_fn(move |_conn| {
        let helper = helper.clone();
        let raw_profile_json_data = buffer.clone();
        let template_values = template_values.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |req| {
                symbolication_service(
                    req,
                    template_values.clone(),
                    helper.clone(),
                    raw_profile_json_data.clone(),
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
    <li>Obtain symbols by POSTing to <code>/symbolicate/v5</code>, with the format specified by the <a href="https://tecken.readthedocs.io/en/latest/symbolication.html">Mozilla symbolication API documentation</a>.</li>
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
    <li>Obtain symbols by POSTing to <code>/symbolicate/v5</code>, with the format specified by the <a href="https://tecken.readthedocs.io/en/latest/symbolication.html">Mozilla symbolication API documentation</a>.</li>
</ul>
"#;

async fn symbolication_service(
    req: Request<Body>,
    template_values: Arc<HashMap<&'static str, String>>,
    helper: Arc<Helper>,
    raw_profile_json_data: Option<Arc<Vec<u8>>>,
) -> Result<Response<Body>, hyper::Error> {
    // This server is open to the public.
    // For background on CORS, see this document:
    // https://w3c.github.io/webappsec-cors-for-developers/#cors

    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("*"),
    );
    let has_profile = raw_profile_json_data.is_some();

    match (req.method(), req.uri().path(), raw_profile_json_data) {
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
        (&Method::GET, "/", _) => {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/html"),
            );
            let template = match has_profile {
                true => TEMPLATE_WITH_PROFILE,
                false => TEMPLATE_WITHOUT_PROFILE,
            };
            *response.body_mut() = Body::from(substitute_template(template, &*template_values));
        }
        (&Method::GET, "/profile.json", Some(raw_profile_json_data)) => {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json; charset=UTF-8"),
            );
            *response.body_mut() = Body::from((*raw_profile_json_data).clone());
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
    path_map: HashMap<(String, String), String>,
    symbol_cache: SymbolCache,
    verbose: bool,
}

fn add_to_path_map_recursive(profile: &Value, path_map: &mut HashMap<(String, String), String>) {
    if let Value::Array(libs) = &profile["libs"] {
        for lib in libs {
            let debug_name = lib["debugName"].as_str().unwrap().to_string();
            let breakpad_id = lib["breakpadId"].as_str().unwrap().to_string();
            let debug_path = lib["debugPath"].as_str().unwrap().to_string();
            path_map.insert((debug_name, breakpad_id), debug_path);
        }
    }
    if let Value::Array(processes) = &profile["processes"] {
        for process in processes {
            add_to_path_map_recursive(process, path_map);
        }
    }
}

impl Helper {
    pub fn for_profile(
        profile: Option<Value>,
        symbol_path: Vec<NtSymbolPathEntry>,
        verbose: bool,
    ) -> Self {
        // Build a map (debugName, breakpadID) -> debugPath from the information
        // in profile.libs.
        let mut path_map = HashMap::new();
        if let Some(profile) = profile {
            add_to_path_map_recursive(&profile, &mut path_map);
        }
        let symbol_cache = SymbolCache::new(symbol_path, verbose);
        Helper {
            path_map,
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
                Ok(self.symbol_cache.get_pdb(Path::new(path)).await?)
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
        breakpad_id: &str,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo>> {
        let mut paths = vec![];

        // Look up (debugName, breakpadId) in the path map.
        if let Some(path) = self
            .path_map
            .get(&(debug_name.to_string(), breakpad_id.to_string()))
        {
            // First, see if we can find a dSYM file for the binary.
            if let Ok(uuid) = Uuid::parse_str(&breakpad_id[0..32]) {
                if let Ok(dsym_path) = moria_mac::locate_dsym(&path, uuid) {
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

            // Also consider .so.dbg files in the same directory.
            if debug_name.ends_with(".so") {
                let debug_debug_name = format!("{}.dbg", debug_name);
                let path = PathBuf::from(path);
                if let Some(dir) = path.parent() {
                    paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                        dir.join(debug_debug_name),
                    )));
                }
            }

            // Fall back to getting symbols from the binary itself.
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Path(
                path.into(),
            )));

            // For macOS system libraries, also consult the dyld shared cache.
            if path.starts_with("/usr/") || path.starts_with("/System/") {
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

        if debug_name.ends_with(".pdb") {
            // We might find this pdb file with the help of a symbol server.
            // Construct a custom string to identify this pdb.
            let custom = format!("symbolserver:{}/{}/{}", debug_name, breakpad_id, debug_name);
            paths.push(CandidatePathInfo::SingleFile(FileLocation::Custom(custom)));
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
