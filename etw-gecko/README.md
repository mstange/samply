# etw-gecko

Converts from ETW logs to json consumable by https://profiler.firefox.com/.

## Setup

You need three tools for the full experience:

 1. `xperf` to record profiling data to an ETL file.
 2. This repo to convert from the ETL file to a `gecko.json`.
 3. `profiler-symbol-server` to open the profile in the profiler and to provide symbols.

You most likely already have `xperf`; it's part of the [Windows Performance Toolkit](https://docs.microsoft.com/en-us/windows-hardware/test/wpt/) which is installed with a Windows SDK.

To install `profiler-symbol-server`, run `cargo install profiler-symbol-server`.

## Usage

Open an Administrator command shell with Win+R, "cmd", Ctrl+Shift+Enter.

Start profiling session by running `xperf -on latency -stackwalk profile` in the Adminstrator shell. Then run `xperf -d out.etl` to capture it.

Then run `cargo run --release out.etl [process-name]` to produce a gecko.json.

Now set the `_NT_SYMBOL_PATH` environment variable: `set _NT_SYMBOL_PATH=srv*C:\symbols*http://msdl.microsoft.com/download/symbols*https://symbols.mozilla.org*https://chromium-browser-symsrv.commondatastorage.googleapis.com`
(use `$Env:_NT_SYMBOL_PATH = "..."` when using powershell)

Finally run `profiler-symbol-server gecko.json` to open the profile in profiler.firefox.com.

### Sampling Interval

The default sampling rate is 0.1221 ms (8192Hz). This can be set to a different value
with something like `xperf -on latency -stackwalk profile -SetProfInt 10000` for a rate
of 1ms. (The units are 100 nanoseconds)
