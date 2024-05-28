use bitflags::bitflags;

use super::elevated_helper::ElevatedRecordingProps;

// https://source.chromium.org/chromium/chromium/src/+/main:third_party/win_build_output/mc/base/trace_event/etw_manifest/chrome_events_win.h;l=622-623;drc=a9274264bd203626a6530bebae3e7d4eae12c733
pub const CHROME_PROVIDER_GUID: &str = "d2d578d9-2936-45b6-a09f-30e32715f42d";

bitflags! {
    // https://source.chromium.org/chromium/chromium/src/+/main:base/trace_event/trace_event_etw_export_win.cc;l=103;drc=8c29f4a8930c3ccccdf1b66c28fe484cee7c7362
    #[derive(PartialEq, Eq)]
    pub struct KeywordNames: u64 {
        const benchmark = 0x1;
        const blink = 0x2;
        const browser = 0x4;
        const cc = 0x8;
        const evdev = 0x10;
        const gpu = 0x20;
        const input = 0x40;
        const netlog = 0x80;
        const sequence_manager = 0x100;
        const toplevel = 0x200;
        const v8 = 0x400;
        const disabled_by_default_cc_debug = 0x800;
        const disabled_by_default_cc_debug_picture = 0x1000;
        const disabled_by_default_toplevel_flow = 0x2000;
        const startup = 0x4000;
        const latency = 0x8000;
        const blink_user_timing = 0x10000;
        const media = 0x20000;
        const loading = 0x40000;
        const base = 0x80000;
        const devtools_timeline = 0x100000;
        const unused_bit_21 = 0x200000;
        const unused_bit_22 = 0x400000;
        const unused_bit_23 = 0x800000;
        const unused_bit_24 = 0x1000000;
        const unused_bit_25 = 0x2000000;
        const unused_bit_26 = 0x4000000;
        const unused_bit_27 = 0x8000000;
        const unused_bit_28 = 0x10000000;
        const unused_bit_29 = 0x20000000;
        const unused_bit_30 = 0x40000000;
        const unused_bit_31 = 0x80000000;
        const unused_bit_32 = 0x100000000;
        const unused_bit_33 = 0x200000000;
        const unused_bit_34 = 0x400000000;
        const unused_bit_35 = 0x800000000;
        const unused_bit_36 = 0x1000000000;
        const unused_bit_37 = 0x2000000000;
        const unused_bit_38 = 0x4000000000;
        const unused_bit_39 = 0x8000000000;
        const unused_bit_40 = 0x10000000000;
        const unused_bit_41 = 0x20000000000;
        const navigation = 0x40000000000;
        const ServiceWorker = 0x80000000000;
        const edge_webview = 0x100000000000;
        const diagnostic_event = 0x200000000000;
        const __OTHER_EVENTS = 0x400000000000;
        const __DISABLED_OTHER_EVENTS = 0x800000000000;
    }
}

pub fn chrome_xperf_args(props: &ElevatedRecordingProps) -> Vec<String> {
    let mut providers = vec![];

    if !props.browsers {
        return providers;
    }

    // JIT symbols
    providers.push("Microsoft-JScript:0x3".to_string());

    // UserTiming trace events
    let enabled_keywords = KeywordNames::blink_user_timing;
    providers.push(format!(
        "{}:{:#x}",
        CHROME_PROVIDER_GUID,
        enabled_keywords.bits()
    ));

    providers
}
