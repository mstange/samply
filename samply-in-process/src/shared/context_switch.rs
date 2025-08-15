/// Accumulates thread running times (for "CPU deltas") and simulates off-cpu sampling,
/// with the help of context switch events.
///
/// In the Firefox Profiler format, a sample's "CPU delta" is the accumulated duration
/// for which the thread was running on the CPU since the previous sample.
///
/// # Off-CPU sampling
///
/// The goal of off-cpu sampling is to know what happened on the thread between on-CPU
/// samples, with about the same "accuracy" as for on-CPU samples.
///
/// There can be lots of context switch events and we don't want to flood the profile
/// with a sample for each context switch.
///
/// However, we also don't want to enforce a "minimum sleep time" because doing so
/// would skew the weighting of short sleeps. Really, what we're after is something
/// that looks comparable to what a wall-clock profiler would produce.
///
/// We solve this by accumulating the off-cpu duration of all thread sleeps, regardless
/// of the individual sleep length. Once the accumulated off-cpu duration exceeds a
/// threshold (a multiple of the off-cpu "sampling interval"), we emit a sample.
///
/// ## Details
///
/// A thread is either running (= on-cpu) or sleeping (= off-cpu).
///
/// The running time is the time between switch-in and switch-out.
/// The sleeping time is the time between switch-out and switch-in.
///
/// After every thread sleep, we accumulate the off-cpu time.
/// Now there are two cases:
///
/// Does the accumulated time cross an "off-cpu sampling" threshold?
/// If yes, turn it into an off-cpu sampling group and consume a multiple of the interval.
/// If no, don't emit any samples. The next sample's cpu delta will just be smaller.
pub struct ContextSwitchHandler {
    off_cpu_sampling_interval_ns: u64,
}

impl ContextSwitchHandler {
    pub fn new(off_cpu_sampling_interval_ns: u64) -> Self {
        Self {
            off_cpu_sampling_interval_ns,
        }
    }

    pub fn handle_switch_out(&self, timestamp: u64, thread: &mut ThreadContextSwitchData) {
        match &thread.state {
            ThreadState::Unknown => {
                // This "switch-out" is the first time we've heard of the thread. So it must
                // have been running until just now, but we didn't get any samples from it.

                // Just store the new state.
                thread.state = ThreadState::Off {
                    off_switch_timestamp: timestamp,
                };
            }

            ThreadState::On {
                last_observed_on_timestamp,
            } => {
                // The thread was running and is now context-switched out.
                // Accumulate the running time since we last saw it. This delta will be picked
                // up by the next sample we emit.
                let on_duration = timestamp - last_observed_on_timestamp;
                thread.on_cpu_duration_since_last_sample += on_duration;

                thread.state = ThreadState::Off {
                    off_switch_timestamp: timestamp,
                };
            }
            ThreadState::Off { .. } => {
                // We are already in the Off state but received another Switch-Out record.
                // This is unexpected; Switch-Out records are the only records that can
                // get us into the Off state and we do not expect two Switch-Out records
                // without an in-between Switch-In record.
                // However, in practice this case has been observed due to a duplicated
                // Switch-Out record in the perf.data file: the record just appeared twice
                // right after itself, same timestamp, same everything. I don't know if
                // this duplication indicates a bug in the kernel or in the perf tool or
                // maybe is not considered a bug at all.
            }
        }
    }

    pub fn handle_switch_in(
        &self,
        timestamp: u64,
        thread: &mut ThreadContextSwitchData,
    ) -> Option<OffCpuSampleGroup> {
        let off_cpu_sample = match thread.state {
            ThreadState::On {
                last_observed_on_timestamp,
            } => {
                // We are already in the On state, most likely due to a Sample record which
                // arrived just before the Switch-In record.
                // This is quite normal. Thread switching is done by some kernel code which
                // executes on the CPU, and this CPU work can get sampled before the CPU gets
                // to the code that emits the Switch-In record.
                let on_duration = timestamp - last_observed_on_timestamp;
                thread.on_cpu_duration_since_last_sample += on_duration;

                None
            }
            ThreadState::Off {
                off_switch_timestamp,
            } => {
                // The thread was sleeping and is now starting to run again.
                // Accumulate the off-cpu time.
                let off_duration = timestamp - off_switch_timestamp;
                thread.off_cpu_duration_since_last_off_cpu_sample += off_duration;

                // We just added some off-cpu time. If the accumulated off-cpu time exceeds the
                // off-cpu sampling interval, we want to consume some of it and turn it into an
                // off-cpu sampling group.
                self.maybe_consume_off_cpu(timestamp, thread)
            }
            ThreadState::Unknown => {
                // This "switch-in" is the first time we've heard of the thread.
                // It must have been sleeping at some stack, but we don't know in what stack,
                // so it seems pointless to emit a sample for the sleep time. We also don't
                // know what the reason for the "sleep" was: It could have been because the
                // thread was blocked (most common) or because it was pre-empted.

                None
            }
        };

        thread.state = ThreadState::On {
            last_observed_on_timestamp: timestamp,
        };

        off_cpu_sample
    }

