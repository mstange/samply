# linux-perf-data

This repo contains a parser for the perf.data format which is output by the Linux `perf` tool.

It also contains some stuff in `main.rs` which will probably move into a different repo at some point. My goal is to create an equivalent to `perf script` which does fast symbolication and can efficiently output the folded stack format.

## Acknowledgements

Some of the code in this repo was copied from [*@koute*'s `not-perf` project](https://github.com/koute/not-perf/tree/20e4ddc2bf8895d96664ab839a64c36f416023c8/perf_event_open/src).

## Run

```
% cargo run --release -- perf.data
Hostname: ubuildu
OS release: 5.13.0-35-generic
Perf version: 5.13.19
Arch: x86_64
CPUs: 16 online (16 available)
Build IDs:
 - PID 4294967295, build ID 101ecd8ba902186974b9d547f9bfa64b166b3bb914000000, filename [kernel.kallsyms]
 - PID 4294967295, build ID bc972053b25ef022a845a3bea1e25c05067ee91f14000000, filename /home/mstange/code/dump_syms/target/release/dump_syms
 - PID 4294967295, build ID 0d82ee4bd7f9609c367095ba0bedf155b71cb05814000000, filename [vdso]
 - PID 4294967295, build ID f0fc29165cbe6088c0e1adf03b0048fbecbc003a14000000, filename /usr/lib/x86_64-linux-gnu/libc.so.6
Comm: {"pid": 212227, "tid": 212227, "name": "perf-exec"}
Comm: {"pid": 212227, "tid": 212227, "name": "dump_syms"}
file "/etc/ld.so.cache" had unrecognized format
Have 19833 events, converted into 15830 processed samples.
[
    ProcessedSample {
        timestamp: 29041229340225,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041229344633,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041229347013,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041229349145,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041229351351,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041229371201,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                140369773667131,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041230398595,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                139761322743248,
            ),
            Address(
                139761330423882,
            ),
            Address(
                139761330534654,
            ),
            Address(
                139761330413769,
            ),
            Address(
                139761330409687,
            ),
        ],
    },
    ProcessedSample {
        timestamp: 29041237992121,
        pid: 212227,
        tid: 212227,
        stack: [
            Address(
                139761322346802,
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 4598934,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 4582211,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 877409,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 981299,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 967729,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 972146,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 1271064,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 4629039,
                },
            ),
            InImage(
                StackFrameInImage {
                    image: ImageCacheHandle(
                        0,
                    ),
                    relative_lookup_address: 973313,
                },
            ),
            TruncatedStackMarker,
        ],
    },
[...]
```
