# samply-symbols

This crate allows obtaining symbol information from binaries and compilation artifacts.

You probably want to be using the [`wholesym` crate](https://docs.rs/wholesym/) instead.
`wholesym` has a much more ergonomic API; it is a wrapper around `samply-symbols`.

More specifically, `samply-symbols` provides the low-level implementation of `wholesym`,
while satisfying both native and WebAssembly consumers, whereas `wholesym` only cares about
native consumers.

The main entry points of this crate are the sans-IO state machines exposed by
`LoadSymbolMap`, `LoadBinary`, `LoadSourceFile`, `LoadExternalFile`, and `LookupQuery`.
With a `SymbolMap`, you can resolve raw code addresses to function name strings, and,
if available, to file name + line number information and inline stacks.

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
Instead, the state machines surface "I need this file" requests, and a driver in the
caller's environment fetches the bytes and feeds them back in. The caller pairs its file
contents type and file location type behind the `FileTypes` trait — a pure type
bundle with no methods.

We cannot even use the `std::path::Path` / `PathBuf` types to represent paths, because the
WASM bundle can run on Windows, and the `Path` / `PathBuf` types have Unix path semantics
in Rust-compiled-to-WebAssembly. Callers therefore use their own `FileLocation`
implementation to model paths in a way that suits their environment.

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

`samply_symbols` is sans-IO: each entry-point state machine surfaces "I need this file"
requests as plain values via `LoadStep` (or `SymbolMapLoadStep`). The caller is
responsible for fetching the requested bytes (sync, async, threadpool — its choice)
and feeding the result back in via `provide`. For an end-to-end driver, see
[`wholesym`](https://docs.rs/wholesym).
