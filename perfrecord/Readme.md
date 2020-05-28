# perfrecord

`perfrecord` is intended to become a command line CPU profiler for macOS.
It is a work in progress and not ready for public consumption.

Try it out:

```
cargo install perfrecord

perfrecord ./yourcommand args
perfrecord --launch-when-done ./yourcommand args
perfrecord -o prof.json ./yourcommand args
perfrecord --launch prof.json
```

See [the repo](https://github.com/mstange/perfrecord/) for more information.
