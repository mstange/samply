# samply

samply is a command line CPU profiler which uses the [Firefox profiler](https://profiler.firefox.com/) as its UI.

At the moment it runs on macOS and Linux. Windows support is planned. samply is still under development and far from finished, but works quite well already.

Give it a try:

```
% cargo install samply
% samply record ./your-command your-arguments
```

This spawns `./your-command your-arguments` in a subprocess and records a profile of its execution. When the command finishes, samply opens
[profiler.firefox.com](https://profiler.firefox.com/) in your default browser, loads the recorded profile in it, and starts a local webserver which serves symbol information and source code.

Then you can inspect the profile. And you can upload it.

Here's an example: https://share.firefox.dev/3j3PJoK

This is a profile of [dump_syms](https://github.com/mozilla/dump_syms), running on macOS, recorded as follows:

```
samply record ./dump_syms ~/mold-opt-libxul.so > /dev/null
```

You can see which functions were running for how long. You can see flame graphs and timelines. You can double-click functions in the call tree to open the source view, and see which lines of code were sampled how many times.

All data is kept locally (on disk and in RAM) until you choose to upload your profile.

samply is a sampling profiler and collects stack traces, per thread, at some sampling interval (the default 1000Hz, i.e. 1ms). On macOS, both on- and off-cpu samples are collected (so you can see under which stack you were blocking on a lock, for example). On Linux, only on-cpu samples are collected at the moment.

On Linux, as samply needs access to performance events system by unprivileged users, run:

```
sudo sysctl kernel.perf_event_paranoid=1 
```

If you still get a `mmap failed` error (an `EPERM`), you might also need to increase the `mlock` limit, e.g.:

```
sudo sysctl kernel.perf_event_mlock_kb=2048
```

## Examples

Here's a profile from `samply record rustup check`: https://share.firefox.dev/3hteKZZ

I'll add some Linux examples when I get a chance.

## Turn on debug info for full stacks

If you profile Rust code, make sure to profile a binary which was compiled **in release mode** and **with debug info**. This will give you inline stacks and a working source code view.

The best way is the following:

 1. Create a global cargo profile called `profiling`, see below how.
 2. Compile with `cargo build --profile profiling`.
 3. Record with `samply record ./target/profiling/yourrustprogram`.

To create the `profiling` cargo profile, create a text file at `~/.cargo/config.toml` with the following content:

```toml
[profile.profiling]
inherits = "release"
debug = true
```

Similar advice applies to other compiled languages. For C++, you'll want to make sure the `-g` flag is included in the compiler invocation.

## Known issues

On macOS, samply cannot profile system commands, such as the `sleep` command or system `python`. This is because system executables are signed in such a way that they block the `DYLD_INSERT_LIBRARIES` environment variable, which breaks samply's ability to siphon out the `mach_port` of the process.

But you can profile any binaries that you've compiled yourself, or which are unsigned or locally-signed (such as anything installed by `cargo install` or by [Homebrew](brew.sh)).

## License

Licensed under either of

  * Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
  * MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
