use std::collections::HashMap;
use std::ffi::OsStr;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Either, StreamBody};
use hyper::body::{Bytes, Frame};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{header, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use rand::RngCore;
use tokio::net::TcpListener;
use tokio_util::io::ReaderStream;
use wholesym::debugid::DebugId;
use wholesym::{LibraryInfo, SymbolManager, SymbolManagerConfig};

use crate::shared;
use crate::shared::ctrl_c::CtrlC;

#[derive(Clone, Debug)]
pub struct ServerProps {
    pub port_selection: PortSelection,
    pub verbose: bool,
    pub open_in_browser: bool,
}

#[tokio::main]
pub async fn start_server_main(
    file: &Path,
    props: ServerProps,
    libinfo_map: HashMap<(String, DebugId), LibraryInfo>,
) {
    start_server(
        Some(file),
        libinfo_map,
        props.port_selection,
        props.verbose,
        props.open_in_browser,
    )
    .await;
}

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

async fn start_server(
    profile_filename: Option<&Path>,
    libinfo_map: HashMap<(String, DebugId), LibraryInfo>,
    port_selection: PortSelection,
    verbose: bool,
    open_in_browser: bool,
) {
    let (listener, addr) = make_listener(port_selection).await;

    let token = generate_token();
    let path_prefix = format!("/{token}");
    let server_origin = format!("http://{addr}");
    let symbol_server_url = format!("{server_origin}{path_prefix}");
    let mut template_values: HashMap<&'static str, String> = HashMap::new();
    template_values.insert("SERVER_URL", server_origin.clone());
    template_values.insert("PATH_PREFIX", path_prefix.clone());

    let profiler_url = if profile_filename.is_some() {
        let profile_url = format!("{symbol_server_url}/profile.json");

        let env_profiler_override = std::env::var("PROFILER_URL").ok();
        let profiler_origin = match &env_profiler_override {
            Some(s) => s.trim_end_matches('/'),
            None => "https://profiler.firefox.com",
        };

        let encoded_profile_url = utf8_percent_encode(&profile_url, BAD_CHARS).to_string();
        let encoded_symbol_server_url =
            utf8_percent_encode(&symbol_server_url, BAD_CHARS).to_string();
        let profiler_url = format!(
            "{profiler_origin}/from-url/{encoded_profile_url}/?symbolServer={encoded_symbol_server_url}"
        );
        template_values.insert("PROFILER_URL", profiler_url.clone());
        template_values.insert("PROFILE_URL", profile_url);
        Some(profiler_url)
    } else {
        None
    };

    let template_values = Arc::new(template_values);

    let mut config = SymbolManagerConfig::new()
        .verbose(verbose)
        .respect_nt_symbol_path(true)
        .use_debuginfod(std::env::var("SAMPLY_USE_DEBUGINFOD").is_ok())
        .use_spotlight(true);

    if let Some(home_dir) = dirs::home_dir() {
        config = config.debuginfod_cache_dir_if_not_installed(home_dir.join("sym"));
    }
    // TODO: Read breakpad symbol server config from some kind of config file, and call breakpad_symbols_server
    // TODO: On Windows, put https://msdl.microsoft.com/download/symbols into the config file.
    // There's a privacy tradeoff here; some people may not want library names and debug IDs to be sent to Microsoft servers.
    //     .default_nt_symbol_path("srv**https://msdl.microsoft.com/download/symbols")

    let mut symbol_manager = SymbolManager::with_config(config);
    for lib_info in libinfo_map.into_values() {
        symbol_manager.add_known_library(lib_info);
    }

    if let Some(profile_filename) = profile_filename {
        let precog_filename = profile_filename.with_extension("syms.json");
        if let Some(precog_info) =
            shared::symbol_precog::PrecogSymbolInfo::try_load(&precog_filename)
        {
            for (debug_id, syms) in precog_info.into_hash_map().into_iter() {
                let lib_info = LibraryInfo {
                    debug_id: Some(debug_id),
                    ..LibraryInfo::default()
                };
                symbol_manager.add_known_library_symbols(lib_info, syms);
            }
        }
    }

    let symbol_manager = Arc::new(symbol_manager);

    let server = tokio::task::spawn(run_server(
        listener,
        symbol_manager,
        profile_filename.map(PathBuf::from),
        template_values,
        path_prefix,
    ));

    eprintln!("Local server listening at {server_origin}");
    if !open_in_browser {
        if let Some(profiler_url) = &profiler_url {
            eprintln!("  Open the profiler at {profiler_url}");
        }
    }
    eprintln!("Press Ctrl+C to stop.");

    if open_in_browser {
        if let Some(profiler_url) = &profiler_url {
            let _ = opener::open_browser(profiler_url);
        }
    }

    // Run this server until it stops.
    if let Err(e) = server.await {
        eprintln!("server error: {e}");
    }
}

// Returns a base32 string for 24 random bytes.
fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    nix_base32::to_nix_base32(&bytes)
}

