# Samply Markers Examples

This directory contains runnable examples demonstrating the `samply-markers` crate.

> [!NOTE]
> These commands work from any directory in the workspace. The `target` directory is always at the workspace root.

## Examples

### 01) Fibonacci

Demonstrates recursive function call tracking using the `samply_measure!` macro.

- [macOS][fib-example-macos] | [Ubuntu][fib-example-ubuntu] | Windows (not yet supported)

```bash
# Build samply
cargo build --release -p samply

# Build the example
cargo build -p samply-markers --example 01_fibonacci --profile profiling --features enabled

# Profile
TARGET=$(cargo metadata --format-version 1 | jq -r '.target_directory')
$TARGET/release/samply record $TARGET/profiling/examples/01_fibonacci
```

---

### 02) Network Requests

A complete async example demonstrating interval markers, instant markers, and timers.

- [macOS][network-example-macos] | [Ubuntu][network-example-ubuntu] | Windows (not yet supported)

```bash
# Build samply
cargo build --release -p samply

# Build the example
cargo build -p samply-markers --example 02_network_requests --profile profiling --features enabled

# Profile
TARGET=$(cargo metadata --format-version 1 | jq -r '.target_directory')
$TARGET/release/samply record $TARGET/profiling/examples/02_network_requests
```

[cargo metadata]: https://doc.rust-lang.org/cargo/commands/cargo-metadata.html
[fib-example-macos]: https://share.firefox.dev/43vY4G8
[fib-example-ubuntu]: https://share.firefox.dev/3XAGkpu
[network-example-macos]: https://share.firefox.dev/3M2zMxq
[network-example-ubuntu]: https://share.firefox.dev/4r7bgeW
[jq]: https://jqlang.github.io/jq/
[samply]: https://github.com/mstange/samply?tab=readme-ov-file#samply
