# perfrecord

This is a work in progress.

The end goal is to have a macOS-only command line profiler that displays the result
in the [Firefox profiler](https://profiler.firefox.com/).

Once it's finished, it should look something like this:

```
% cargo install perfrecord
% perfrecord ./your-command your-arguments
% perfrecord --open profile.json
```

This would collect a profile of the `./your-command your-arguments` command and save it to a file. Then it should open
your default browser, load the profile in it, and run a local webserver so that profiler.firefox.com
can symbolicate the profile and show source code and assembly code on demand.

Tha captured data should be similar to that of the "CPU Profiler" in Instruments.
`perfrecord` should be a sampling profiler that collects stack traces, per thread, at some sampling interval,
and it should support sampling based on wall-clock time ("All thread states") and CPU time.

`perfrecord` should not require sudo privileges for profiling (non-signed) processes that it launches itself.

## Why?

This is meant to be an alternative to the existing profilers on macOS:

 - Instruments
 - the `sample` command line tool
 - the dtrace scripts that people use to create flame graphs on macOS.

It is meant to overcome the following shortcomings:

 - `sample` and the dtrace `@[ustack()] = count();` script do not capture sample timestamps. They only capture aggregates. This makes it impossible to see the sequence of execution.
 - The Instruments command line tool does not allow specifying the sampling interval or to capture all-thread-states (wall-clock time-based) profiles. This means that you often have to initiate profiling from the UI, which can be cumbersome.
 - Instruments is not open source.
 - Instruments is unusably slow when loading profiles of large binaries. For example, profiling a local Firefox build with debug information hangs the Instruments UI for ten minutes (!).
 - Instruments has bugs, lots of them.
 - It misses some features, such as certain call tree transforms, or Rust demangling.

The last two could be overcome by using Instruments just as a way to capture data, and then loading the .trace bundles in our own tool.

## Run the work in progress

```
cd perfrecord-preload
cargo build --release
cd ..
cd perfrecord
# Now open src/process_launcher.rs and edit `PRELOAD_LIB_PATH`

# And then run it:
cargo run --release -- your-command your-arguments

# Example (using sleep, but copied to a different place so that we can use DYLD_INSERT_LIBRARIES on it without needing to disable SIP):
cat /bin/sleep > /tmp/sleep; chmod +x /tmp/sleep
cargo run --release -- /tmp/sleep 2
```

## How does it work?

There are two main challenges here:

 1. Getting the `mach_task_self` of the launched child process into perfrecord.
 2. Obtaining stacks from the task.

### Getting the task

We get the task by injecting a library into the launched process using `DYLD_INSERT_LIBRARIES`.
The injected library establishes a mach connection to perfrecord during its module constructor,
and sends its `mach_task_self()` up to the perfrecord process.
This makes use of code from the [ipc-channel crate](https://github.com/servo/ipc-channel/)'s
mach implementation.

We can only get the task of binaries that are not signed or have entitlements.
Similar tools require you to use Xcode to create a build that has task_for_pid
entitelments to work around this restriction.

### Obtaining stacks

Once perfrecord has the `mach_port_t` for the child task, it has complete control over it.
It can enumerate threads, pause them at will, and read process memory.

In the future, I would like to obtain stacks using our own code, given these primitives.

However, for now, we make use of a private macOS framework called "Symbolication".
The Symbolication framework has two Objective C classes called `VMUSampler` and
`VMUProcessDescription` that provide exactly the type of functionality we need.
Obviously we don't want to rely on private frameworks long-term. But for now they
can get us started.
