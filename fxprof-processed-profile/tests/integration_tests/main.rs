use std::sync::Arc;
use std::time::Duration;

use assert_json_diff::assert_json_eq;
use debugid::DebugId;
use fxprof_processed_profile::{
    Category, CategoryColor, CpuDelta, FrameAddress, FrameFlags, GraphColor, LibraryInfo,
    MarkerFieldFlags, MarkerFieldFormat, MarkerGraphType, MarkerLocations, MarkerTiming, Profile,
    ReferenceTimestamp, SamplingInterval, StaticSchemaMarker, StaticSchemaMarkerField,
    StaticSchemaMarkerGraph, StringHandle, Symbol, SymbolTable, Timestamp, WeightType,
};
use serde_json::json;

// TODO: Add tests for SubcategoryHandle, ProcessHandle, ThreadHandle

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct TextMarker {
    pub name: StringHandle,
    pub text: StringHandle,
}

impl StaticSchemaMarker for TextMarker {
    const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
    const CHART_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.name}");
    const FIELDS: &'static [StaticSchemaMarkerField] = &[StaticSchemaMarkerField {
        key: "name",
        label: "Details",
        format: MarkerFieldFormat::String,
        flags: MarkerFieldFlags::SEARCHABLE,
    }];

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.name
    }

    fn string_field_value(&self, _field_index: u32) -> StringHandle {
        self.text
    }

    fn number_field_value(&self, _field_index: u32) -> f64 {
        unreachable!()
    }

    fn flow_field_value(&self, _field_index: u32) -> u64 {
        unreachable!()
    }
}

