# Releases

<!-- next-header -->

## Unreleased - ReleaseDate

### Breaking changes for library consumers

The `samply-symbols` and `samply-api` crates are now sans-IO. They no longer
contain any `async fn`, `.await`, `Future` returns, or async wrappers. Instead,
they expose state machines whose `poll → provide` interface a driver
satisfies. `wholesym` continues to provide an async API on top of these state
machines and is the recommended entry point for most callers.

Concretely:

  - `FileAndPathHelper` is now a pure type bundle: it has only the two
    associated types `F: FileContents` and `FL: FileLocation` and no methods.
    All file-loading and candidate-path methods (`load_file`,
    `get_candidate_paths_for_debug_file`,
    `get_candidate_paths_for_binary`, `get_dyld_shared_cache_paths`,
    `get_candidate_paths_for_gnu_debug_link_dest`,
    `get_candidate_paths_for_supplementary_debug_file`,
    `get_symbol_map_for_library`) are removed and live entirely in the driver
    layer (e.g. `wholesym::FileResolver`).
  - `OptionallySendFuture` and the `send_futures` cargo feature are removed.
  - `samply_symbols::SymbolManager` is removed entirely. Construct the
    sans-IO state machines directly (`LoadSymbolMap::new`, `LoadBinary::new`,
    `LoadSourceFile::new`, `LoadExternalFile::new`). The candidate-iterating
    state machines (`LoadSymbolMapForLibraryInfo`, `LoadBinaryForLibraryInfo`,
    `Load{SymbolMap,Binary}ForDyldCacheImage`) are removed too; equivalent
    logic now lives in `wholesym` as plain async helpers.
  - `LoadSymbolMap::new` no longer takes a `helper: Arc<H>`; it takes only the
    file location and the multi-arch disambiguator.
  - There are two step enums: `LoadStep` (used by `LoadBinary`, `LoadSourceFile`,
    `LoadExternalFile`, `LookupQuery`, `DyldCacheLoad`) only has `NeedFile`
    and `Done` variants. `SymbolMapLoadStep` (used by `LoadSymbolMap` and
    `ElfLoad`) additionally has `NeedDebugLinkCandidates` and
    `NeedSupplementaryCandidates`, which surface the previous mid-load
    `get_candidate_paths_for_*` calls. The corresponding state machines have a
    new `provide_candidates(Vec<FL>)` method.
  - `SymbolMap::lookup` and `SymbolMap::lookup_external` (the async ones) are
    removed. Use `LookupQuery::for_address` / `LookupQuery::for_external` and
    drive the resulting state machine via your own helper.
  - `samply-api`'s `Api::query_api` is replaced by `Api::build_query`, which
    returns a `Box<dyn ApiQueryState<H> + Send>` for the driver to run.

`wholesym`'s public API is unchanged.

## 0.13.1 - 2025-02-01

## 0.13.0 - 2025-02-01

This release adds Windows support. It uses ETW via `xperf` to record the system activity to an ETL file. Then samply converts the ETL file.

Samply asks for Adminstrator privileges during profiling. This is necessary for ETW to work.

Thanks to @jrmuizel for getting this off the ground. Most of the Windows implementation was initially written by him. ETW is rather lightly documented, so this required a lot of research.

Also thanks to @vvuk, who integrated Jeff's code into samply and contributed hugely to getting this ready for production!

And thanks to the authors of the https://github.com/n4r1b/ferrisetw crate; samply uses etw-reader which started out as a fork of ferrisetw.

