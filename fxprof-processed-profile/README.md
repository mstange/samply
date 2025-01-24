# fxprof-processed-profile

This crate allows you to create a profile that can be loaded into
the [Firefox Profiler](https://profiler.firefox.com/).

Specifically, this uses the ["Processed profile format"](https://github.com/firefox-devtools/profiler/blob/main/docs-developer/processed-profile-format.md).

Use [`Profile::new`] to create a new [`Profile`] object. Then add all the
information into it. To convert it to JSON, use [`serde_json`], for
example [`serde_json::to_writer`] or [`serde_json::to_string`].

## Example

```rust
use fxprof_processed_profile::{Profile, CategoryHandle, CpuDelta, Frame, FrameInfo, FrameFlags, SamplingInterval, Timestamp};
use std::time::SystemTime;

let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
profile.set_thread_name(thread, "Main thread");
let stack_frames = vec![
    FrameInfo { frame: Frame::Label(profile.intern_string("Root node")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() },
    FrameInfo { frame: Frame::Label(profile.intern_string("First callee")), category_pair: CategoryHandle::OTHER.into(), flags: FrameFlags::empty() }
];
let stack = profile.intern_stack_frames(thread, stack_frames.into_iter());
profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), stack, CpuDelta::ZERO, 1);

let writer = std::io::BufWriter::new(output_file);
serde_json::to_writer(writer, &profile)?;
```
