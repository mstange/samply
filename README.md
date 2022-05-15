# fxprof-perf-convert

A converter from the Linux perf `perf.data` format into the [Firefox Profiler](https://profiler.firefox.com/) format, specifically into the [Processed profile format](https://crates.io/crates/fxprof-processed-profile).

## Run

```
% # Install profiler-symbol-server once:
% cargo install profiler-symbol-server
%
% # Convert the file and open the profiler:
% cargo run --release -- perf.data
% profiler-symbol-server profile-conv.json
```
