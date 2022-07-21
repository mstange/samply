# samply-api

This crate implements a JSON API for profiler symbolication with the help of local symbol files. It exposes a single function `query_api`, and uses the `samply-symbols` crate for its implementation.

The API is documented in [API.md](../API.md).

Just like the `samply-symbols` crate, this crate does not contain any direct file access. It is written in such a way that it can be compiled to WebAssembly, with all file access being mediated via a `FileAndPathHelper` trait.
