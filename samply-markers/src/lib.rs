#![warn(missing_docs)]

//! # Samply Markers
//!
//! Emit profiler markers that [samply] records and displays in the [Firefox Profiler](https://profiler.firefox.com/) UI.
//!
//! <br>
//!
//! ## Quick Demo
//!
//! <br>
//!
//! The following Fibonacci Sequence example demonstrates recursive function call tracking.
//!
//! The [`samply_measure!`](crate::samply_measure) macro emits an interval marker for each recursive call,
//! allowing the profiler to display the complete call tree with timing information for every `fib(n)` invocation.
//!
//! **[macOS](https://share.firefox.dev/43vY4G8) |
//! [Ubuntu](https://share.firefox.dev/3XAGkpu) |
//! Windows (not yet supported)**
//!
//! ```rust,no_run
//! use samply_markers::prelude::*;
//!
//! fn fib(n: u64) -> u64 {
//!     samply_measure!({
//!         match n {
//!             0 | 1 => n,
//!             _ => fib(n - 1) + fib(n - 2),
//!         }
//!     }, marker: {
//!         name: format!("fib({n})"),
//!     })
//! }
//!
//! fn main() {
//!     let n = 10;
//!     let value = fib(n);
//!     println!("fib({n}) = {}", value);
//! }
//! ```
//!
//! <br>
//!
//! ## Project Configuration
//!
//! <br> **1)** Add [samply-markers](crate) as a dependency to your project's `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! samply-markers = "{version}"
//! ```
//!
//! <br> **2)** Add a `samply-markers` feature flag to your project's `Cargo.toml`.
//!
//! > Samply markers are designed to no-op by default, so they must be explicitly enabled in order
//! > to see them in profiles.
//! >
//! > * Using [samply-markers](crate) has effectively zero cost when not enabled.
//! > * Using [samply-markers](crate) does not pull in any additional dependencies when not enabled.
//!
//! ```toml
//! [features]
//! samply-markers = ["samply-markers/enabled"]
//! ```
//!
//! <br> **3)** Add a `profiling` profile to your project's `Cargo.toml`.
//!
//! * This step is optional, but highly recommended for profiling with [samply].
//!
//! ```toml
//! [profile.profiling]
//! inherits = "release"
//! debug = true
//! ```
//!
//! <br> **4)** Build your project for profiling, then record the resulting binary with [samply] to process the emitted markers.
//!
//! ```text
//! cargo build --release -p samply
//! cargo build --profile profiling --features samply-markers
//! samply record target/profiling/{binary}
//! ```
//!
//! <br>
//!
//! ## Public API
//!
//!
//! #### The [`samply_marker!`](crate::samply_marker) macro
//!
//! <br>
//!
//! The [`samply_marker!`](crate::samply_marker) macro is the foundational way to emit profiler markers.
//! It creates and emits a [`SamplyMarker`](marker::SamplyMarker) at the current location in your code,
//! supporting both instant markers (a single point in time) and interval markers (a span of time).
//!
//! ##### Instant Marker
//!
//! An instant marker marks a specific point in time:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn process_request(request_id: u32) {
//!     // Emit an instant marker at this exact moment
//!     samply_marker!({ name: format!("processing request {request_id}") });
//!
//!     // ... process the request.
//! }
//! ```
//!
//! ##### Interval Marker
//!
//! An interval marker spans from a start time to the current time using [`SamplyTimestamp`](marker::SamplyTimestamp):
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn query_database(query: &str) -> Vec<String> {
//!     let start = SamplyTimestamp::now();
//!
//!     // ... execute the database query.
//!     let results = vec![]; // Placeholder for actual results.
//!
//!     // Emit an interval marker from start to now.
//!     samply_marker!({
//!         name: format!("database query: {query}"),
//!         start_time: start,
//!     });
//!
//!     results
//! }
//! ```
//!
//! <br>
//!
//! ---
//!
//! <br>
//!
//! #### The [`samply_timer!`](crate::samply_timer) macro
//!
//! <br>
//!
//! While [`samply_marker!`](crate::samply_marker) requires manually providing a [`SamplyTimestamp`](marker::SamplyTimestamp)
//! for interval markers, the [`samply_timer!`](crate::samply_timer) macro simplifies this pattern by wrapping the timestamp in a scoped RAII object.
//! It creates a [`SamplyTimer`](marker::SamplyTimer) that registers the time it was created and automatically emits the interval marker when dropped at the end of its scope.
//!
//! ##### Automatic Interval
//!
//! The interval marker emits when the timer is dropped:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn expensive_computation() {
//!     let _timer = samply_timer!({ name: "expensive computation" });
//!
//!     // ... perform expensive work.
//!
//!     // The interval marker is automatically emitted here when _timer is dropped.
//! }
//! ```
//!
//! ##### Early Emit
//!
//! You can explicitly emit the interval marker before the end of the scope:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn process_with_cleanup() {
//!     let timer = samply_timer!({ name: "core processing" });
//!
//!     // ... perform core processing work.
//!
//!     // Emit the interval marker now, excluding cleanup from the measurement.
//!     timer.emit();
//!
//!     // ... perform cleanup tasks (not included in the interval marker).
//!
//!     // The interval marker will not emit a second time when dropped.
//! }
//! ```
//!
//! <br>
//!
//! ---
//!
//! <br>
//!
//! #### The [`samply_measure!`](crate::samply_measure) macro
//!
//! <br>
//!
//! Building on the scoped approach of [`samply_timer!`](crate::samply_timer), the [`samply_measure!`](crate::samply_measure) macro further simplifies profiling
//! by eliminating the need to create a scoped timer yourself. Instead, you wrap a code block, then its execution time is automatically measured with an interval marker.
//!
//! ##### Measure Synchronous Code
//!
//! The value of the measured block expression is preserved:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn compute_sum(values: &[i32]) -> i32 {
//!     samply_measure!({
//!         values.iter().sum()
//!     }, marker: {
//!         name: "compute sum",
//!     })
//! }
//!
//! let values = vec![1, 2, 3, 4, 5];
//! let result = compute_sum(&values);
//! assert_eq!(result, 15);
//! ```
//!
//! ##### With `?` Operator
//!
//! The block's control flow is preserved, including early returns:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! fn parse_and_validate(data: &str) -> Result<u32, String> {
//!     samply_measure!({
//!         let parsed = data.parse::<u32>()
//!             .map_err(|e| format!("parse error: {e}"))?;
//!
//!         if parsed > 100 {
//!             return Err(String::from("value too large"));
//!         }
//!
//!         Ok(parsed)
//!     }, marker: {
//!         name: "parse and validate",
//!     })
//! }
//! ```
//!
//! ##### Measure Asynchronous Code
//!
//! The macro works the same within asynchronous code. However, the clock does not stop between polls.
//! The resulting interval will span the total time to completion, including time spent waiting:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! # async fn fetch_data() -> String {
//! #     String::from("data")
//! # }
//! async fn process_data(data_id: u64) -> String {
//!     samply_measure!({
//!         let data = fetch_data().await;
//!         format!("Processed: {data} (id: {data_id})")
//!     }, marker: {
//!         name: format!("process data {data_id}"),
//!     })
//! }
//! ```
//!
//! ##### Create a New Async Block
//!
//! Use the `async` keyword to create a new async block, which allows the `?`
//! operator to return from the `async` block instead of from the enclosing function:
//!
//! ```rust
//! # use samply_markers::prelude::*;
//! # async fn read_file(path: &str) -> Option<String> { Some(String::from("100,200")) }
//! async fn load_config(path: &str) -> (u32, u32) {
//!     let config = samply_measure!(async {
//!         let contents = read_file(path).await?;
//!         let mut parts = contents.split(',');
//!
//!         let x = parts.next()?.parse::<u32>().ok()?;
//!         let y = parts.next()?.parse::<u32>().ok()?;
//!
//!         Some((x, y))
//!     }, marker: {
//!         name: "load config",
//!     })
//!     .await;
//!
//!     config.unwrap_or((0, 0))
//! }
//! ```
//!
//! <br>
//!
//! ## Example
//!
//! <br>
//!
//! Here's a complete example demonstrating everything in context:
//!
//! *See the [examples directory] to build, run, and profile the examples in this README yourself.*
//!
//! **[macOS](https://share.firefox.dev/3M2zMxq) |
//! [Ubuntu](https://share.firefox.dev/4r7bgeW) |
//! Windows (not yet supported)**
//!
//! ```rust,ignore
//! use samply_markers::prelude::*;
//!
//! async fn fetch_url(url: &str) -> (String, Option<String>) {
//!     // Emit an interval marker for the time it takes to fetch.
//!     let result = samply_measure!(async {
//!         let response = reqwest::get(url).await?;
//!         response.text().await
//!     }, marker: {
//!         name: format!("fetch {url}"),
//!     })
//!     .await;
//!
//!     match result {
//!         Ok(content) => {
//!             println!("  ✓ Fetched {url} ({} bytes)", content.len());
//!             (String::from(url), Some(content))
//!         }
//!         Err(error) => {
//!             // Emit an instant marker any time a fetch fails.
//!             samply_marker!({ name: format!("fetch failed: {url}") });
//!             println!("  ✗ Failed to fetch {url}: {error}");
//!             (String::from(url), None)
//!         }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a timer that will span the entirety of main().
//!     // The timer will emit an interval marker when it is dropped at the end of the scope.
//!     let _main_timer = samply_timer!({ name: "main()" });
//!
//!     println!("\nStarting samply-markers demo...\n");
//!
//!     std::thread::sleep(std::time::Duration::from_millis(200));
//!
//!     let urls = &[
//!         "https://example.com",
//!         "https://fail.invalid",
//!         "https://fail.invalid",
//!         "https://en.wikipedia.org/wiki/Firefox",
//!         "https://fail.invalid",
//!         "https://github.com/nordzilla",
//!     ];
//!
//!     println!("\n=== Sequential Fetching ===\n");
//!
//!     // Emit an interval marker for the total time to fetch all URLs sequentially.
//!     samply_measure!({
//!         for url in urls {
//!             fetch_url(url).await;
//!         }
//!     }, marker: {
//!         name: "fetch all URLs sequentially",
//!     });
//!
//!     std::thread::sleep(std::time::Duration::from_millis(200));
//!
//!     println!("\n=== Concurrent Fetching ===\n");
//!
//!     // Emit an interval marker for the total time to fetch all URLs concurrently.
//!     samply_measure!({
//!         futures::future::join_all(
//!             urls.iter().map(|url| fetch_url(url))
//!         ).await;
//!     }, marker: {
//!         name: "fetch all URLs concurrently",
//!     });
//!
//!     std::thread::sleep(std::time::Duration::from_millis(200));
//!
//!     println!("\nDemo completed!\n");
//! }
//! ```
//!
//! [samply]: https://github.com/mstange/samply?tab=readme-ov-file#samply
//! [examples directory]: https://github.com/mstange/samply/tree/main/samply-markers/examples

mod macros;
mod provider;

pub mod marker;

/// This module contains everything needed to add markers within a convenient glob import.
///
/// ```rust
/// use samply_markers::prelude::*;
/// ```
pub mod prelude {
    pub use crate::marker::SamplyMarker;
    pub use crate::marker::SamplyTimer;
    pub use crate::marker::SamplyTimestamp;

    #[doc(inline)]
    pub use crate::samply_marker;
    #[doc(inline)]
    pub use crate::samply_measure;
    #[doc(inline)]
    pub use crate::samply_timer;
}
