Converts from ETW logs to json consumable by https://profiler.firefox.com/.

Start profiling session by running `xperf -on latency -stackwalk profile` as Administrator. Then run `xperf -d out.etl` to capture it.
Finally run `cargo run --release out.etl [process-name]` to produce a gecko.json

The default sampling rate is 0.1221 ms (8192Hz). This can be set to a different value
with something like `xperf -on latency -stackwalk profile -SetProfInt 10000` for a rate
of 1ms. (The units are 100 nanoseconds)
