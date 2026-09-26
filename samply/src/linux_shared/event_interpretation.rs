use std::collections::HashMap;
use std::fmt::Debug;

use linux_perf_data::{linux_perf_event_reader, AttributeDescription};
use linux_perf_event_reader::{AttrFlags, PerfEventType, SamplingPolicy, SoftwareCounterType};

#[derive(Debug, Clone)]
pub enum KnownEvent {
    RssStat,
    MmapEnter,
    MmapExit,
    MprotectEnter,
    PageFault,
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
    pub main_event_name: String,
    pub sampling_is_time_based: Option<u64>,
    pub off_cpu_indicator: Option<OffCpuIndicator>,
    pub sched_switch_attr_index: Option<usize>,
    pub known_event_indices: HashMap<usize, KnownEvent>,
    pub event_names: Vec<String>,
    /// The fixed sampling period of each attribute, in attribute order.
    /// `None` for frequency-based and non-sampling attributes.
    pub fixed_periods: Vec<Option<u64>>,
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

        let fixed_periods =
            fixed_periods(attrs.iter().map(|attr_desc| attr_desc.attr.sampling_policy));

        Self {
            main_event_attr_index,
            main_event_name,
            sampling_is_time_based,
            off_cpu_indicator,
            sched_switch_attr_index,
            known_event_indices,
            event_names,
            fixed_periods,
        }
    }
}

/// The fixed period of each attribute that samples every N events, in
/// order. Frequency-based attributes get `None`, because each of their
/// records carries its own period.
pub fn fixed_periods(policies: impl IntoIterator<Item = SamplingPolicy>) -> Vec<Option<u64>> {
    policies
        .into_iter()
        .map(|policy| match policy {
            SamplingPolicy::Period(period) => Some(period.get()),
            SamplingPolicy::Frequency(_) | SamplingPolicy::NoSampling => None,
        })
        .collect()
}

/// Value of an attribute's `Perf events` entry: `frequency N Hz` or
/// `period N`. Non-sampling attributes, which produce no samples, read
/// `no sampling`.
pub fn sampling_description(policy: SamplingPolicy) -> String {
    match policy {
        SamplingPolicy::Frequency(hz) => format!("frequency {hz} Hz"),
        SamplingPolicy::Period(period) => format!("period {period}"),
        SamplingPolicy::NoSampling => "no sampling".to_string(),
    }
}

/// Entries of the `Perf events` info section: one per attribute in
/// attribute order, labeled with the event name and valued with its
/// sampling, then `Sample weight`, which reads `period` with period weights
/// and `1` otherwise. The first entry is the main event.
pub fn perf_events_section(
    event_names: &[String],
    policies: &[SamplingPolicy],
    weight_by_period: bool,
) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = event_names
        .iter()
        .zip(policies)
        .map(|(name, policy)| (name.clone(), sampling_description(*policy)))
        .collect();
    let weight = if weight_by_period { "period" } else { "1" };
    entries.push(("Sample weight".to_string(), weight.to_string()));
    entries
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;

    #[test]
    fn fixed_periods_come_from_period_attributes_only() {
        let policies = [
            SamplingPolicy::Frequency(999),
            SamplingPolicy::Period(NonZeroU64::new(10_000).unwrap()),
            SamplingPolicy::NoSampling,
        ];
        assert_eq!(fixed_periods(policies), vec![None, Some(10_000), None]);
    }

    #[test]
    fn perf_events_section_lists_every_attribute_then_the_weight_mode() {
        let names = vec![
            "cycles:u".to_string(),
            "cache-misses".to_string(),
            "<unknown event 2>".to_string(),
        ];
        let policies = [
            SamplingPolicy::Frequency(999),
            SamplingPolicy::Period(NonZeroU64::new(10_000).unwrap()),
            SamplingPolicy::NoSampling,
        ];
        assert_eq!(
            perf_events_section(&names, &policies, false),
            vec![
                ("cycles:u".to_string(), "frequency 999 Hz".to_string()),
                ("cache-misses".to_string(), "period 10000".to_string()),
                ("<unknown event 2>".to_string(), "no sampling".to_string()),
                ("Sample weight".to_string(), "1".to_string()),
            ]
        );
        assert_eq!(
            perf_events_section(&names, &policies, true).last(),
            Some(&("Sample weight".to_string(), "period".to_string()))
        );
    }
}
