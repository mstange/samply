# samply

samply is a command line CPU profiler which uses the [Firefox profiler](https://profiler.firefox.com/) as its UI.

samply works on macOS, Linux, and Windows.

In order to profile the execution of `./my-application`, prepend `samply record` to the command invocation:

```sh
samply record ./my-application my-arguments
```

On Linux, samply uses perf events. You can grant temporary access by running:

```sh
echo '1' | sudo tee /proc/sys/kernel/perf_event_paranoid
```

Visit [the git repository](https://github.com/mstange/samply/) for more information.
