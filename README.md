# fxprof-perf-convert

A converter from the Linux perf `perf.data` format into the [Firefox Profiler](https://profiler.firefox.com/) format, specifically into the [processed profile format](https://crates.io/crates/fxprof-processed-profile).

## Run

```
% cargo run --release -- perf.data
```

This creates a file called `profile-conv.json`.

Then open the profile in the Firefox profiler:

```
% profiler-symbol-server profile-conv.json   # Install with `cargo install profiler-symbol-server`
```
