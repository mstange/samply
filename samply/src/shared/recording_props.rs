use std::{path::PathBuf, time::Duration};

pub struct RecordingProps {
    pub output_file: PathBuf,
    pub time_limit: Option<Duration>,
    pub interval: Duration,
}

pub struct ConversionProps {
    pub profile_name: String,
    /// Merge non-overlapping threads of the same name.
    pub merge_threads: bool,
    /// Fold repeated frames at the base of the stack.
    pub fold_recursive_prefix: bool,
}
