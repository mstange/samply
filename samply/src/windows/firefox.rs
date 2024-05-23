use bitflags::bitflags;

use super::elevated_helper::ElevatedRecordingProps;

// From https://searchfox.org/mozilla-central/rev/0e7394a77cdbe1df5e04a1d4171d6da67b57fa17/mozglue/baseprofiler/public/BaseProfilerMarkersPrerequisites.h#355-360
pub const PHASE_INSTANT: u8 = 0;
pub const PHASE_INTERVAL: u8 = 1;
pub const PHASE_INTERVAL_START: u8 = 2;
pub const PHASE_INTERVAL_END: u8 = 3;

/// The Firefox provider GUID, which is a hash of the string "Mozilla.FirefoxTraceLogger".
/// https://searchfox.org/mozilla-central/rev/010ccb86d48fa23b2874d1a7cbe6957ec78538c3/tools/profiler/core/ETWTools.cpp#14-32
pub const FIREFOX_PROVIDER_GUID: &str = "c923f508-96e4-5515-e32c-7539d1b10504";

bitflags! {
    // https://searchfox.org/mozilla-central/rev/010ccb86d48fa23b2874d1a7cbe6957ec78538c3/mozglue/baseprofiler/public/BaseProfilerMarkersPrerequisites.h#779-790
    #[derive(PartialEq, Eq)]
    pub struct EtwMarkerGroup: u64 {
        const Generic = 1;
        const UserMarkers = 1 << 1;
        const Memory = 1 << 2;
        const Scheduling = 1 << 3;
        const Text = 1 << 4;
        const Tracing = 1 << 5;
    }
}

pub fn firefox_xperf_args(props: &ElevatedRecordingProps) -> Vec<String> {
    let mut providers = vec![];

    if !props.browsers {
        return providers;
    }

    // JIT symbols
    providers.push("Microsoft-JScript:0x3".to_string());

    // UserTiming + GC markers
    let bits = (EtwMarkerGroup::UserMarkers | EtwMarkerGroup::Memory).bits();
    providers.push(format!("{}:{:#x}", FIREFOX_PROVIDER_GUID, bits));

    providers
}
