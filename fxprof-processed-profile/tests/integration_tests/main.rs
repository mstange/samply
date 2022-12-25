use assert_json_diff::assert_json_eq;
use debugid::{CodeId, DebugId};
use serde_json::json;
use std::{str::FromStr, time::Duration};

use fxprof_processed_profile::{
    CategoryColor, CpuDelta, Frame, LibraryInfo, MarkerDynamicField, MarkerFieldFormat,
    MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerStaticField, MarkerTiming, Profile,
    ProfilerMarker, ReferenceTimestamp, SamplingInterval, Timestamp,
};

// TODO: Add tests for CategoryPairHandle, ProcessHandle, ThreadHandle

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct TextMarker(pub String);

impl ProfilerMarker for TextMarker {
    const MARKER_TYPE_NAME: &'static str = "Text";

    fn json_marker_data(&self) -> serde_json::Value {
        json!({
            "type": Self::MARKER_TYPE_NAME,
            "name": self.0
        })
    }

    fn schema() -> MarkerSchema {
        MarkerSchema {
            type_name: Self::MARKER_TYPE_NAME,
            locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
            chart_label: Some("{marker.data.name}"),
            tooltip_label: None,
            table_label: Some("{marker.name} - {marker.data.name}"),
            fields: vec![MarkerSchemaField::Dynamic(MarkerDynamicField {
                key: "name",
                label: "Details",
                format: MarkerFieldFormat::String,
                searchable: None,
            })],
        }
    }
}

