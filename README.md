# fxprof-perf-convert

A converter from the Linux perf `perf.data` format into the [Firefox Profiler](https://profiler.firefox.com/) format, specifically into the [processed profile format](https://crates.io/crates/fxprof-processed-profile).

Here's an [example profile of Firefox](https://share.firefox.dev/37QbKlM). And here's [a profile of the converter](https://share.firefox.dev/3wh6CQZ). Both of these profiles were obtained by running `perf record --call-graph dwarf` and then converting the perf.data file.

## Run

```
% cargo run --release -- perf.data
```

This creates a file called `profile-conv.json`.

Then open the profile in the Firefox profiler:

```
% profiler-symbol-server profile-conv.json   # Install with `cargo install profiler-symbol-server`
```