    pub fn handle_on_cpu_sample(
        &self,
        timestamp: u64,
        thread: &mut ThreadContextSwitchData,
    ) -> Option<OffCpuSampleGroup> {
        let off_cpu_sample = match thread.state {
            ThreadState::On {
                last_observed_on_timestamp,
            } => {
                // The last time we heard from this thread, it was already running.
                // Accumulate the running time.
                let on_duration = timestamp - last_observed_on_timestamp;
                thread.on_cpu_duration_since_last_sample += on_duration;

                None
            }
            ThreadState::Off {
                off_switch_timestamp,
            } => {
                // The last time we heard from this thread, it was being context switched away from.
                // We are processing a sample on it so we know it is running again. Treat this sample
                // as a switch-in event.
                let off_duration = timestamp - off_switch_timestamp;
                thread.off_cpu_duration_since_last_off_cpu_sample += off_duration;

                // We just added some off-cpu time. If the accumulated off-cpu time exceeds the
                // off-cpu sampling interval, we want to consume some of it and turn it into an
                // off-cpu sampling group.
                self.maybe_consume_off_cpu(timestamp, thread)
            }
            ThreadState::Unknown => {
                // This sample is the first time we've ever head from a thread.
                // We don't know whether it was running or sleeping.
                // Do nothing. The first sample will have a CPU delta of 0.

                None
            }
        };

        thread.state = ThreadState::On {
            last_observed_on_timestamp: timestamp,
        };

        off_cpu_sample
    }

    fn maybe_consume_off_cpu(
        &self,
        timestamp: u64,
        thread: &mut ThreadContextSwitchData,
    ) -> Option<OffCpuSampleGroup> {
        // If the accumulated off-cpu time exceeds the off-cpu sampling interval,
        // we want to consume some of it and turn it into an off-cpu sampling group.
        let interval = self.off_cpu_sampling_interval_ns;
        if thread.off_cpu_duration_since_last_off_cpu_sample < interval {
            return None;
        }

        // Let's turn the accumulated off-cpu time into an off-cpu sample group.
        let sample_count = thread.off_cpu_duration_since_last_off_cpu_sample / interval;
        debug_assert!(sample_count >= 1);

        let consumed_duration = sample_count * interval;
        let remaining_duration =
            thread.off_cpu_duration_since_last_off_cpu_sample - consumed_duration;

        let begin_timestamp =
            timestamp - (thread.off_cpu_duration_since_last_off_cpu_sample - interval);
        let end_timestamp = timestamp - remaining_duration;
        debug_assert_eq!(
            end_timestamp - begin_timestamp,
            (sample_count - 1) * interval
        );

        // Consume the consumed duration and save the leftover duration.
        thread.off_cpu_duration_since_last_off_cpu_sample = remaining_duration;

        Some(OffCpuSampleGroup {
            begin_timestamp,
            end_timestamp,
            sample_count,
        })
    }

    pub fn consume_cpu_delta(&self, thread: &mut ThreadContextSwitchData) -> u64 {
        std::mem::replace(&mut thread.on_cpu_duration_since_last_sample, 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffCpuSampleGroup {
    pub begin_timestamp: u64,
    pub end_timestamp: u64,
    pub sample_count: u64,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ThreadContextSwitchData {
    state: ThreadState,
    on_cpu_duration_since_last_sample: u64,
    off_cpu_duration_since_last_off_cpu_sample: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum ThreadState {
    #[default]
    Unknown,
    Off {
        off_switch_timestamp: u64,
    },
    On {
        last_observed_on_timestamp: u64,
    },
}

#[cfg(test)]
mod test {
    use super::{ContextSwitchHandler, OffCpuSampleGroup, ThreadContextSwitchData};

    #[test]
    fn it_works() {
        // sampling interval: 10
        //
        // 0         10        20        30        40        50        60
        // 01234567890123456789012345678901234567890123456789012345678901
        // ===__========__=_____==____===___________________=============
        //             ^           v            v         v   ^         ^
        //
        // Graph legend:
        //  = Thread is running.
        //  _ Thread is sleeping.
        //  ^ On-cpu sample
        //  v Off-cpu sample

        let mut thread = ThreadContextSwitchData::default();
        let handler = ContextSwitchHandler::new(10);
        let s = handler.handle_switch_in(0, &mut thread);
        assert_eq!(s, None);
        handler.handle_switch_out(3, &mut thread);
        let s = handler.handle_switch_in(5, &mut thread);
        assert_eq!(s, None);
        let s = handler.handle_on_cpu_sample(12, &mut thread);
        let delta = handler.consume_cpu_delta(&mut thread);
        assert_eq!(s, None);
        assert_eq!(delta, 10);
        handler.handle_switch_out(13, &mut thread);
        let s = handler.handle_switch_in(15, &mut thread);
        assert_eq!(s, None);
        handler.handle_switch_out(16, &mut thread);
        let s = handler.handle_switch_in(21, &mut thread);
        assert_eq!(s, None);
        handler.handle_switch_out(23, &mut thread);
        let s = handler.handle_switch_in(27, &mut thread);
        assert_eq!(
            s,
            Some(OffCpuSampleGroup {
                begin_timestamp: 24,
                end_timestamp: 24,
                sample_count: 1
            })
        );
        let delta = handler.consume_cpu_delta(&mut thread);
        assert_eq!(delta, 4);
        handler.handle_switch_out(30, &mut thread);
        let s = handler.handle_switch_in(48, &mut thread);
        assert_eq!(
            s,
            Some(OffCpuSampleGroup {
                begin_timestamp: 37,
                end_timestamp: 47,
                sample_count: 2
            })
        );
        let delta = handler.consume_cpu_delta(&mut thread);
        assert_eq!(delta, 3);
        let s = handler.handle_on_cpu_sample(51, &mut thread);
        let delta = handler.consume_cpu_delta(&mut thread);
        assert_eq!(s, None);
        assert_eq!(delta, 3);
        let s = handler.handle_on_cpu_sample(61, &mut thread);
        let delta = handler.consume_cpu_delta(&mut thread);
        assert_eq!(s, None);
        assert_eq!(delta, 10);
    }
}
