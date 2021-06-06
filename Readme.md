# perfrecord

This is a work in progress and not ready for public consumption.

`perfrecord` is a macOS-only command line CPU profiler that displays the result
in the [Firefox profiler](https://profiler.firefox.com/).

Try it out now:

```
% cargo install perfrecord
% perfrecord ./your-command your-arguments
```

This collects a profile of the `./your-command your-arguments` command and saves it to a file. Then it opens
your default browser, loads the profile in it, and runs a local webserver so that profiler.firefox.com
can symbolicate the profile and show source code and assembly code on demand.

The captured data is similar to that of the "CPU Profiler" in Instruments.
`perfrecord` is a sampling profiler that collects stack traces, per thread, at some sampling interval.
In the future it should support sampling based on wall-clock time ("All thread states") and CPU time.

`perfrecord` does not require sudo privileges for profiling (non-signed) processes that it launches itself.

## Other examples

`perfrecord rustup check` generates [this profile](https://share.firefox.dev/2MfPzak).

Profiling system-provided command line tools is not straightforward because of system-integrity protection.
Here's an example for profiling `sleep`:

```
cat /bin/sleep > /tmp/sleep; chmod +x /tmp/sleep
perfrecord /tmp/sleep 2
```

It produces [this profile](https://share.firefox.dev/2ZRmN7H).


## Why?

This is meant to be an alternative to the existing profilers on macOS:

 - Instruments
 - the `sample` command line tool
 - the dtrace scripts that people use to create flame graphs on macOS.

It is meant to overcome the following shortcomings:

 - `sample` and the dtrace `@[ustack()] = count();` script do not capture sample timestamps. They only capture aggregates. This makes it impossible to see the sequence of execution.
 - The Instruments command line tool does not allow specifying the sampling interval or to capture all-thread-states (wall-clock time-based) profiles. This means that you often have to initiate profiling from the UI, which can be cumbersome.
 - Instruments is not open source.
 - Instruments only profiles a single process (or all processes system-wide). It would be nice to have a profiler that can follow the process subtree of a command.
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

We use these primitives to walk the stack and enumerate shared libraries.

At the moment, only frame pointer stack walking is implemented. This is usually fine because keeping frame pointers is the default on macOS.