Known issues:

 - By default, you won't get Windows symbols, but you can use `samply record --windows-symbol-server https://msdl.microsoft.com/download/symbols` to fix this - this will download symbols for Windows system libraries and kernel stacks from Microsoft's server. I'm planning to add a config file for samply so that symbol servers can be configured more permanently, but it doesn't exist yet.
 - Missing symbols for precompiled .NET code: This is [getsentry/pdb#153](https://github.com/getsentry/pdb/issues/153), which has a potential patch in [getsentry/pdb#154](https://github.com/getsentry/pdb/pull/154).
 - CoreCLR support could be better - some of it isn't working correctly any more (see [#483](https://github.com/mstange/samply/issues/483))

### Breaking changes

 - The minimum supported Rust version is now 1.77.

### Features

 - Windows: Initial support.
 - macOS: Support attaching to running processes and their subprocesses ([#190](https://github.com/mstange/samply/pull/190), by @vvuk, and [#425](https://github.com/mstange/samply/pull/425), by @tmm1)
 - macOS: Add `samply setup` to code-sign samply so that attaching to running processes can work ([#217](https://github.com/mstange/samply/pull/217) + [#353](https://github.com/mstange/samply/pull/353), by @vvuk)
 - All platforms: `samply import` has much better support for Android simpleperf now
 - All platforms: Add `--main-thread-only` flag
 - All platforms: Add `--include-args` argument
 - Windows, Linux: Add `--per-cpu-threads` flag
 - All platforms: Add `--symbol-dir`, `--windows-symbol-server`, `--windows-symbol-cache`, `--breakpad-symbol-server`, `--breakpad-symbol-dir`, `--breakpad-symbol-cache`, and `--simpleperf-binary-cache` arguments (various PRs, including some by @ishitatsuyuki)
 - All platforms: Add `--address` option to specify the IP address at which the local server is listening ([#234](https://github.com/mstange/samply/pull/234), by @Rjected)
 - All platforms: Add `--unstable-presymbolicate` flag ([#202](https://github.com/mstange/samply/pull/202), by @vvuk)

### Fixes

 - Fix build errors related to `zerocopy` and `zerocopy_derive` ([#356](https://github.com/mstange/samply/pull/356), by @mox692)
 - macOS: Fix library enumeration on macOS 15 Sequoia ([#403](https://github.com/mstange/samply/pull/403), by @Maaarcocr)

## 0.12.0 - 2024-04-16

### Breaking changes

 - The minimum supported Rust version is now 1.74.
 - `samply load perf.data` is now called `samply import perf.data`.
 - The `--port` alias has changed from `-p` to `-P`.

### Features

 - Linux: Allow attaching to running processes with `samply record -p [pid]` ([#18](https://github.com/mstange/samply/pull/18), by @ishitatsuyuki)
 - Linux, macOS: Support Jitdump in `samply record`.
 - Linux: Support Jitdump in `samply import perf.data` without `perf inject --jit`.
 - Linux, macOS: Support `/tmp/perf-[pid].map`([#34](https://github.com/mstange/samply/pull/34) + [#36](https://github.com/mstange/samply/pull/36), by @bnjbvr)
 - Linux, macOS: Support specifying environment variables after `samply record`.
 - Linux, macOS: Add `--iteration-count` and`--reuse-threads` flags to `samply record`.
 - Linux: Support symbolication with `.dwo` and `.dwp` files.
 - Linux: Support unwinding and symbolicating VDSO frames.
 - Linux, macOS: Support overwriting the launched browser with `$BROWSER` ([#50](https://github.com/mstange/samply/pull/50), by @ishitatsuyuki)
 - Linux, macOS: Add `--profile-name` argument to `samply record` and `samply import` to allow overriding the profile name ([#68](https://github.com/mstange/samply/pull/68), by @rukai)
 - Linux, macOS: Support Scala Native demangling ([#109](https://github.com/mstange/samply/pull/109), by @keynmol)
 - macOS: Support `--main-thread-only` in `samply record`, for lower-overhead sampling
 - macOS, Linux: Unstable support for adding markers from `marker-[pid].txt` files which are opened (and, on Linux, mmap'ed) during profiling.
 - Linux: Support kernel symbols when importing `perf.data` files with kernel stacks, if `/proc/sys/kernel/kptr_restrict` is `0`.
 - Android: Support importing `perf.data` files recorded with simpleperf's `--trace-offcpu` flag.

### In progress

 - Linux: Groundwork to support profiling Wine apps (by @ishitatsuyuki)

### Fixes

 - Linux, macOS: Don't discard information from processes with reused process IDs (e.g. due to exec).
 - Linux: Support recording on more types of machines, by falling back to software perf events in more cases. ([#70](https://github.com/mstange/samply/pull/70), by @rkd-msw)
 - Linux: Fix out-of-order samples. ([#30](https://github.com/mstange/samply/pull/30) + [#62](https://github.com/mstange/samply/pull/62), by @ishitatsuyuki)
 - Linux: Fix unwinding and symbolicating in processes which have forked without exec.
 - Linux: Capture startup work of launched processes more reliably.
 - Linux: Fix debuglink symbolication in certain cases. ([#38](https://github.com/mstange/samply/pull/38), by @zecakeh)
 - Linux: Fix stackwalking if unwinding information is stored in compressed `.debug_frame` sections. ([#10](https://github.com/mstange/samply/pull/10), by @bobrik)
 - macOS: Fix symbolication of system libraries on x86_64 macOS 13+.
 - Android: Allow building samply for Android. ([#76](https://github.com/mstange/samply/pull/76), by @flxo)
 - macOS: Fix Jitdump symbolication for functions which were JITted just before the sample was taken ([#128](https://github.com/mstange/samply/pull/128), by @vvuk)
 - macOS, Linux: More reliable handling of Ctrl+C during profiling.
 - macOS: Support recording workloads with deep recursion by eliding the middle of long stacks and not running out of memory.
 - x86_64: Improve disassembly of relative jumps by displaying the absolute target address ([#54](https://github.com/mstange/samply/pull/54), by @jrmuizel)
 - macOS: Use yellow instead of blue, for consistency with Linux which uses yellow for user stacks and orange for kernel stacks.

### Other

 - Improve build times by using the separate serde-derive crate ([#65](https://github.com/mstange/samply/pull/65), by @CryZe)

## 0.11.0 (2023-01-06)
