use linux_perf_data::linux_perf_event_reader;
use linux_perf_data::AttributeDescription;

use linux_perf_event_reader::{AttrFlags, PerfEventType, SamplingPolicy, SoftwareCounterType};

use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone)]
pub enum KnownEvent {
    RssStat,
    MmapEnter,
    MmapExit,
    MprotectEnter,
    PageFault,
}

#[derive(Debug, Clone)]
pub struct EventInterpretation {
    pub main_event_attr_index: usize,
    #[allow(unused)]
    pub main_event_name: String,
    pub sampling_is_time_based: Option<u64>,
    pub have_context_switches: bool,
    pub sched_switch_attr_index: Option<usize>,
    pub known_event_indices: HashMap<usize, KnownEvent>,
    pub event_names: Vec<String>,
}

impl EventInterpretation {
    pub fn divine_from_attrs(attrs: &[AttributeDescription]) -> Self {
        let main_event_attr_index = 0;
        let main_event_name = attrs[0]
            .name
            .as_deref()
            .unwrap_or("<unnamed event>")
            .to_string();
        let sampling_is_time_based = match (attrs[0].attr.type_, attrs[0].attr.sampling_policy) {
            (_, SamplingPolicy::NoSampling) => {
                panic!("Can only convert profiles with sampled events")
            }
            (_, SamplingPolicy::Frequency(freq)) => {
                let nanos = 1_000_000_000 / freq;
                Some(nanos)
            }
            (
                PerfEventType::Software(
                    SoftwareCounterType::CpuClock | SoftwareCounterType::TaskClock,
                ),
                SamplingPolicy::Period(period),
            ) => {
                // Assume that we're using a nanosecond clock. TODO: Check how we can know this for sure
                let nanos = u64::from(period);
                Some(nanos)
            }
            (_, SamplingPolicy::Period(_)) => None,
        };
        let have_context_switches = attrs[0].attr.flags.contains(AttrFlags::CONTEXT_SWITCH);
        let sched_switch_attr_index = attrs
            .iter()
            .position(|attr_desc| attr_desc.name.as_deref() == Some("sched:sched_switch"));
        let mut known_event_indices = HashMap::new();

        let known_events = [
            ("kmem:rss_stat", KnownEvent::RssStat),
            ("exceptions:page_fault_user", KnownEvent::PageFault),
            ("syscalls:sys_enter_mprotect", KnownEvent::MprotectEnter),
            ("syscalls:sys_enter_mmap", KnownEvent::MmapEnter),
            ("syscalls:sys_exit_mmap", KnownEvent::MmapExit),
        ];

        for (event_name, event) in known_events {
            let index = attrs
                .iter()
                .position(|attr_desc| attr_desc.name.as_deref() == Some(event_name));
            if let Some(index) = index {
                known_event_indices.insert(index, event);
            }
        }

        let event_names = attrs
            .iter()
            .enumerate()
            .map(|(attr_index, attr_desc)| {
                attr_desc
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("<unknown event {attr_index}>"))
            })
            .collect();

        Self {
            main_event_attr_index,
            main_event_name,
            sampling_is_time_based,
            have_context_switches,
            sched_switch_attr_index,
            known_event_indices,
            event_names,
        }
    }
}
