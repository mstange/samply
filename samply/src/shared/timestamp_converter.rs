use fxprof_processed_profile::Timestamp;

pub struct TimestampConverter {
    reference_ns: u64,
}

impl TimestampConverter {
    pub fn with_reference_timestamp(reference_ns: u64) -> Self {
        Self { reference_ns }
    }

    pub fn convert_time(&self, ktime_ns: u64) -> Timestamp {
        Timestamp::from_nanos_since_reference(ktime_ns.saturating_sub(self.reference_ns))
    }
}
