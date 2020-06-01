use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Request, Response, Server};
use hyper::{Method, StatusCode};
use memmap::MmapOptions;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use profiler_get_symbols::query_api;
use profiler_get_symbols::{
    self, FileAndPathHelper, FileAndPathHelperResult, OptionallySendFuture, OwnedFileData,
};
use serde_json::{self, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;

mod moria_mac;

#[cfg(target_os = "macos")]
mod moria_mac_spotlight;

const BAD_CHARS: &AsciiSet = &CONTROLS.add(b':').add(b'/');

pub async fn start_server(file: &Path, open_in_browser: bool) {
    // Read the profile.json file and parse it as JSON. TODO: allow specifying the file on the command line
    let mut file = File::open(file).expect("couldn't open file");
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).expect("couldn't read file");
    let profile: Value = serde_json::from_slice(&buffer).expect("couldn't parse json");
    let buffer = Arc::new(buffer);

    // We'll bind to 127.0.0.1:3000
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let helper = Arc::new(Helper::for_profile(profile));
    let new_service = make_service_fn(move |_conn| {
        let helper = helper.clone();
        let buffer = buffer.clone();
        async {
            Ok::<_, Infallible>(service_fn(move |req| {
                symbolication_service(req, helper.clone(), buffer.clone())
            }))
        }
    });

    let server = Server::bind(&addr).serve(new_service);

    let profile_url = "http://127.0.0.1:3000/profile.json";
    // let profiler_url_prefix = "https://profiler.firefox.com/from-url/";
    let profiler_url_prefix = "https://deploy-preview-2576--perf-html.netlify.app/from-url/"; // TODO: come up with a proper fix
    let encoded_profile_url = utf8_percent_encode(profile_url, BAD_CHARS).to_string();
    let url = format!("{}{}", profiler_url_prefix, encoded_profile_url);

    eprintln!("Serving symbolication server at http://127.0.0.1:3000");
    eprintln!("  The profile is at http://127.0.0.1:3000/profile.json");
    eprintln!("  Symbols can be obtained by posting to");
    eprintln!("    http://127.0.0.1:3000/symbolicate/v5 or");
    eprintln!("    http://127.0.0.1:3000/symbolicate/v6a1");
    eprintln!("  Open the profiler at");
    eprintln!("    {}", url);
    eprintln!("Press Ctrl+C to abort.");
    eprintln!("");

    if open_in_browser {
        let mut cmd = Command::new("open");
        let _ = cmd.arg(&url).status();
    }

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn symbolication_service(
    req: Request<Body>,
    helper: Arc<Helper>,
    buffer: Arc<Vec<u8>>,
) -> Result<Response<Body>, hyper::Error> {
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("*"),
    );

    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            *response.body_mut() = Body::from("Try POSTing data to /symbolicate/v5");
        }
        (&Method::GET, "/profile.json") => {
            *response.body_mut() = Body::from((*buffer).clone());
        }
        (&Method::POST, path) => {
            let path = path.to_string();
            // Await the full body to be concatenated into a single `Bytes`...
            let full_body = hyper::body::to_bytes(req.into_body()).await?;
            let full_body = String::from_utf8(full_body.to_vec()).expect("invalid utf-8");
            let response_json = query_api(&path, &full_body, &*helper).await;

            *response.body_mut() = response_json.clone().into();
        }
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    };

    Ok(response)
}

struct MmapFileContents(memmap::Mmap);

impl OwnedFileData for MmapFileContents {
    fn get_data(&self) -> &[u8] {
        &*self.0
    }
}

struct Helper {
    path_map: HashMap<(String, String), String>,
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
    pub fn for_profile(profile: Value) -> Self {
        // Build a map (debugName, breakpadID) -> debugPath from the information
        // in profile.libs.
        let mut path_map = HashMap::new();
        add_to_path_map_recursive(&profile, &mut path_map);
        Helper { path_map }
    }
}

impl FileAndPathHelper for Helper {
    type FileContents = MmapFileContents;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Vec<PathBuf>>>>> {
        async fn to_future(
            res: FileAndPathHelperResult<Vec<PathBuf>>,
        ) -> FileAndPathHelperResult<Vec<PathBuf>> {
            res
        }

        let mut paths = vec![];

        // Look up (debugName, breakpadId) in the path map.
        if let Some(path) = self
            .path_map
            .get(&(debug_name.to_string(), breakpad_id.to_string()))
        {
            // First, see if we can find a dSYM file for the binary.
            if let Ok(uuid) = Uuid::parse_str(&breakpad_id[0..32]) {
                if let Ok(dsym_path) = moria_mac::locate_dsym(&path, uuid) {
                    paths.push(dsym_path.clone());
                    paths.push(
                        dsym_path
                            .join("Contents")
                            .join("Resources")
                            .join("DWARF")
                            .join(debug_name),
                    );
                }
            }
            // Fall back to getting symbols from the binary itself.
            paths.push(path.into());
        }

        Box::pin(to_future(Ok(paths)))
    }

    fn read_file(
        &self,
        path: &Path,
    ) -> Pin<Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::FileContents>>>>
    {
        async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<MmapFileContents> {
            eprintln!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
        }

        Box::pin(read_file_impl(path.to_owned()))
    }
}
