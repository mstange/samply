Converts from ETW logs to json consumable by https://profiler.firefox.com/.

Start profiling session by running `xperf -on latency -stackwalk profile` as Administrator. Then run `xperf -d out.etl` to capture it.
Finally run `cargo run out.etl [process-name]` to produce a gecko.json