#[test]
fn profile_without_js() {
    struct CustomMarker {
        event_name: StringHandle,
        allocation_size: u32,
        url: StringHandle,
        latency: Duration,
    }
    impl StaticSchemaMarker for CustomMarker {
        const UNIQUE_MARKER_TYPE_NAME: &'static str = "custom";
        const TOOLTIP_LABEL: Option<&'static str> = Some("Custom tooltip label");

        const FIELDS: &'static [StaticSchemaMarkerField] = &[
            StaticSchemaMarkerField {
                key: "eventName",
                label: "Event name",
                format: MarkerFieldFormat::String,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
            StaticSchemaMarkerField {
                key: "allocationSize",
                label: "Allocation size",
                format: MarkerFieldFormat::Bytes,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
            StaticSchemaMarkerField {
                key: "url",
                label: "URL",
                format: MarkerFieldFormat::Url,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
            StaticSchemaMarkerField {
                key: "latency",
                label: "Latency",
                format: MarkerFieldFormat::Duration,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
        ];

        const DESCRIPTION: Option<&'static str> =
            Some("This is a test marker with a custom schema.");

        const GRAPHS: &'static [StaticSchemaMarkerGraph] = &[StaticSchemaMarkerGraph {
            key: "latency",
            graph_type: MarkerGraphType::Line,
            color: Some(GraphColor::Green),
        }];

        fn name(&self, profile: &mut Profile) -> StringHandle {
            profile.handle_for_string("CustomName")
        }

        fn string_field_value(&self, field_index: u32) -> StringHandle {
            match field_index {
                0 => self.event_name,
                2 => self.url,
                _ => unreachable!(),
            }
        }

        fn number_field_value(&self, field_index: u32) -> f64 {
            match field_index {
                1 => self.allocation_size.into(),
                3 => self.latency.as_secs_f64() * 1000.0,
                _ => unreachable!(),
            }
        }

        fn flow_field_value(&self, _field_index: u32) -> u64 {
            unreachable!()
        }
    }

    let mut profile = Profile::new(
        "test",
        ReferenceTimestamp::from_millis_since_unix_epoch(1636162232627.0),
        SamplingInterval::from_millis(1),
    );
    profile.set_os_name("macOS 14.4");
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
        None,
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
    });
    profile.set_lib_symbol_table(
        libc_handle,
        Arc::new(SymbolTable::new(vec![
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
        ])),
    );
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
    });
    profile.add_lib_mapping(
        process,
        dump_syms_lib_handle,
        0x000055ba9ebf6000,
        0x000055ba9f07e000,
        (0x000055ba9ebf6000u64 - 0x000055ba9eb4d000u64) as u32,
    );
    let category = profile.handle_for_category(Category("Regular", CategoryColor::Blue));
    let mut frames1_iter = [
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
            FrameAddress::InstructionPointer(addr)
        } else {
            FrameAddress::ReturnAddress(addr)
        }
    });
    let s1 = profile.handle_for_stack_frames(thread, |p| {
        Some(p.handle_for_frame_with_address(
            thread,
            frames1_iter.next()?,
            category,
            FrameFlags::empty(),
        ))
    });
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(1.0),
        s1,
        CpuDelta::ZERO,
        1,
    );
    let mut frames2_iter = [
        0x55ba9eda018e,
        0x55ba9ec3c3cf,
        0x55ba9ec2a2d7,
        0x55ba9ec53993,
        0x7f76b7e8707d,
        0x55ba9ec0f705,
        0x7ffdb4824838,
    ]
    .iter()
    .enumerate()
    .rev()
    .map(|(i, addr)| {
        if i == 0 {
            FrameAddress::InstructionPointer(*addr)
        } else {
            FrameAddress::ReturnAddress(*addr)
        }
    });
    let s2 = profile.handle_for_stack_frames(thread, |p| {
        Some(p.handle_for_frame_with_address(
            thread,
            frames2_iter.next()?,
            category,
            FrameFlags::empty(),
        ))
    });
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(2.0),
        s2,
        CpuDelta::ZERO,
        1,
    );
    let mut frames3_iter = [
        0x7f76b7f019c6,
        0x55ba9edc48f5,
        0x55ba9ec010e3,
        0x55ba9eca41b9,
        0x7f76b7e8707d,
        0x55ba9ec0f705,
        0x7ffdb4824838,
    ]
    .iter()
    .enumerate()
    .rev()
    .map(|(i, addr)| {
        if i == 0 {
            FrameAddress::InstructionPointer(*addr)
        } else {
            FrameAddress::ReturnAddress(*addr)
        }
    });
    let s3 = profile.handle_for_stack_frames(thread, |p| {
        Some(p.handle_for_frame_with_address(
            thread,
            frames3_iter.next()?,
            category,
            FrameFlags::empty(),
        ))
    });
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(3.0),
        s3,
        CpuDelta::ZERO,
        1,
    );

    let text_marker = TextMarker {
        name: profile.handle_for_string("Experimental"),
        text: profile.handle_for_string("Hello world!"),
    };
    profile.add_marker(
        thread,
        MarkerTiming::Instant(Timestamp::from_millis_since_reference(0.0)),
        text_marker,
    );
    let custom_marker = CustomMarker {
        event_name: profile.handle_for_string("My event"),
        allocation_size: 512000,
        url: profile.handle_for_string("https://mozilla.org/"),
        latency: Duration::from_millis(123),
    };
    profile.add_marker(
        thread,
        MarkerTiming::Interval(
            Timestamp::from_millis_since_reference(0.0),
            Timestamp::from_millis_since_reference(2.0),
        ),
        custom_marker,
    );

    let memory_counter =
        profile.add_counter(process, "malloc", "Memory", "Amount of allocated memory");
    profile.set_counter_color(memory_counter, GraphColor::Red);
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
                  "name": "Other",
                  "color": "grey",
                  "subcategories": [
                    "Other"
                  ]
                },
                {
                  "name": "Regular",
                  "color": "blue",
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
              "preprocessedProfileVersion": 56,
              "processType": 0,
              "product": "test",
              "oscpu": "macOS 14.4",
              "sampleUnits": {
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
                "time": "ms"
              },
              "startTime": 1636162232627.0,
              "symbolicated": false,
              "pausedRanges": [],
              "version": 24,
              "usesOnlyOneStackType": true,
              "sourceCodeIsNotOnSearchfox": true,
              "markerSchema": [
                {
                  "name": "Text",
                  "display": [
                    "marker-chart",
                    "marker-table"
                  ],
                  "chartLabel": "{marker.data.name}",
                  "tableLabel": "{marker.name} - {marker.data.name}",
                  "fields": [
                    {
                      "key": "name",
                      "label": "Details",
                      "format": "unique-string",
                      "searchable": true
                    }
                  ]
                },
                {
                  "name": "custom",
                  "display": [
                    "marker-chart",
                    "marker-table"
                  ],
                  "tooltipLabel": "Custom tooltip label",
                  "description": "This is a test marker with a custom schema.",
                  "fields": [
                    {
                      "key": "eventName",
                      "label": "Event name",
                      "format": "unique-string",
                      "searchable": true
                    },
                    {
                      "key": "allocationSize",
                      "label": "Allocation size",
                      "format": "bytes",
                      "searchable": true
                    },
                    {
                      "key": "url",
                      "label": "URL",
                      "format": "url",
                      "searchable": true
                    },
                    {
                      "key": "latency",
                      "label": "Latency",
                      "format": "duration",
                      "searchable": true
                    }
                  ],
                  "graphs": [
                    {
                      "key": "latency",
                      "type": "line",
                      "color": "green"
                    }
                  ]
                }
              ]
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
            "shared": {
              "stringArray": [
                "0x7ffdb4824837",
                "0xc2704",
                "dump_syms",
                "0xde777",
                "0x145418",
                "0x23eb61",
                "0x256d7e",
                "libc_symbol_1",
                "libc.so.6",
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
                "Hello world!",
                "My event",
                "https://mozilla.org/", // currently redundantly specified in the marker itself
                "CustomName",
              ],
            },
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
                      "type": "Text",
                      "name": 19
                    },
                    {
                      "type": "custom",
                      "eventName": 20,
                      "allocationSize": 512000.0,
                      "url": "https://mozilla.org/",
                      "latency": 123.0
                    }
                  ],
                  "endTime": [
                    0.0,
                    2.0
                  ],
                  "name": [
                    18,
                    22
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
                  "length": 3,
                  "address": [
                    1700001,
                    172156,
                    674226
                  ],
                  "functionSize": [
                    180,
                    20,
                    44
                  ],
                  "libIndex": [
                    1,
                    1,
                    1
                  ],
                  "name": [
                    7,
                    9,
                    17
                  ]
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
                  "weightType": "samples",
                  "stack": [
                    null,
                    6,
                    11,
                    15
                  ],
                  "timeDeltas": [
                    0.0,
                    1.0,
                    1.0,
                    1.0
                  ],
                  "weight": [
                    1,
                    1,
                    1,
                    1
                  ],
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
                  ]
                },
                "tid": "12345",
                "unregisterTime": null,
                "showMarkersInTimeline": false
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
                  "timeDeltas": [
                    0.0,
                    1.0,
                    1.0
                  ]
                },
                "color": "red"
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

    let some_label_string = profile.handle_for_string("Some label string");
    let category = profile.handle_for_category(Category("Cycle Collection", CategoryColor::Orange));
    let subcategory = profile.handle_for_subcategory(category, "Graph Reduction");
    let frames = vec![
        profile.handle_for_frame_with_label(thread, some_label_string, category, FrameFlags::IS_JS),
        profile.handle_for_frame_with_address(
            thread,
            FrameAddress::ReturnAddress(0x7f76b7ffc0e7),
            subcategory,
            FrameFlags::empty(),
        ),
    ];
    let mut iter = frames.into_iter();
    let s1 = profile.handle_for_stack_frames(thread, |_| iter.next());
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(1.0),
        s1,
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
                  "name": "Cycle Collection",
                  "color": "orange",
                  "subcategories": [
                    "Other",
                    "Graph Reduction"
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
              "preprocessedProfileVersion": 56,
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
              "sourceCodeIsNotOnSearchfox": true,
              "markerSchema": []
            },
            "libs": [],
            "shared": {
              "stringArray": [
                "Some label string",
                "0x7f76b7ffc0e6"
              ],
            },
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
                    1
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
                    0,
                    0
                  ],
                  "line": [
                    null,
                    null
                  ],
                  "column": [
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
                  "timeDeltas": [
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
                "showMarkersInTimeline": false,
                "stackTable": {
                  "length": 2,
                  "prefix": [
                    null,
                    0
                  ],
                  "frame": [
                    0,
                    1
                  ]
                },
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

#[test]
fn profile_counters_with_sorted_processes() {
    let mut profile = Profile::new(
        "test",
        ReferenceTimestamp::from_millis_since_unix_epoch(1636162232627.0),
        SamplingInterval::from_millis(1),
    );
    // Setting the timestamps first `1` and then `0` intentionally to make sure that the processes
    // are sorted and order has been reversed.
    let process0 = profile.add_process("test 1", 123, Timestamp::from_millis_since_reference(1.0));
    let process1 = profile.add_process("test 2", 123, Timestamp::from_millis_since_reference(0.0));
    let thread0 = profile.add_thread(
        process0,
        12345,
        Timestamp::from_millis_since_reference(0.0),
        true,
    );
    let thread1 = profile.add_thread(
        process1,
        54321,
        Timestamp::from_millis_since_reference(1.0),
        true,
    );

    profile.set_thread_show_markers_in_timeline(thread0, true);

    profile.add_sample(
        thread0,
        Timestamp::from_millis_since_reference(1.0),
        None,
        CpuDelta::ZERO,
        1,
    );
    profile.add_sample(
        thread1,
        Timestamp::from_millis_since_reference(0.0),
        None,
        CpuDelta::ZERO,
        1,
    );

    let memory_counter0 =
        profile.add_counter(process0, "malloc", "Memory 1", "Amount of allocated memory");
    profile.add_counter_sample(
        memory_counter0,
        Timestamp::from_millis_since_reference(1.0),
        0.0,
        0,
    );
    let memory_counter1 =
        profile.add_counter(process0, "malloc", "Memory 2", "Amount of allocated memory");
    profile.add_counter_sample(
        memory_counter1,
        Timestamp::from_millis_since_reference(0.0),
        0.0,
        0,
    );

    profile.set_symbolicated(true);

    profile.add_initial_visible_thread(thread1);
    profile.add_initial_selected_thread(thread1);

    profile.set_thread_samples_weight_type(thread0, WeightType::Bytes);

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
                }
              ],
              "debug": false,
              "extensions": {
                "baseURL": [],
                "id": [],
                "length": 0,
                "name": []
              },
              "initialSelectedThreads": [0],
              "initialVisibleThreads": [0],
              "interval": 1.0,
              "preprocessedProfileVersion": 56,
              "processType": 0,
              "product": "test",
              "sampleUnits": {
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
                "time": "ms"
              },
              "startTime": 1636162232627.0,
              "symbolicated": true,
              "pausedRanges": [],
              "version": 24,
              "usesOnlyOneStackType": true,
              "sourceCodeIsNotOnSearchfox": true,
              "markerSchema": []
            },
            "libs": [],
            "shared": {
              "stringArray": [],
            },
            "threads": [
              {
                "frameTable": {
                  "length": 0,
                  "address": [],
                  "inlineDepth": [],
                  "category": [],
                  "subcategory": [],
                  "func": [],
                  "nativeSymbol": [],
                  "innerWindowID": [],
                  "line": [],
                  "column": []
                },
                "funcTable": {
                  "length": 0,
                  "name": [],
                  "isJS": [],
                  "relevantForJS": [],
                  "resource": [],
                  "fileName": [],
                  "lineNumber": [],
                  "columnNumber": []
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
                "name": "test 2",
                "isMainThread": true,
                "nativeSymbols": {
                  "length": 0,
                  "address": [],
                  "functionSize": [],
                  "libIndex": [],
                  "name": []
                },
                "pausedRanges": [],
                "pid": "123.1",
                "processName": "test 2",
                "processShutdownTime": null,
                "processStartupTime": 0.0,
                "processType": "default",
                "registerTime": 1.0,
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
                    null
                  ],
                  "timeDeltas": [
                    0.0
                  ],
                  "weight": [
                    1
                  ],
                  "weightType": "samples",
                  "threadCPUDelta": [
                    0
                  ]
                },
                "showMarkersInTimeline": false,
                "stackTable": {
                  "length": 0,
                  "prefix": [],
                  "frame": []
                },
                "tid": "54321",
                "unregisterTime": null
              },
              {
                "frameTable": {
                  "length": 0,
                  "address": [],
                  "inlineDepth": [],
                  "category": [],
                  "subcategory": [],
                  "func": [],
                  "nativeSymbol": [],
                  "innerWindowID": [],
                  "line": [],
                  "column": []
                },
                "funcTable": {
                  "length": 0,
                  "name": [],
                  "isJS": [],
                  "relevantForJS": [],
                  "resource": [],
                  "fileName": [],
                  "lineNumber": [],
                  "columnNumber": []
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
                "name": "test 1",
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
                "processName": "test 1",
                "processShutdownTime": null,
                "processStartupTime": 1.0,
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
                    null
                  ],
                  "timeDeltas": [
                    1.0
                  ],
                  "weight": [
                    1
                  ],
                  "weightType": "bytes",
                  "threadCPUDelta": [
                    0
                  ]
                },
                "showMarkersInTimeline": true,
                "stackTable": {
                  "length": 0,
                  "prefix": [],
                  "frame": []
                },
                "tid": "12345",
                "unregisterTime": null
              }
            ],
            "pages": [],
            "profilerOverhead": [],
            "counters": [
              {
                "category": "Memory 1",
                "name": "malloc",
                "description": "Amount of allocated memory",
                "mainThreadIndex": 1,
                "pid": "123",
                "samples": {
                  "length": 1,
                  "count": [
                    0.0
                  ],
                  "number": [
                    0
                  ],
                  "timeDeltas": [
                    1.0
                  ]
                }
              },
              {
                "category": "Memory 2",
                "name": "malloc",
                "description": "Amount of allocated memory",
                "mainThreadIndex": 1,
                "pid": "123",
                "samples": {
                  "length": 1,
                  "count": [
                    0.0
                  ],
                  "number": [
                    0
                  ],
                  "timeDeltas": [
                    0.0
                  ]
                }
              }
            ]
          }
        )
    )
}

