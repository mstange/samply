# profiler-get-symbols

The crates in this repo allow you to obtain symbol tables from ELF, Mach-O and PE
binaries as well as from pdb files. The implementation makes use of the crates
`object` and `pdb`.

The `lib` directory contains a generic Rust implementation. The `wasm` directory
contains a wrapper that targets WebAssembly and JavaScript.
The `examples` directory contains two command line tools that can be used to test
the functionality, for example as follows (executed from the workspace root):

```
cargo run -p dump-table -- firefox.pdb fixtures/win64-ci
cargo run -p query-api -- fixtures/win64-ci /symbolicate/v5 '{"jobs": [{"stacks":[[[0,204776],[0,129423],[1, 237799]]],"memoryMap":[["firefox.pdb","AA152DEB2D9B76084C4C44205044422E1"],["mozglue.pdb","63C609072D3499F64C4C44205044422E1"],["wntdll.pdb","D74F79EB1F8D4A45ABCD2F476CCABACC1"]]}]}'
```

The .wasm file and the JavaScript bindings are used by the Gecko profiler.
More specifically, they are used by the
[ProfilerGetSymbols.jsm](https://searchfox.org/mozilla-central/source/browser/components/extensions/ProfilerGetSymbols.jsm) module in Firefox. The code is run every time you use the Gecko profiler: On macOS and Linux
it is used to get symbols for native system libraries, and on all platforms it
is used if you're profiling a local build of Firefox for which there are no
symbols on the [Mozilla symbol server](https://symbols.mozilla.org/).

## Documentation

Documentation for the Rust API can be found at [https://docs.rs/profiler-get-symbols/](https://docs.rs/profiler-get-symbols/).
Documentation for the "Symbolication API" format can be found in [API.md](API.md).

## Running / Testing

### command line tools

Examples of running the `dump-table` tool:

```
cargo run -p dump-table -- firefox.pdb fixtures/win64-ci
cargo run -p dump-table -- firefox.exe fixtures/win64-ci
cargo run -p dump-table -- libmozglue.dylib fixtures/macos-local
cargo run -p dump-table -- libmozglue.dylib fixtures/macos-local INCORRECTID
cargo run -p dump-table -- libmozglue.dylib fixtures/macos-local F38030E4A3783F90B2282FCB0B33261A0
cargo run -p dump-table -- AppKit /System/Library/Frameworks/AppKit.framework/Versions/C/
cargo run -p dump-table -- libsystem_kernel.dylib /usr/lib/system
cargo run -p dump-table -- libsystem_kernel.dylib /usr/lib/system B6602BF001213894AED620A8CF2A30B80 --full
```

Examples of running the `query-api` tool:

```
cargo run -p query-api -- fixtures/win64-ci /symbolicate/v5 '{"jobs": [{"stacks":[[[0,204776],[0,129423],[1, 237799]]],"memoryMap":[["firefox.pdb","AA152DEB2D9B76084C4C44205044422E1"],["mozglue.pdb","63C609072D3499F64C4C44205044422E1"],["wntdll.pdb","D74F79EB1F8D4A45ABCD2F476CCABACC1"]]}]}'
cargo run -p query-api -- fixtures/android32-local /symbolicate/v5 '{"jobs": [{"stacks":[[[0,247618],[0,685896],[0,686768]]],"memoryMap":[["libmozglue.so","0CE47B7C29F27CED55C41233B93EBA450"]]}]}'
cargo run -p query-api -- fixtures/android32-local /symbolicate/v5 '{"jobs": [{"stacks":[[[0,247618],[0,685896],[0,686768]]],"memoryMap":[["libmozglue.so","0CE47B7C29F27CED55C41233B93EBA450"]]}]}'
cargo run -p query-api -- fixtures/android32-local /symbolicate/v5 '{"jobs": [{"stacks":[[[0,247618],[0,685896],[0,686768]]],"memoryMap":[["libmozglue.so","0CE47B7C29F27CED55C41233B93EBA45"]]}]}'
cargo run -p query-api -- fixtures/android32-local /symbolicate/v5 '{"jobs": [{"stacks":[[[0,247618],[0,685896],[0,686768]]],"memoryMap":[["lebmozglue.so","0CE47B7C29F27CED55C41233B93EBA45"]]}]}'
cargo run -p query-api -- fixtures/win64-ci /symbolicate/v5 '{"jobs": [{"stacks":[[[0,244290],[0,244219]]],"memoryMap":[["mozglue.pdb","63C609072D3499F64C4C44205044422E1"]]}]}'
cargo run -p query-api -- fixtures/macos-local /symbolicate/v5 '{"jobs": [{"stacks":[[[0,247618],[0,685896],[0,686768]]],"memoryMap":[["libmozglue.dylib","F38030E4A3783F90B2282FCB0B33261A0"]]}]}'
cargo run --release -p query-api -- ~/code/obj-m-opt/dist/bin /symbolicate/v5 @fixtures/requests/macos-local-xul.json
```

Running tests:

```
cargo test --workspace
```

Benchmarks:

```
# Download big-benchmark-fixtures directory once (multiple GB), and run all benchmarks:
cargo run --release -p benchmarks

# Run a specific benchmark (requires big-benchmark-fixtures directory):
cargo run --release -p query-api -- big-benchmark-fixtures/win64-ci/ /symbolicate/v5 @fixtures/requests/win64-ci-xul.json > /dev/null
cargo run --release -p query-api -- big-benchmark-fixtures/macos-ci/ /symbolicate/v5 @fixtures/requests/macos-ci-xul.json > /dev/null
cargo run --release -p query-api -- big-benchmark-fixtures/macos-local/ /symbolicate/v5 @fixtures/requests/macos-local-xul.json > /dev/null
# ... (see examples/benchmarks/src/main.rs for more)
```

### WebAssembly

There's a UI for the WebAssembly / JavaScript version in `index.html`.
You can use the files in the `fixtures` directory as examples.

To test, as a one-time setup, install `simple-http-server` using cargo:

```bash
cargo install simple-http-server
```

(The advantage of this over python's `SimpleHTTPServer` is that `simple-http-server` sends the correct mime type for .wasm files.)

Then start the server in this directory, by typing `simple-http-server` and pressing enter:

```bash
$ simple-http-server
     Index: disabled, Upload: disabled, Cache: enabled, Cors: disabled, Range: enabled, Sort: enabled, Threads: 3
          Auth: disabled, Compression: disabled
         https: disabled, Cert: , Cert-Password: 
          Root: /Users/mstange/code/profiler-get-symbols,
    TryFile404: 
       Address: http://0.0.0.0:8000
    ======== [2021-05-28 14:50:47] ========
```

Now you can open [http://127.0.0.1:8000/index.html](http://127.0.0.1:8000/index.html) in your browser and play with the API.

#### Updating the WebAssembly build

One-time setup:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli # --force to update
```

After a change:

```bash
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/profiler_get_symbols_wasm.wasm --out-dir . --no-modules --no-typescript
```

If this complains about wasm-bindgen version mismatches, update both your local wasm-bindgen-cli and the wasm-bindgen dependency at wasm/Cargo.toml to the latest version.

#### Importing a new version into Firefox

Firefox uses profiler-get-symbols from its [symbolication-worker.js](https://searchfox.org/mozilla-central/rev/553bf8428885dbd1eab9b63f71ef647f799372c2/devtools/client/performance-new/symbolication-worker.js). The wasm file is hosted in a Google Cloud Platform storage bucket (https://storage.googleapis.com/firefox-profiler-get-symbols/). The wasm-bindgen bindings file is checked into the mozilla-central repo, along with the wasm URL and its SRI hash.

Importing a new version of profiler-get-symbols into Firefox is done as follows:

 1. Re-generate the wasm file and the bindings file as described in the previous section.
 2. Commit the new generated files into this git repository, and push to github.
 3. Generate the SRI hash for `profiler_get_symbols_wasm_bg.wasm`: `shasum -b -a 384 profiler_get_symbols_wasm_bg.wasm | awk '{ print $1 }' | xxd -r -p | base64`
 4. Copy the file `profiler_get_symbols_wasm_bg.wasm` to a file named `<current git commit hash>.wasm`.
 5. Upload this file to the GCP bucket.
 6. In mozilla-central, update the following pieces:
   - In `symbolication.jsm.js`, update the .wasm URL, the SRI hash, and the git commit hash.
   - Copy the contents of `profiler_get_symbols_wasm.js` (in this repo) into `profiler_get_symbols.js` (in mozilla-central), leaving the header at the top intact, but updating the commit hash in that header comment.
 7. Address any API changes, if there were any since the last import.
 8. Create a mozilla-central patch with these changes.
