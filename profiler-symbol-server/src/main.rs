use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Request, Response, Server};
use hyper::{Method, StatusCode};
use memmap::MmapOptions;
use moria;
use profiler_get_symbols::query_api;
use profiler_get_symbols::{self, FileAndPathHelper, FileAndPathHelperResult, OwnedFileData};
use serde_json::{self, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    // Open the file in read-only mode with buffer.
    let file = File::open("/Users/mstange/code/profiler-get-symbols/profile.json")
        .expect("couldn't open file");
    let reader = BufReader::new(file);

    // Read the JSON contents of the file as an instance of `User`.
    let p: Value = serde_json::from_reader(reader).expect("couldn't parse json");

    // We'll bind to 127.0.0.1:3000
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    // A `Service` is needed for every connection, so this
    // creates one from our `hello_world` function.

    let helper = Arc::new(Helper::for_profile(p));
    let make_svc = make_service_fn(move |_conn| {
        let helper2 = helper.clone();
        async {
            // service_fn converts our function into a `Service`
            Ok::<_, Infallible>(service_fn(move |req| hello_world(req, helper2.clone())))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn hello_world(
    req: Request<Body>,
    helper: Arc<Helper>,
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

impl Helper {
    pub fn for_profile(profile: Value) -> Self {
        // Build a map (debugName, breakpadID) -> debugPath from the information
        // in profile.libs.
        let path_map = if let Value::Array(libs) = &profile["libs"] {
            libs.iter()
                .map(|l| {
                    (
                        (
                            l["debugName"].as_str().unwrap().to_string(),
                            l["breakpadId"].as_str().unwrap().to_string(),
                        ),
                        l["debugPath"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };
        Helper { path_map }
    }
}

impl FileAndPathHelper for Helper {
    type FileContents = MmapFileContents;

    fn get_candidate_paths_for_binary_or_pdb(
        &self,
        debug_name: &str,
        breakpad_id: &str,
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Vec<PathBuf>>> + Send>> {
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
                if let Ok(dsym_path) = moria::locate_dsym(&path, uuid) {
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
    ) -> Pin<Box<dyn Future<Output = FileAndPathHelperResult<Self::FileContents>> + Send>> {
        async fn read_file_impl(path: PathBuf) -> FileAndPathHelperResult<MmapFileContents> {
            eprintln!("Reading file {:?}", &path);
            let file = File::open(&path)?;
            Ok(MmapFileContents(unsafe { MmapOptions::new().map(&file)? }))
        }

        Box::pin(read_file_impl(path.to_owned()))
    }
}