#[test]
fn test_flow_marker_fields() {
    /// A marker type with flow fields to test Flow and TerminatingFlow support.
    #[derive(Debug, Clone)]
    pub struct FlowMarker {
        pub name: StringHandle,
        pub flow_id: u64,
        pub terminating_flow_id: u64,
    }

    impl StaticSchemaMarker for FlowMarker {
        const UNIQUE_MARKER_TYPE_NAME: &'static str = "FlowTest";
        const LOCATIONS: MarkerLocations =
            MarkerLocations::MARKER_CHART.union(MarkerLocations::MARKER_TABLE);
        const CHART_LABEL: Option<&'static str> = Some("{marker.name}");
        const TABLE_LABEL: Option<&'static str> =
            Some("{marker.name} - flow:{marker.data.flowId} term:{marker.data.termFlowId}");

        const FIELDS: &'static [StaticSchemaMarkerField] = &[
            StaticSchemaMarkerField {
                key: "flowId",
                label: "Flow ID",
                format: MarkerFieldFormat::Flow,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
            StaticSchemaMarkerField {
                key: "termFlowId",
                label: "Terminating Flow ID",
                format: MarkerFieldFormat::TerminatingFlow,
                flags: MarkerFieldFlags::SEARCHABLE,
            },
        ];

        fn name(&self, _profile: &mut Profile) -> StringHandle {
            self.name
        }

        fn string_field_value(&self, _field_index: u32) -> StringHandle {
            unreachable!()
        }

        fn number_field_value(&self, _field_index: u32) -> f64 {
            unreachable!()
        }

        fn flow_field_value(&self, field_index: u32) -> u64 {
            match field_index {
                0 => self.flow_id,
                1 => self.terminating_flow_id,
                _ => unreachable!(),
            }
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

    // Add a flow marker
    let marker_name = profile.handle_for_string("Flow Start");
    let flow_marker = FlowMarker {
        name: marker_name,
        flow_id: 0xab54a98ceb1f0ad2,
        terminating_flow_id: 0x891087b8e3b70cb1,
    };

    profile.add_marker(
        thread,
        MarkerTiming::Instant(Timestamp::from_millis_since_reference(10.0)),
        flow_marker,
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
              "preprocessedProfileVersion": 56,
              "processType": 0,
              "product": "test",
              "sampleUnits": {
                "eventDelay": "ms",
                "threadCPUDelta": "µs",
                "time": "ms"
              },
              "startTime": 1636162232627.0,
              "symbolicated": false,
              "pausedRanges": [],
              "version": 24,
              "usesOnlyOneStackType": true,
              "sourceCodeIsNotOnSearchfox": true,
              "markerSchema": [
                {
                  "name": "FlowTest",
                  "display": [
                    "marker-chart",
                    "marker-table"
                  ],
                  "chartLabel": "{marker.name}",
                  "tableLabel": "{marker.name} - flow:{marker.data.flowId} term:{marker.data.termFlowId}",
                  "fields": [
                    {
                      "key": "flowId",
                      "label": "Flow ID",
                      "format": "flow-id",
                      "searchable": true
                    },
                    {
                      "key": "termFlowId",
                      "label": "Terminating Flow ID",
                      "format": "terminating-flow-id",
                      "searchable": true
                    }
                  ]
                }
              ]
            },
            "libs": [],
            "shared": {
              "stringArray": [
                "Flow Start",
                "ab54a98ceb1f0ad2",
                "891087b8e3b70cb1"
              ]
            },
            "threads": [
              {
                "frameTable": {
                  "length": 0,
                  "func": [],
                  "category": [],
                  "subcategory": [],
                  "line": [],
                  "column": [],
                  "address": [],
                  "nativeSymbol": [],
                  "inlineDepth": [],
                  "innerWindowID": []
                },
                "funcTable": {
                  "length": 0,
                  "name": [],
                  "isJS": [],
                  "relevantForJS": [],
                  "resource": [],
                  "fileName": [],
                  "lineNumber": [],
                  "columnNumber": []
                },
                "markers": {
                  "length": 1,
                  "category": [
                    0
                  ],
                  "data": [
                    {
                      "type": "FlowTest",
                      "flowId": 1,
                      "termFlowId": 2
                    }
                  ],
                  "endTime": [
                    0.0
                  ],
                  "name": [
                    0
                  ],
                  "phase": [
                    0
                  ],
                  "startTime": [
                    10.0
                  ]
                },
                "name": "test",
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
                "processName": "test",
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
                  "length": 0,
                  "weightType": "samples",
                  "stack": [],
                  "timeDeltas": [],
                  "weight": [],
                  "threadCPUDelta": []
                },
                "stackTable": {
                  "length": 0,
                  "prefix": [],
                  "frame": []
                },
                "tid": "12345",
                "unregisterTime": null,
                "showMarkersInTimeline": false
              }
            ],
            "pages": [],
            "profilerOverhead": [],
            "counters": []
          }
        )
    );
}
