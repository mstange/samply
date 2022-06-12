# fxprof-perf-convert

A converter from the Linux perf `perf.data` format into the [Firefox Profiler](https://profiler.firefox.com/) format, specifically into the [processed profile format](https://crates.io/crates/fxprof-processed-profile).

Here's an [example profile of Firefox](https://share.firefox.dev/37QbKlM). And here's [a profile of the conversion process of that example](https://share.firefox.dev/3wh6CQZ). Both of these profiles were obtained by running `perf record --call-graph dwarf` and then converting the perf.data file.

## Run

For best results, run perf record as root and use the following arguments to capture context switch events and off-cpu stacks:

```
$ sudo perf record -e cycles -e sched:sched_switch --switch-events --sample-cpu -m 8M --aio --call-graph dwarf,32768 --pid <pid>
$ sudo chown $USER perf.data
```

Then run the converter:

```
$ cargo run --release -- perf.data
```

This creates a file called `profile-conv.json`.

Then open the profile in the Firefox profiler:

```
$ profiler-symbol-server profile-conv.json   # Install with `cargo install profiler-symbol-server`
```

That's it.

## More command lines

If you don't want to attach to an existing process, and instead want to launch a new process, you can use something like this:

```
$ sudo perf record -e cycles -e sched:sched_switch --switch-events --sample-cpu -m 8M --aio --call-graph dwarf,32768 sudo -u $USER env "PATH=$PATH" sh -c 'YOUR COMMAND' && sudo chown $USER perf.data
```

It's not the best. If you know of a better way to make perf run as root and invoke a program as non-root, please let me know. Thanks!
