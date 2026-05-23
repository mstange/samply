# samply-api

This crate implements a JSON API for profiler symbolication with the help of
local symbol files. It exposes a single type called `Api`, and uses the
`samply-symbols` crate for its implementation.

The API is documented in [API.md](../API.md).

Just like the `samply-symbols` crate, this crate does not contain any direct
file access. It is written in such a way that it can be compiled to
WebAssembly. The state machines exposed here are generic over a `FileTypes`
type bundle, and a driver in the consumer's environment performs the actual
file I/O.

Do not use this crate directly unless you have to. Instead, use
[`wholesym`](https://docs.rs/wholesym), which provides a much more ergonomic Rust API.
`wholesym` exposes the JSON API functionality via [`SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).

## Example

`samply-api` itself is sans-IO: `Api::build_query` returns a state machine
which surfaces "I need this file / symbol map / binary" requests as values
via `ApiQueryState::poll`. A driver — typically `wholesym` — fetches what's
requested and feeds the result back in. For an end-to-end usage example, see
[`wholesym::SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).
