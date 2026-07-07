use std::sync::Arc;
use std::time::Duration;

use debugid::DebugId;
use fxprof_processed_profile::{
    Category, CategoryColor, CounterDisplayConfig, CpuDelta, FlowId, FrameAddress, FrameFlags,
    GraphColor, LibraryInfo, Marker, MarkerField, MarkerGraph, MarkerGraphType, MarkerLocations,
    MarkerTiming, Profile, ReferenceTimestamp, SamplingInterval, Schema, StringHandle, Symbol,
    SymbolTable, Timestamp, WeightType,
};

// TODO: Add tests for SubcategoryHandle, ProcessHandle, ThreadHandle

fn profile_as_json_value(profile: &Profile) -> serde_json::Value {
    serde_json::from_slice(&profile.to_vec()).expect("profile JSON must parse")
}

/// An example marker type with some text content.
#[derive(Debug, Clone)]
pub struct TextMarker {
    pub name: StringHandle,
    pub text: StringHandle,
}

impl Marker for TextMarker {
    type FieldsType = StringHandle;

    const UNIQUE_MARKER_TYPE_NAME: &'static str = "Text";
    const CHART_LABEL: Option<&'static str> = Some("{marker.data.name}");
    const TABLE_LABEL: Option<&'static str> = Some("{marker.name} - {marker.data.name}");
    const FIELDS: Schema<Self::FieldsType> = Schema(MarkerField::string("name", "Details"));

    fn name(&self, _profile: &mut Profile) -> StringHandle {
        self.name
    }

    fn field_values(&self) -> StringHandle {
        self.text
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
    impl Marker for CustomMarker {
        type FieldsType = (StringHandle, f64, StringHandle, f64);

        const UNIQUE_MARKER_TYPE_NAME: &'static str = "custom";
        const TOOLTIP_LABEL: Option<&'static str> = Some("Custom tooltip label");

        const FIELDS: Schema<Self::FieldsType> = Schema((
            MarkerField::string("eventName", "Event name"),
            MarkerField::bytes("allocationSize", "Allocation size"),
            MarkerField::url("url", "URL"),
            MarkerField::duration("latency", "Latency"),
        ));

        const DESCRIPTION: Option<&'static str> =
            Some("This is a test marker with a custom schema.");

        const GRAPHS: &'static [MarkerGraph] = &[MarkerGraph {
            key: "latency",
            graph_type: MarkerGraphType::Line,
            color: Some(GraphColor::Green),
        }];

        fn name(&self, profile: &mut Profile) -> StringHandle {
            profile.handle_for_string("CustomName")
        }

        fn field_values(&self) -> (StringHandle, f64, StringHandle, f64) {
            (
                self.event_name,
                self.allocation_size.into(),
                self.url,
                self.latency.as_secs_f64() * 1000.0,
            )
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
            FrameAddress::InstructionPointer(process, addr)
        } else {
            FrameAddress::ReturnAddress(process, addr)
        }
    });
    let s1 = profile.handle_for_stack_frames(|p| {
        Some(p.handle_for_frame_with_address(frames1_iter.next()?, category, FrameFlags::empty()))
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
            FrameAddress::InstructionPointer(process, *addr)
        } else {
            FrameAddress::ReturnAddress(process, *addr)
        }
    });
    let s2 = profile.handle_for_stack_frames(|p| {
        Some(p.handle_for_frame_with_address(frames2_iter.next()?, category, FrameFlags::empty()))
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
            FrameAddress::InstructionPointer(process, *addr)
        } else {
            FrameAddress::ReturnAddress(process, *addr)
        }
    });
    let s3 = profile.handle_for_stack_frames(|p| {
        Some(p.handle_for_frame_with_address(frames3_iter.next()?, category, FrameFlags::empty()))
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

    let memory_counter = profile.add_counter(
        process,
        "malloc",
        "Memory",
        CounterDisplayConfig::for_memory(),
        "Amount of allocated memory",
    );
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

    insta::assert_json_snapshot!(profile_as_json_value(&profile));
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
        profile.handle_for_frame_with_label(some_label_string, category, FrameFlags::IS_JS),
        profile.handle_for_frame_with_address(
            FrameAddress::ReturnAddress(process, 0x7f76b7ffc0e7),
            subcategory,
            FrameFlags::empty(),
        ),
    ];
    let mut iter = frames.into_iter();
    let s1 = profile.handle_for_stack_frames(|_| iter.next());
    profile.add_sample(
        thread,
        Timestamp::from_millis_since_reference(1.0),
        s1,
        CpuDelta::ZERO,
        1,
    );

    insta::assert_json_snapshot!(profile_as_json_value(&profile));
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

    let memory_counter0 = profile.add_counter(
        process0,
        "malloc",
        "Memory 1",
        CounterDisplayConfig::for_memory(),
        "Amount of allocated memory",
    );
    profile.add_counter_sample(
        memory_counter0,
        Timestamp::from_millis_since_reference(1.0),
        0.0,
        0,
    );
    let memory_counter1 = profile.add_counter(
        process0,
        "malloc",
        "Memory 2",
        CounterDisplayConfig::for_memory(),
        "Amount of allocated memory",
    );
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

    insta::assert_json_snapshot!(profile_as_json_value(&profile));
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

    impl Marker for FlowMarker {
        type FieldsType = (FlowId, FlowId);
        const UNIQUE_MARKER_TYPE_NAME: &'static str = "FlowTest";
        const LOCATIONS: MarkerLocations =
            MarkerLocations::MARKER_CHART.union(MarkerLocations::MARKER_TABLE);
        const CHART_LABEL: Option<&'static str> = Some("{marker.name}");
        const TABLE_LABEL: Option<&'static str> =
            Some("{marker.name} - flow:{marker.data.flowId} term:{marker.data.termFlowId}");

        const FIELDS: Schema<Self::FieldsType> = Schema((
            MarkerField::flow("flowId", "Flow ID"),
            MarkerField::terminating_flow("termFlowId", "Terminating Flow ID"),
        ));

        fn name(&self, _profile: &mut Profile) -> StringHandle {
            self.name
        }

        fn field_values(&self) -> Self::FieldsType {
            (FlowId(self.flow_id), FlowId(self.terminating_flow_id))
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

    insta::assert_json_snapshot!(profile_as_json_value(&profile));
}