async fn make_listener(port_selection: PortSelection) -> (TcpListener, SocketAddr) {
    match port_selection {
        PortSelection::OnePort(port) => {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            match TcpListener::bind(&addr).await {
                Ok(listener) => (listener, addr),
                Err(e) => {
                    eprintln!("Could not bind to port {port}: {e}");
                    std::process::exit(1)
                }
            }
        }
        PortSelection::TryMultiple(range) => {
            let mut error = None;
            for port in range.clone() {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                match TcpListener::bind(&addr).await {
                    Ok(listener) => return (listener, addr),
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
            }
            match error {
                Some(error) => {
                    eprintln!("Could not bind to any port in the range {range:?}: {error}",);
                }
                None => {
                    eprintln!("Binding failed, port range empty? {range:?}");
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

async fn run_server(
    listener: TcpListener,
    symbol_manager: Arc<SymbolManager>,
    profile_filename: Option<PathBuf>,
    template_values: Arc<HashMap<&'static str, String>>,
    path_prefix: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut ctrl_c_receiver = CtrlC::observe_oneshot();

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = tokio::select! {
            stream_and_addr_res = listener.accept() => stream_and_addr_res?,
            ctrl_c_result = &mut ctrl_c_receiver => {
                return Ok(ctrl_c_result?);
            }
        };

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        let symbol_manager = symbol_manager.clone();
        let profile_filename = profile_filename.clone();
        let template_values = template_values.clone();
        let path_prefix = path_prefix.clone();

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(
                    io,
                    service_fn(move |req| {
                        symbolication_service(
                            req,
                            template_values.clone(),
                            symbol_manager.clone(),
                            profile_filename.clone(),
                            path_prefix.clone(),
                        )
                    }),
                )
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn symbolication_service(
    req: Request<hyper::body::Incoming>,
    template_values: Arc<HashMap<&'static str, String>>,
    symbol_manager: Arc<SymbolManager>,
    profile_filename: Option<PathBuf>,
    path_prefix: String,
) -> Result<Response<Either<String, BoxBody<Bytes, std::io::Error>>>, hyper::Error> {
    let has_profile = profile_filename.is_some();
    let method = req.method();
    let path = req.uri().path();
    let mut response = Response::new(Either::Left(String::new()));

    let Some(path_without_prefix) = path.strip_prefix(&path_prefix) else {
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
                    Either::Left(substitute_template(template, &template_values));
            }
            _ => {
                *response.status_mut() = StatusCode::NOT_FOUND;
            }
        }
        return Ok(response);
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

            // Stream the file. This follows the send_file example from the hyper repo.
            // https://github.com/hyperium/hyper/blob/7206fe30302937075c51c16a69d1eb3bbce6a671/examples/send_file.rs
            let file = tokio::fs::File::open(&profile_filename)
                .await
                .expect("couldn't open profile file");

            // Wrap in a tokio_util::io::ReaderStream
            let reader_stream = ReaderStream::new(file);

            let stream_body = StreamBody::new(reader_stream.map_ok(Frame::data));
            *response.body_mut() = Either::Right(stream_body.boxed());
        }
        (&Method::POST, path, _) => {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            let path = path.to_string();
            // Await the full body to be concatenated into a `Collected<Bytes>`.
            let full_body = req.into_body().collect().await?;
            // Convert the `Collected<Bytes>` into a `String`.
            let full_body =
                String::from_utf8(full_body.to_bytes().to_vec()).expect("invalid utf-8");
            let response_json = symbol_manager.query_json_api(&path, &full_body).await;

            *response.body_mut() = Either::Left(response_json);
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
