use assert_json_diff::assert_json_eq;
use debugid::DebugId;
use serde_json::json;

use fxprof_processed_profile::{
    CategoryColor, CpuDelta, Frame, FrameFlags, FrameInfo, LibraryInfo, MarkerDynamicField,
    MarkerFieldFormat, MarkerLocation, MarkerSchema, MarkerSchemaField, MarkerStaticField,
    MarkerTiming, Profile, ProfilerMarker, ReferenceTimestamp, SamplingInterval, Symbol,
    SymbolTable, Timestamp,
};

use std::sync::Arc;
use std::time::Duration;

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
fn profile_without_js() {
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
    let libc_handle = profile.add_lib(LibraryInfo {
        name: "libc.so.6".to_string(),
        debug_name: "libc.so.6".to_string(),
        path: "/usr/lib/x86_64-linux-gnu/libc.so.6".to_string(),
        code_id: Some("f0fc29165cbe6088c0e1adf03b0048fbecbc003a".to_string()),
        debug_path: "/usr/lib/x86_64-linux-gnu/libc.so.6".to_string(),
        debug_id: DebugId::from_breakpad("1629FCF0BE5C8860C0E1ADF03B0048FB0").unwrap(),
        arch: None,
        symbol_table: Some(Arc::new(SymbolTable::new(vec![
            Symbol {
                address: 1700001,
                size: Some(180),
                name: "libc_symbol_1".to_string(),
            },
            Symbol {
                address: 674226,
                size: Some(44),
                name: "libc_symbol_3".to_string(),
            },
            Symbol {
                address: 172156,
                size: Some(20),
                name: "libc_symbol_2".to_string(),
            },
        ]))),
    });
    profile.add_lib_mapping(
        process,
        libc_handle,
        0x00007f76b7e85000,
        0x00007f76b8019000,
        (0x00007f76b7e85000u64 - 0x00007f76b7e5d000u64) as u32,
    );
    let dump_syms_lib_handle = profile.add_lib(LibraryInfo {
        name: "dump_syms".to_string(),
        debug_name: "dump_syms".to_string(),
        path: "/home/mstange/code/dump_syms/target/release/dump_syms".to_string(),
        code_id: Some("510d0a5c19eadf8043f203b4525be9be3dcb9554".to_string()),
        debug_path: "/home/mstange/code/dump_syms/target/release/dump_syms".to_string(),
        debug_id: DebugId::from_breakpad("5C0A0D51EA1980DF43F203B4525BE9BE0").unwrap(),
        arch: None,
        symbol_table: None,
    });
    profile.add_lib_mapping(
        process,
        dump_syms_lib_handle,
        0x000055ba9ebf6000,
        0x000055ba9f07e000,
        (0x000055ba9ebf6000u64 - 0x000055ba9eb4d000u64) as u32,
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
        .map(|frame| FrameInfo {
            frame,
            category_pair: category.into(),
            flags: FrameFlags::empty(),
        }),
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
        .map(|frame| FrameInfo {
            frame,
            category_pair: category.into(),
            flags: FrameFlags::empty(),
        }),
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
        .map(|frame| FrameInfo {
            frame,
            category_pair: category.into(),
            flags: FrameFlags::empty(),
        }),
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

    let memory_counter =
        profile.add_counter(process, "malloc", "Memory", "Amount of allocated memory");
    profile.add_counter_sample(
        memory_counter,
        Timestamp::from_millis_since_reference(0.0),
        0.0,
        0,
    );
    profile.add_counter_sample(
        memory_counter,
        Timestamp::from_millis_since_reference(1.0),
        1000.0,
        2,
    );
    profile.add_counter_sample(
        memory_counter,
        Timestamp::from_millis_since_reference(2.0),
        800.0,
        1,
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
                  "preprocessedProfileVersion": 46,
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
                        0,
                        1,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        null,
                        2
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
                        2,
                        3,
                        4,
                        5,
                        6,
                        8,
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
                    "name": "test",
                    "isMainThread": true,
                    "nativeSymbols": {
                      "address": [1700001, 172156, 674226],
                      "functionSize": [Some(180), Some(20), Some(44)],
                      "length": 3,
                      "libIndex": [1, 1, 1],
                      "name": [8, 9, 17]
                    },
                    "pausedRanges": [],
                    "pid": "123",
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
                        1,
                        7
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
                      "dump_syms",
                      "0xc2704",
                      "0xde777",
                      "0x145418",
                      "0x23eb61",
                      "0x256d7e",
                      "libc.so.6",
                      "libc_symbol_1",
                      "libc_symbol_2",
                      "0x106992",
                      "0xdd2d6",
                      "0xef3ce",
                      "0x25318e",
                      "0x1571b8",
                      "0xb40e2",
                      "0x2778f4",
                      "libc_symbol_3",
                      "Experimental",
                      "CustomName"
                    ],
                    "tid": "12345",
                    "unregisterTime": null
                  }
                ],
                "pages": [],
                "profilerOverhead": [],
                "counters": [
                  {
                    "category": "Memory",
                    "name": "malloc",
                    "description": "Amount of allocated memory",
                    "mainThreadIndex": 0,
                    "pid": "123",
                    "sampleGroups": [
                      {
                        "id": 0,
                        "samples": {
                          "length": 3,
                          "count": [
                            0.0,
                            1000.0,
                            800.0
                          ],
                          "number": [
                            0,
                            2,
                            1
                          ],
                          "time": [
                            0.0,
                            1.0,
                            2.0
                          ]
                        }
                      }
                    ]
                  }
                ]
              }
        )
    )
}

