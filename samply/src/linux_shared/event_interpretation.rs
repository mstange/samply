use std::collections::HashMap;
use std::fmt::Debug;

use linux_perf_data::{linux_perf_event_reader, AttributeDescription};
use linux_perf_event_reader::{AttrFlags, PerfEventType, SamplingPolicy, SoftwareCounterType};

use crate::shared::{
    event_display::{EventDisplay, EventDisplaySelector},
    prop_types::ProfileCreationProps,
};

#[derive(Debug, Clone)]
pub enum KnownEvent {
    RssStat,
    MmapEnter,
    MmapExit,
    MprotectEnter,
    PageFault,
    Display(EventDisplay),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OffCpuIndicator {
    /// We can see when threads go off-CPU and back with CONTEXT_SWITCH records.
    ContextSwitches,
    /// We can use sched_switch samples to see when threads go off-CPU, and
    /// "main event" (e.g. cpu-cycles) samples to see when they come back on-CPU.
    SchedSwitchAndSamples,
}

#[derive(Debug, Clone)]
pub struct EventInterpretation {
    pub main_event_attr_index: usize,
    #[allow(unused)]
    pub main_event_name: String,
    pub sampling_is_time_based: Option<u64>,
    pub off_cpu_indicator: Option<OffCpuIndicator>,
    pub sched_switch_attr_index: Option<usize>,
    pub known_event_indices: HashMap<usize, KnownEvent>,
    pub event_names: Vec<String>,
}

impl EventInterpretation {
    pub fn divine_from_attrs(
        attrs: &[AttributeDescription],
        events_display: &[EventDisplaySelector],
    ) -> Self {
        dbg!(events_display.len());
        let main_selector_i = events_display
            .iter()
            .position(|e| matches!(e.display, EventDisplay::TimingData))
            .expect(&format!(
                "Can only convert profiles with an event displayed as timing_data. Got: {}",
                events_display
                    .iter()
                    .map(|e| e.to_string() + ", ")
                    .collect::<String>()
            ));
        let mut other_selectors = events_display.to_vec();
        let main_selector = other_selectors.swap_remove(main_selector_i);

        let event_names: Vec<String> = attrs
            .iter()
            .enumerate()
            .map(|(attr_index, attr_desc)| {
                attr_desc
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("<unknown event {attr_index}>"))
            })
            .collect();

        let main_event_attr_index = event_names
            .iter()
            .enumerate()
            .position(|(i, attr)| main_selector.selector.matches(i, attr))
            .expect(&format!(
                "Could not find event with timing_data display in events. Searched using: {}",
                main_selector.selector
            ));

        let main_event_name = attrs[main_event_attr_index]
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
        let have_context_switches = attrs[main_event_attr_index]
            .attr
            .flags
            .contains(AttrFlags::CONTEXT_SWITCH);
        let sched_switch_attr_index = attrs
            .iter()
            .position(|attr_desc| attr_desc.name.as_deref() == Some("sched:sched_switch"));
        let off_cpu_indicator = match (have_context_switches, sched_switch_attr_index) {
            (true, _) => Some(OffCpuIndicator::ContextSwitches),
            (false, Some(_)) => Some(OffCpuIndicator::SchedSwitchAndSamples),
            _ => None,
        };
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
        for (i, name) in event_names.iter().enumerate() {
            if i == main_event_attr_index
                || Some(i) == sched_switch_attr_index
                || known_event_indices.contains_key(&i)
            {
                continue;
            }
            for selector in other_selectors.iter() {
                if selector.selector.matches(i, name) {
                    known_event_indices.insert(i, KnownEvent::Display(selector.display));
                };
            }
        }
        if known_event_indices.len() + 1 < attrs.len() {
            eprintln!(
                "The following events were not matched to any event display, and will be ignored:"
            );
            for (i, name) in event_names.iter().enumerate() {
                if i == main_event_attr_index
                    || Some(i) == sched_switch_attr_index
                    || known_event_indices.contains_key(&i)
                {
                    continue;
                }
                eprintln!("\t{i}. {name}")
            }
        }

        Self {
            main_event_attr_index,
            main_event_name,
            sampling_is_time_based,
            off_cpu_indicator,
            sched_switch_attr_index,
            known_event_indices,
            event_names,
        }
    }
}
