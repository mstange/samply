# samply

`samply` is intended to become a command line CPU profiler for macOS, Linux and Windows.

At the moment, the macOS implementation works best.
The Linux implementation is extremely new and experimental.
There is no Windows implementation yet.

samply is a work in progress and not ready for public consumption, but you can give it a try if you'd like:

```
cargo install samply

samply record ./yourcommand args   # This profiles yourcommand and then opens the profile in a viewer.

# Alternatively:
samply record --save-only -o prof.json -- ./yourcommand args
samply load prof.json

# You can also import Linux perf profiles:
samply import perf.data
```

See [the repo](https://github.com/mstange/samply/) for more information.

This project was formerly known as perfrecord.
