# samply-api

This crate implements a JSON API for profiler symbolication with the help of
local symbol files. It exposes a single function `query_api`, and uses the
`samply-symbols` crate for its implementation.

The API is documented in [API.md](../API.md).

Just like the `samply-symbols` crate, this crate does not contain any direct
file access. It is written in such a way that it can be compiled to
WebAssembly, with all file access being mediated via a `FileAndPathHelper`
trait.

## Example

```rust
use samply_api::samply_symbols::{
    FileContents, FileAndPathHelper, FileAndPathHelperResult, LibraryInfo, OptionallySendFuture,
    CandidatePathInfo, FileLocation, SymbolManager,
};
use samply_api::samply_symbols::debugid::DebugId;

async fn run_query() -> String {
    let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let helper = ExampleHelper {
        artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci")
    };
    let symbol_manager = SymbolManager::with_helper(&helper);
    let api = samply_api::Api::new(&symbol_manager);

    api.query_api(
        "/symbolicate/v5",
        r#"{
            "memoryMap": [
              [
                "firefox.pdb",
                "AA152DEB2D9B76084C4C44205044422E1"
              ]
            ],
            "stacks": [
              [
                [0, 204776],
                [0, 129423],
                [0, 244290],
                [0, 244219]
              ]
            ]
          }"#,
    ).await
}

struct ExampleHelper {
    artifact_directory: std::path::PathBuf,
}

impl<'h> FileAndPathHelper<'h> for ExampleHelper {
    type F = Vec<u8>;
    type FL = ExampleFileLocation;
    type OpenFileFuture = std::pin::Pin<
        Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
    >;

    fn get_candidate_paths_for_debug_file(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<ExampleFileLocation>>> {
        if let Some(debug_name) = library_info.debug_name.as_deref() {
            Ok(vec![CandidatePathInfo::SingleFile(ExampleFileLocation(
                self.artifact_directory.join(debug_name),
            ))])
        } else {
            Ok(vec![])
        }
    }

    fn get_candidate_paths_for_binary(
        &self,
        library_info: &LibraryInfo,
    ) -> FileAndPathHelperResult<Vec<CandidatePathInfo<ExampleFileLocation>>> {
        if let Some(name) = library_info.name.as_deref() {
            Ok(vec![CandidatePathInfo::SingleFile(ExampleFileLocation(
                self.artifact_directory.join(name),
            ))])
        } else {
            Ok(vec![])
        }
    }

   fn get_dyld_shared_cache_paths(
       &self,
       _arch: Option<&str>,
   ) -> FileAndPathHelperResult<Vec<ExampleFileLocation>> {
       Ok(vec![])
   }

    fn load_file(
        &'h self,
        location: ExampleFileLocation,
    ) -> std::pin::Pin<
        Box<dyn OptionallySendFuture<Output = FileAndPathHelperResult<Self::F>> + 'h>,
    > {
        async fn load_file_impl(path: std::path::PathBuf) -> FileAndPathHelperResult<Vec<u8>> {
            Ok(std::fs::read(&path)?)
        }

        Box::pin(load_file_impl(location.0))
    }
}

#[derive(Clone, Debug)]
struct ExampleFileLocation(std::path::PathBuf);

impl std::fmt::Display for ExampleFileLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.to_string_lossy().fmt(f)
    }
}

impl FileLocation for ExampleFileLocation {
    fn location_for_dyld_subcache(&self, suffix: &str) -> Option<Self> {
        let mut filename = self.0.file_name().unwrap().to_owned();
        filename.push(suffix);
        Some(Self(self.0.with_file_name(filename)))
    }

    fn location_for_external_object_file(&self, object_file: &str) -> Option<Self> {
        Some(Self(object_file.into()))
    }

    fn location_for_pdb_from_binary(&self, pdb_path_in_binary: &str) -> Option<Self> {
        Some(Self(pdb_path_in_binary.into()))
    }

    fn location_for_source_file(&self, source_file_path: &str) -> Option<Self> {
        Some(Self(source_file_path.into()))
    }
}
```
