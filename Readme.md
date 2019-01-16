# profiler-get-symbols

This repo contains a WebAssembly wrapper for the object and goblin crates that
allows dumping symbol tables from ELF and Mach-O binaries.

The resulting .wasm file is going to be used by the Gecko profiler.

## Building

```bash
$ rustup default nightly
$ cargo build --target wasm32-unknown-unknown --release
$ wasm-bindgen target/wasm32-unknown-unknown/release/profiler_get_symbols.wasm --out-dir . --no-modules --no-typescript
$ cp profiler_get_symbols_bg.wasm `shasum -b -a 384 profiler_get_symbols_bg.wasm | awk '{ print $1 }'`.wasm
$ shasum -b -a 384 profiler_get_symbols_bg.wasm | awk '{ print $1 }' | xxd -r -p | base64 # This is your SRI hash
```
