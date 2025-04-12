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

// Creates the following call tree:
//
// App process (pid: 54132) > Main thread (tid: 54132000)
//
// 1  0  Root node
// 1  1  - First callee

let mut profile = Profile::new("My app", SystemTime::now().into(), SamplingInterval::from_millis(1));
let process = profile.add_process("App process", 54132, Timestamp::from_millis_since_reference(0.0));
let thread = profile.add_thread(process, 54132000, Timestamp::from_millis_since_reference(0.0), true);
profile.set_thread_name(thread, "Main thread");

let root_node_string = profile.handle_for_string("Root node");
let root_frame = profile.handle_for_frame_with_label(thread, root_node_string, CategoryHandle::OTHER, FrameFlags::empty());
let first_callee_string = profile.handle_for_string("First callee");
let first_callee_frame = profile.handle_for_frame_with_label(thread, first_callee_string, CategoryHandle::OTHER, FrameFlags::empty());

let root_stack_node = profile.handle_for_stack(thread, root_frame, None);
let first_callee_node = profile.handle_for_stack(thread, first_callee_frame, Some(root_stack_node));
profile.add_sample(thread, Timestamp::from_millis_since_reference(0.0), Some(first_callee_node), CpuDelta::ZERO, 1);

let writer = std::io::BufWriter::new(output_file);
serde_json::to_writer(writer, &profile)?;
```
