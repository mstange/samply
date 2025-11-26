# Samply Markers Examples

This directory contains runnable examples demonstrating the `samply-markers` crate.

> [!NOTE]
> These commands work from any directory in the workspace. The `target` directory is always at the workspace root.

## Examples

### 01) Fibonacci

Demonstrates recursive function-call tracing of the [Fibonacci Sequence].

**[macOS][01-fib-example-macos] | [Ubuntu][01-fib-example-ubuntu] | Windows (not yet supported)**

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

Demonstrates sequential vs. parallel async network requests with [reqwest].

**[macOS][02-network-example-macos] | [Ubuntu][02-network-example-ubuntu] | Windows (not yet supported)**

```bash
# Build samply
cargo build --release -p samply

# Build the example
cargo build -p samply-markers --example 02_network_requests --profile profiling --features enabled

# Profile
TARGET=$(cargo metadata --format-version 1 | jq -r '.target_directory')
$TARGET/release/samply record $TARGET/profiling/examples/02_network_requests
```

---

### 03) Tokio Task Migration

Demonstrates async task migration across worker threads in a multi-threaded [tokio] runtime.

**[macOS][03-tokio-example-macos] | [Ubuntu][03-tokio-example-ubuntu] | Windows (not yet supported)**

```bash
# Build samply
cargo build --release -p samply

# Build the example
cargo build -p samply-markers --example 03_tokio_task_migration --profile profiling --features enabled

# Profile
TARGET=$(cargo metadata --format-version 1 | jq -r '.target_directory')
$TARGET/release/samply record $TARGET/profiling/examples/03_tokio_task_migration
```

---

### 04) Parallel Computation

Demonstrates sequential vs. parallel computation using [rayon].

**[macOS][04-parallel-example-macos] | [Ubuntu][04-parallel-example-ubuntu] | Windows (not yet supported)**

> [!IMPORTANT]
> This example isn't working correctly with the current FlushOnDrop implemenation.
> Rayon's threads live forever, so the per-thread buffers never flush to their files.
>
> This example works fine if you force a flush on write, but that is quite slow.
>
> I'm going to have to revisit the per-thread writing strategy to cover this case.


```bash
# Build samply
cargo build --release -p samply

# Build the example
cargo build -p samply-markers --example 04_parallel_computation --profile profiling --features enabled

# Profile
TARGET=$(cargo metadata --format-version 1 | jq -r '.target_directory')
$TARGET/release/samply record $TARGET/profiling/examples/04_parallel_computation
```

[01-fib-example-macos]: https://share.firefox.dev/4oMKHtT
[01-fib-example-ubuntu]: https://share.firefox.dev/4o3wur4
[02-network-example-macos]: https://share.firefox.dev/48eHTj8
[02-network-example-ubuntu]: https://share.firefox.dev/3LzT6lJ
[03-tokio-example-macos]: https://share.firefox.dev/3X3Wl7k
[03-tokio-example-ubuntu]: https://share.firefox.dev/43vXgB9
[04-parallel-example-macos]: https://share.firefox.dev/4iabqhw
[04-parallel-example-ubuntu]: https://share.firefox.dev/4oSvBDe
[jq]: https://jqlang.github.io/jq/
[cargo metadata]: https://doc.rust-lang.org/cargo/commands/cargo-metadata.html
[Fibonacci Sequence]: https://en.wikipedia.org/wiki/Fibonacci_sequence
[rayon]: https://crates.io/crates/rayon
[reqwest]: https://crates.io/crates/reqwest
[samply]: https://github.com/mstange/samply?tab=readme-ov-file#samply
[tokio]: https://crates.io/crates/tokio
