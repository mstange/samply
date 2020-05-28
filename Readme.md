# perfrecord

`perfrecord` is a macOS-only command line CPU profiler that displays the result
in the [Firefox profiler](https://profiler.firefox.com/).

Try it out now:

```
% cargo install perfrecord
% perfrecord --launch-when-done ./your-command your-arguments
```

This collects a profile of the `./your-command your-arguments` command and saves it to a file. Then it opens
your default browser, loads the profile in it, and runs a local webserver so that profiler.firefox.com
can symbolicate the profile and show source code and assembly code on demand.

Tha captured data is similar to that of the "CPU Profiler" in Instruments.
`perfrecord` is a sampling profiler that collects stack traces, per thread, at some sampling interval.
In the future it supports sampling based on wall-clock time ("All thread states") and CPU time.

`perfrecord` does not require sudo privileges for profiling (non-signed) processes that it launches itself.

## Other examples

`perfrecord --launch-when-done rustup check` generates [this profile](https://deploy-preview-2556--perf-html.netlify.app/public/7c64cd279d674a29ae445a4eb3b7e046748083da/calltree/?globalTrackOrder=0-1-2-3-4-5-6-7&thread=5&timelineType=stack&v=4).

Profiling system-provided command line tools is not straightforward because of system-integrity protection.
Here's an example for profiling `sleep`:

```
cat /bin/sleep > /tmp/sleep; chmod +x /tmp/sleep
perfrecord --launch-when-done /tmp/sleep 2
```

It produces [this profile](https://deploy-preview-2556--perf-html.netlify.app/public/ffc4c3a15a6da64c1e6b7ecdc7d4ffc37d41c032/calltree/?globalTrackOrder=0&thread=0&timelineType=stack&v=4).


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
The Symbolication framework has an Objective C class called `VMUSampler` that provides
stack sampling functionality.
The use of `VMUSampler` is intended to be temporary. `VMUSampler` runs into the same
performance problem as Instruments with local Firefox binaries because it tries to
symbolicate eagerly.