#[test]
fn it_works() {
    struct CustomMarker {
        event_name: String,
        allocation_size: u32,
        url: String,
        latency: Duration,
    }
    impl ProfilerMarker for CustomMarker {
        const MARKER_TYPE_NAME: &'static str = "custom";

        fn schema() -> MarkerSchema {
            MarkerSchema {
                type_name: Self::MARKER_TYPE_NAME,
                locations: vec![MarkerLocation::MarkerChart, MarkerLocation::MarkerTable],
                chart_label: None,
                tooltip_label: Some("Custom tooltip label"),
                table_label: None,
                fields: vec![
                    MarkerSchemaField::Dynamic(MarkerDynamicField {
                        key: "eventName",
                        label: "Event name",
                        format: MarkerFieldFormat::String,
                        searchable: None,
                    }),
                    MarkerSchemaField::Dynamic(MarkerDynamicField {
                        key: "allocationSize",
                        label: "Allocation size",
                        format: MarkerFieldFormat::Bytes,
                        searchable: None,
                    }),
                    MarkerSchemaField::Dynamic(MarkerDynamicField {
                        key: "url",
                        label: "URL",
                        format: MarkerFieldFormat::Url,
                        searchable: None,
                    }),
                    MarkerSchemaField::Dynamic(MarkerDynamicField {
                        key: "latency",
                        label: "Latency",
                        format: MarkerFieldFormat::Duration,
                        searchable: None,
                    }),
                    MarkerSchemaField::Static(MarkerStaticField {
                        label: "Description",
                        value: "This is a test marker with a custom schema.",
                    }),
                ],
            }
        }

        fn json_marker_data(&self) -> serde_json::Value {
            json!({
                "type": Self::MARKER_TYPE_NAME,
                "eventName": self.event_name,
                "allocationSize": self.allocation_size,
                "url": self.url,
                "latency": self.latency.as_secs_f64() * 1000.0,
            })
        }
    }

    let mut profile = Profile::new(
        "test",
        ReferenceTimestamp::from_millis_since_unix_epoch(1636162232627.0),
        SamplingInterval::from_millis(1),
    );
    let process = profile.add_process("test", 123, Timestamp::from_millis_since_reference(0.0));
    let thread = profile.add_thread(
        process,
        12345,
        Timestamp::from_millis_since_reference(0.0),
        true,
    );

    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(0.0),
        vec![].into_iter(),
        CpuDelta::ZERO,
        1,
    );
    profile.add_lib(
        process,
        LibraryInfo {
            name: "libc.so.6".to_string(),
            debug_name: "libc.so.6".to_string(),
            path: "/usr/lib/x86_64-linux-gnu/libc.so.6".to_string(),
            code_id: Some(CodeId::from_str("f0fc29165cbe6088c0e1adf03b0048fbecbc003a").unwrap()),
            debug_path: "/usr/lib/x86_64-linux-gnu/libc.so.6".to_string(),
            debug_id: DebugId::from_breakpad("1629FCF0BE5C8860C0E1ADF03B0048FB0").unwrap(),
            arch: None,
            base_avma: 0x00007f76b7e5d000,
            avma_range: 0x00007f76b7e85000..0x00007f76b8019000,
        },
    );
    profile.add_lib(
        process,
        LibraryInfo {
            name: "dump_syms".to_string(),
            debug_name: "dump_syms".to_string(),
            path: "/home/mstange/code/dump_syms/target/release/dump_syms".to_string(),
            code_id: Some(CodeId::from_str("510d0a5c19eadf8043f203b4525be9be3dcb9554").unwrap()),
            debug_path: "/home/mstange/code/dump_syms/target/release/dump_syms".to_string(),
            debug_id: DebugId::from_breakpad("5C0A0D51EA1980DF43F203B4525BE9BE0").unwrap(),
            arch: None,
            base_avma: 0x000055ba9eb4d000,
            avma_range: 0x000055ba9ebf6000..0x000055ba9f07e000,
        },
    );
    let category = profile.add_category("Regular", CategoryColor::Blue);
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(1.0),
        vec![
            0x7f76b7ffc0e7,
            0x55ba9eda3d7f,
            0x55ba9ed8bb62,
            0x55ba9ec92419,
            0x55ba9ec2b778,
            0x55ba9ec0f705,
            0x7ffdb4824838,
        ]
        .into_iter()
        .enumerate()
        .rev()
        .map(|(i, addr)| {
            if i == 0 {
                Frame::InstructionPointer(addr)
            } else {
                Frame::ReturnAddress(addr)
            }
        })
        .map(|frame| (frame, category.into())),
        CpuDelta::ZERO,
        1,
    );
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(2.0),
        vec![
            0x55ba9eda018e,
            0x55ba9ec3c3cf,
            0x55ba9ec2a2d7,
            0x55ba9ec53993,
            0x7f76b7e8707d,
            0x55ba9ec0f705,
            0x7ffdb4824838,
        ]
        .into_iter()
        .enumerate()
        .rev()
        .map(|(i, addr)| {
            if i == 0 {
                Frame::InstructionPointer(addr)
            } else {
                Frame::ReturnAddress(addr)
            }
        })
        .map(|frame| (frame, category.into())),
        CpuDelta::ZERO,
        1,
    );
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(3.0),
        vec![
            0x7f76b7f019c6,
            0x55ba9edc48f5,
            0x55ba9ec010e3,
            0x55ba9eca41b9,
            0x7f76b7e8707d,
            0x55ba9ec0f705,
            0x7ffdb4824838,
        ]
        .into_iter()
        .enumerate()
        .rev()
        .map(|(i, addr)| {
            if i == 0 {
                Frame::InstructionPointer(addr)
            } else {
                Frame::ReturnAddress(addr)
            }
        })
        .map(|frame| (frame, category.into())),
        CpuDelta::ZERO,
        1,
    );

    profile.add_marker(
        thread,
        "Experimental",
        TextMarker("Hello world!".to_string()),
        MarkerTiming::Instant(Timestamp::from_millis_since_reference(0.0)),
    );
    profile.add_marker(
        thread,
        "CustomName",
        CustomMarker {
            event_name: "My event".to_string(),
            allocation_size: 512000,
            url: "https://mozilla.org/".to_string(),
            latency: Duration::from_millis(123),
        },
        MarkerTiming::Interval(
            Timestamp::from_millis_since_reference(0.0),
            Timestamp::from_millis_since_reference(2.0),
        ),
    );
    // eprintln!("{}", serde_json::to_string_pretty(&profile).unwrap());
    assert_json_eq!(
        profile,
        json!(
            {
                "meta": {
                  "categories": [
                    {
                      "color": "grey",
                      "name": "Other",
                      "subcategories": [
                        "Other"
                      ]
                    },
                    {
                      "color": "blue",
                      "name": "Regular",
                      "subcategories": [
                        "Other"
                      ]
                    }
                  ],
                  "debug": false,
                  "extensions": {
                    "baseURL": [],
                    "id": [],
                    "length": 0,
                    "name": []
                  },
                  "interval": 1.0,
                  "markerSchema": [
                    {
                      "chartLabel": "{marker.data.name}",
                      "data": [
                        {
                          "format": "string",
                          "key": "name",
                          "label": "Details"
                        }
                      ],
                      "display": [
                        "marker-chart",
                        "marker-table"
                      ],
                      "name": "Text",
                      "tableLabel": "{marker.name} - {marker.data.name}"
                    },
                    {
                      "data": [
                        {
                          "format": "string",
                          "key": "eventName",
                          "label": "Event name"
                        },
                        {
                          "format": "bytes",
                          "key": "allocationSize",
                          "label": "Allocation size"
                        },
                        {
                          "format": "url",
                          "key": "url",
                          "label": "URL"
                        },
                        {
                          "format": "duration",
                          "key": "latency",
                          "label": "Latency"
                        },
                        {
                          "label": "Description",
                          "value": "This is a test marker with a custom schema."
                        }
                      ],
                      "display": [
                        "marker-chart",
                        "marker-table"
                      ],
                      "name": "custom",
                      "tooltipLabel": "Custom tooltip label"
                    }
                  ],
                  "pausedRanges": [],
                  "preprocessedProfileVersion": 44,
                  "processType": 0,
                  "product": "test",
                  "sampleUnits": {
                    "eventDelay": "ms",
                    "threadCPUDelta": "µs",
                    "time": "ms"
                  },
                  "startTime": 1636162232627.0,
                  "symbolicated": false,
                  "version": 24,
                  "usesOnlyOneStackType": true,
                  "doesNotUseFrameImplementation": true,
                  "sourceCodeIsNotOnSearchfox": true
                },
                "libs": [
                  {
                    "name": "dump_syms",
                    "path": "/home/mstange/code/dump_syms/target/release/dump_syms",
                    "debugName": "dump_syms",
                    "debugPath": "/home/mstange/code/dump_syms/target/release/dump_syms",
                    "breakpadId": "5C0A0D51EA1980DF43F203B4525BE9BE0",
                    "codeId": "510d0a5c19eadf8043f203b4525be9be3dcb9554",
                    "arch": null
                  },
                  {
                    "name": "libc.so.6",
                    "path": "/usr/lib/x86_64-linux-gnu/libc.so.6",
                    "debugName": "libc.so.6",
                    "debugPath": "/usr/lib/x86_64-linux-gnu/libc.so.6",
                    "breakpadId": "1629FCF0BE5C8860C0E1ADF03B0048FB0",
                    "codeId": "f0fc29165cbe6088c0e1adf03b0048fbecbc003a",
                    "arch": null
                  }
                ],
                "threads": [
                  {
                    "frameTable": {
                      "length": 16,
                      "address": [
                        -1,
                        796420,
                        911223,
                        1332248,
                        2354017,
                        2452862,
                        1700071,
                        172156,
                        1075602,
                        905942,
                        979918,
                        2437518,
                        1405368,
                        737506,
                        2586868,
                        674246
                      ],
                      "inlineDepth": [
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0
                      ],
                      "category": [
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1
                      ],
                      "subcategory": [
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0
                      ],
                      "func": [
                        0,
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        14,
                        15
                      ],
                      "nativeSymbol": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "innerWindowID": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "implementation": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "line": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "column": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "optimizations": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ]
                    },
                    "funcTable": {
                      "length": 16,
                      "name": [
                        0,
                        1,
                        3,
                        4,
                        5,
                        6,
                        7,
                        9,
                        10,
                        11,
                        12,
                        13,
                        14,
                        15,
                        16,
                        17
                      ],
                      "isJS": [
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false
                      ],
                      "relevantForJS": [
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false,
                        false
                      ],
                      "resource": [
                        -1,
                        0,
                        0,
                        0,
                        0,
                        0,
                        1,
                        1,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        1
                      ],
                      "fileName": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "lineNumber": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ],
                      "columnNumber": [
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null
                      ]
                    },
                    "markers": {
                      "length": 2,
                      "category": [
                        0,
                        0
                      ],
                      "data": [
                        {
                          "name": "Hello world!",
                          "type": "Text"
                        },
                        {
                          "allocationSize": 512000,
                          "eventName": "My event",
                          "latency": 123.0,
                          "type": "custom",
                          "url": "https://mozilla.org/"
                        }
                      ],
                      "endTime": [
                        0.0,
                        2.0
                      ],
                      "name": [
                        18,
                        19
                      ],
                      "phase": [
                        0,
                        1
                      ],
                      "startTime": [
                        0.0,
                        0.0
                      ]
                    },
                    "name": "GeckoMain",
                    "nativeSymbols": {
                      "address": [],
                      "functionSize": [],
                      "length": 0,
                      "libIndex": [],
                      "name": []
                    },
                    "pausedRanges": [],
                    "pid": 123,
                    "processName": "test",
                    "processShutdownTime": null,
                    "processStartupTime": 0.0,
                    "processType": "default",
                    "registerTime": 0.0,
                    "resourceTable": {
                      "length": 2,
                      "lib": [
                        0,
                        1
                      ],
                      "name": [
                        2,
                        8
                      ],
                      "host": [
                        null,
                        null
                      ],
                      "type": [
                        1,
                        1
                      ]
                    },
                    "samples": {
                      "length": 4,
                      "stack": [
                        null,
                        6,
                        11,
                        15
                      ],
                      "time": [
                        0.0,
                        1.0,
                        2.0,
                        3.0
                      ],
                      "weight": [
                        1,
                        1,
                        1,
                        1
                      ],
                      "weightType": "samples",
                      "threadCPUDelta": [
                        0,
                        0,
                        0,
                        0
                      ]
                    },
                    "stackTable": {
                      "length": 16,
                      "prefix": [
                        null,
                        0,
                        1,
                        2,
                        3,
                        4,
                        5,
                        1,
                        7,
                        8,
                        9,
                        10,
                        7,
                        12,
                        13,
                        14
                      ],
                      "frame": [
                        0,
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        14,
                        15
                      ],
                      "category": [
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1,
                        1
                      ],
                      "subcategory": [
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0
                      ]
                    },
                    "stringArray": [
                      "0x7ffdb4824837",
                      "0xc2704",
                      "dump_syms",
                      "0xde777",
                      "0x145418",
                      "0x23eb61",
                      "0x256d7e",
                      "0x19f0e7",
                      "libc.so.6",
                      "0x2a07c",
                      "0x106992",
                      "0xdd2d6",
                      "0xef3ce",
                      "0x25318e",
                      "0x1571b8",
                      "0xb40e2",
                      "0x2778f4",
                      "0xa49c6",
                      "Experimental",
                      "CustomName"
                    ],
                    "tid": 12345,
                    "unregisterTime": null
                  }
                ],
                "pages": [],
                "profilerOverhead": [],
                "counters": []
              }
        )
    )
}
