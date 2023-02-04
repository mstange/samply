# samply-symbols

This crate allows obtaining symbol information from binaries and compilation artifacts.

You probably want to be using the [`wholesym` crate](https://docs.rs/wholesym/) instead.
`wholesym` has a much more ergonomic API; it is a wrapper around `samply-symbols`.

More specifically, `samply-symbols` provides the low-level implementation of `wholesym`,
while satisfying both native and WebAssembly consumers, whereas `wholesym` only cares about
native consumers.

The main entry point of this crate is the `SymbolManager` struct and its async `load_symbol_map` method.
With a `SymbolMap`, you can resolve raw code addresses to function name strings, and, if available,
to file name + line number information and inline stacks.

# Design constraints

This crate operates under the following design constraints:

  - Must be usable from JavaScript / WebAssembly: The Firefox profiler runs this code in a
    WebAssembly environment, invoked from a privileged piece of JavaScript code inside Firefox itself.
    This setup allows us to download a wasm bundle on demand, rather than shipping
    it with Firefox, which would increase the Firefox download size for a piece of functionality
    that the vast majority of Firefox users don't need.
  - Performance: We want to be able to obtain symbol data from a fresh build of a locally compiled
    Firefox instance as quickly as possible, without an expensive preprocessing step. The time between
    "finished compilation" and "returned symbol data" should be minimized. This means that symbol
    data needs to be obtained directly from the compilation artifacts rather than from, say, a
    dSYM bundle or a Breakpad .sym file.
  - Must scale to large inputs: This applies to both the size of the API request and the size of the
    object files that need to be parsed: The Firefox profiler will supply anywhere between tens of
    thousands and hundreds of thousands of different code addresses in a single symbolication request.
    Firefox build artifacts such as libxul.so can be multiple gigabytes big, and contain around 300000
    function symbols. We want to serve such requests within a few seconds or less.
  - "Best effort" basis: If only limited symbol information is available, for example from system
    libraries, we want to return whatever limited information we have.

The WebAssembly requirement means that this crate cannot contain any direct file access.
Instead, all file access is mediated through a `FileAndPathHelper` trait which has to be implemented
by the caller. We cannot even use the `std::path::Path` / `PathBuf` types to represent paths,
because the WASM bundle can run on Windows, and the `Path` / `PathBuf` types have! Unix path
semantics in Rust-compiled-to-WebAssembly.

Furthermore, the caller needs to be able to find the right symbol files based on a subset
of information about a library, for example just based on its debug name and debug ID. This
is used when `SymbolManager::load_symbol_map` is called with such a subset of information.
More concretely, this ability is used by `samply-api` when processing a JSON symbolication
API call, which only comes with the debug name and debug ID for a library.

# Supported formats and data

This crate supports obtaining symbol data from PE binaries (Windows), PDB files (Windows),
mach-o binaries (including fat binaries) (macOS & iOS), and ELF binaries (Linux, Android, etc.).
For mach-o files it also supports finding debug information in external objects, by following
OSO stabs entries.
It supports gathering both basic symbol information (function name strings) as well as information
based on debug data, i.e. inline callstacks where each frame has a function name, a file name,
and a line number.
For debug data we support both DWARF debug data (inside mach-o and ELF binaries) and PDB debug data.

# Example

```rust
use samply_symbols::debugid::DebugId;
use samply_symbols::{
    CandidatePathInfo, FileAndPathHelper, FileAndPathHelperResult, FileLocation,
    FramesLookupResult, LibraryInfo, OptionallySendFuture, SymbolManager,
};

async fn run_query() {
    let this_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let helper = ExampleHelper {
        artifact_directory: this_dir.join("..").join("fixtures").join("win64-ci"),
    };

    let symbol_manager = SymbolManager::with_helper(&helper);

    let library_info = LibraryInfo {
        debug_name: Some("firefox.pdb".to_string()),
        debug_id: DebugId::from_breakpad("AA152DEB2D9B76084C4C44205044422E1").ok(),
        ..Default::default()
    };
    let symbol_map = match symbol_manager.load_symbol_map(&library_info).await {
        Ok(symbol_map) => symbol_map,
        Err(e) => {
            println!("Error while loading the symbol map: {:?}", e);
            return;
        }
    };

    // Look up the symbol for an address.
    let lookup_result = symbol_map.lookup_relative_address(0x1f98f);

    match lookup_result {
        Some(address_info) => {
            // Print the symbol name for this address:
            println!("0x1f98f: {}", address_info.symbol.name);

            // See if we have debug info (file name + line, and inlined frames):
            match address_info.frames {
                FramesLookupResult::Available(frames) => {
                    println!("Debug info:");
                    for frame in frames {
                        println!(
                            " - {:?} ({:?}:{:?})",
                            frame.function, frame.file_path, frame.line_number
                        );
                    }
                }
                FramesLookupResult::External(ext_address) => {
                    // Debug info is located in a different file.
                    if let Some(frames) =
                        symbol_manager.lookup_external(&symbol_map.debug_file_location(), &ext_address).await
                    {
                        println!("Debug info:");
                        for frame in frames {
                            println!(
                                " - {:?} ({:?}:{:?})",
                                frame.function, frame.file_path, frame.line_number
                            );
                        }
                    }
                }
                FramesLookupResult::Unavailable => {}
            }
        }
        None => {
            println!("No symbol was found for address 0x1f98f.")
        }
    }
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

    fn location_for_breakpad_symindex(&self) -> Option<Self> {
        Some(Self(self.0.with_extension("symindex")))
    }
}
```