#[test]
fn profile_with_js() {
    let mut profile = Profile::new(
        "test with js",
        ReferenceTimestamp::from_millis_since_unix_epoch(1636162232627.0),
        SamplingInterval::from_millis(1),
    );
    let process = profile.add_process("test2", 123, Timestamp::from_millis_since_reference(0.0));
    let thread = profile.add_thread(
        process,
        12346,
        Timestamp::from_millis_since_reference(0.0),
        true,
    );

    let some_label_string = profile.intern_string("Some label string");
    let category = profile.add_category("Regular", CategoryColor::Green);
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(1.0),
        vec![
            FrameInfo {
                frame: Frame::Label(some_label_string),
                category_pair: category.into(),
                flags: FrameFlags::IS_JS,
            },
            FrameInfo {
                frame: Frame::ReturnAddress(0x7f76b7ffc0e7),
                category_pair: category.into(),
                flags: FrameFlags::empty(),
            },
        ]
        .into_iter(),
        CpuDelta::ZERO,
        1,
    );

    // eprintln!("{}", serde_json::to_string_pretty(&profile).unwrap());
    assert_json_eq!(
        profile,
        json!(
          {
            "meta": {
              "categories": [
                {
                  "name": "Other",
                  "color": "grey",
                  "subcategories": [
                    "Other"
                  ]
                },
                {
                  "name": "Regular",
                  "color": "green",
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
              "preprocessedProfileVersion": 46,
              "processType": 0,
              "product": "test with js",
              "sampleUnits": {
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
                "time": "ms"
              },
              "startTime": 1636162232627.0,
              "symbolicated": false,
              "pausedRanges": [],
              "version": 24,
              "usesOnlyOneStackType": false,
              "doesNotUseFrameImplementation": true,
              "sourceCodeIsNotOnSearchfox": true,
              "markerSchema": []
            },
            "libs": [],
            "threads": [
              {
                "frameTable": {
                  "length": 2,
                  "address": [
                    -1,
                    -1
                  ],
                  "inlineDepth": [
                    0,
                    0
                  ],
                  "category": [
                    1,
                    1
                  ],
                  "subcategory": [
                    0,
                    0
                  ],
                  "func": [
                    0,
                    1
                  ],
                  "nativeSymbol": [
                    null,
                    null
                  ],
                  "innerWindowID": [
                    null,
                    null
                  ],
                  "implementation": [
                    null,
                    null
                  ],
                  "line": [
                    null,
                    null
                  ],
                  "column": [
                    null,
                    null
                  ],
                  "optimizations": [
                    null,
                    null
                  ]
                },
                "funcTable": {
                  "length": 2,
                  "name": [
                    0,
                    1
                  ],
                  "isJS": [
                    true,
                    false
                  ],
                  "relevantForJS": [
                    false,
                    false
                  ],
                  "resource": [
                    -1,
                    -1
                  ],
                  "fileName": [
                    null,
                    null
                  ],
                  "lineNumber": [
                    null,
                    null
                  ],
                  "columnNumber": [
                    null,
                    null
                  ]
                },
                "markers": {
                  "length": 0,
                  "category": [],
                  "data": [],
                  "endTime": [],
                  "name": [],
                  "phase": [],
                  "startTime": []
                },
                "name": "test2",
                "isMainThread": true,
                "nativeSymbols": {
                  "length": 0,
                  "address": [],
                  "functionSize": [],
                  "libIndex": [],
                  "name": []
                },
                "pausedRanges": [],
                "pid": "123",
                "processName": "test2",
                "processShutdownTime": null,
                "processStartupTime": 0.0,
                "processType": "default",
                "registerTime": 0.0,
                "resourceTable": {
                  "length": 0,
                  "lib": [],
                  "name": [],
                  "host": [],
                  "type": []
                },
                "samples": {
                  "length": 1,
                  "stack": [
                    1
                  ],
                  "time": [
                    1.0
                  ],
                  "weight": [
                    1
                  ],
                  "weightType": "samples",
                  "threadCPUDelta": [
                    0
                  ]
                },
                "stackTable": {
                  "length": 2,
                  "prefix": [
                    null,
                    0
                  ],
                  "frame": [
                    0,
                    1
                  ],
                  "category": [
                    1,
                    1
                  ],
                  "subcategory": [
                    0,
                    0
                  ]
                },
                "stringArray": [
                  "Some label string",
                  "0x7f76b7ffc0e6"
                ],
                "tid": "12346",
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
