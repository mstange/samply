# profiler-get-symbols

This repo contains a WebAssembly wrapper which allows dumping symbol tables from
ELF and Mach-O binaries as well as from pdb files. It is a relatively thin
wrapper around the crates `object`, `goblin` and `pdb`.

The resulting .wasm file is used by the Gecko profiler; more specifically, it is
used by the [ProfilerGetSymbols.jsm](https://searchfox.org/mozilla-central/source/browser/components/extensions/ProfilerGetSymbols.jsm) module in Firefox. The code is run every time you use the Gecko profiler: On macOS and Linux
it is used to get symbols for native system libraries, and on all platforms it
is used if you're profiling a local build of Firefox for which there are no
symbols on the [Mozilla symbol server](https://symbols.mozilla.org/).


## Building

One-time setup:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli # --force to update
```

On changes:

```bash
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/profiler_get_symbols.wasm --out-dir . --no-modules --no-typescript
shasum -b -a 384 profiler_get_symbols_bg.wasm | awk '{ print $1 }' | xxd -r -p | base64 # This is your SRI hash, update it in index.html
```

## Running / Testing

This repo contains a minimal `index.html` which lets you test the resulting wasm
module manually in the browser. However, you need a file to test it on; this
repo does not contain a test binary.

To test, as a one-time setup, install http-server using cargo:

```bash
cargo install http-server
```

(The advantage of this over python's `SimpleHTTPServer` is that `http-server` sends the correct mime type for .wasm files.)

Then start the server in this directory, by typing `http-server` and pressing enter:

```bash
$ http-server
Starting up http-server, serving ./
Available on:
  http://0.0.0.0:8080
Hit CTRL-C to stop the server
```

Now:

 1. Open [http://0.0.0.0:8080](http://0.0.0.0:8080) in your browser.
 2. Use the file inputs to select your binary:
    - For Windows binaries, put the exe file in the first field and the pdb file in the second field.
    - For Mach-O and ELF binaries, load that same binary into both fields.
 3. Enter the breakpad ID for the binary into the text field. (I promise to add a better explanation for this step in the future.)
 4. Hit the button.
 5. Open the web console in your browser's devtools. There should be some numbers there.

Unfortunately the symbols are not printed in a very human-readable format currently.
Instead, the symbol table is output in the [`SymbolTableAsTuple` format](https://github.com/firefox-devtools/profiler/blob/40a56a1f305bd8726fa366b72a43287261a254a8/src/profile-logic/symbol-store-db.js#L17-L40),
which is the format that the Firefox profiler front-end expects.

## Publishing

At the moment, the resulting wasm files are hosted in a separate repo called
[`profiler-assets`](https://github.com/mstange/profiler-assets/), in the
[`assets/wasm` directory](https://github.com/mstange/profiler-assets/tree/master/assets/wasm).
The filename of each of those wasm file is the same as its SRI hash value, but expressed in hexadecimal
instead of base64. Here's a command which creates a file with such a name from your `profiler_get_symbols_bg.wasm`:

```bash
cp profiler_get_symbols_bg.wasm `shasum -b -a 384 profiler_get_symbols_bg.wasm | awk '{ print $1 }'`.wasm
```
