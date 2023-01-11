# Symbolication API

The `query_json_api` function in this project implements a server-like API: The input is a "path" string and a "request" JSON string, and the output is a "response" JSON string.

```rust
pub async fn query_json_api(
    path: &str,
    request_json: &str,
) -> String { ... }
```

Examples:

 - `query_json_api("/symbolicate/v5", "{...}")` returns `"{...}"`
 - `query_json_api("/source/v1", "{...}")` returns `"{...}"`
 - `query_json_api("/asm/v1", "{...}")` returns `"{...}"`

The implementation for this API lives in the `samply-api` crate. This crate is currently used in the following projects:

 1. [`wholesym`](https://docs.rs/wholesym/) uses it and exposes the API through [`SymbolManager::query_json_api`](https://docs.rs/wholesym/latest/wholesym/struct.SymbolManager.html#method.query_json_api).
 2. `samply` uses it (via `wholesym`) when opening a profile: It runs a local web server which exposes it as a web API, for example at `http://127.0.0.1:3000/abcdefghijkl/symbolicate/v5`.
 2. Firefox uses it in profiler support code ([`symbolication.jsm.js`](https://searchfox.org/mozilla-central/source/devtools/client/performance-new/symbolication.jsm.js)), via the [`profiler-get-symbols` WebAssembly module](https://github.com/mstange/profiler-get-symbols).

The consumer of these APIs is the [Firefox Profiler](https://github.com/firefox-devtools/profiler), a profiling UI written in HTML / CSS / JS. It accesses the web API via `fetch`, and the Firefox API via `WebChannel` messages. Since the same API is used for both, a lot of code can be shared.

Furthermore, there is another implementation of the same API in [Tecken](https://github.com/mozilla-services/tecken), hosted at [symbolication.services.mozilla.com](https://symbolication.services.mozilla.com/). Tecken was the original implementer of the `/symbolicate/v5` API.
The Firefox Profiler accesses Tecken via `fetch`.

## Supported APIs

`samply-api` currently supports three "paths", or API entry points:

 - `/symbolicate/v5`: Symbolicate addresses to function names, file names and line numbers. The API matches [the Tecken API](https://tecken.readthedocs.io/en/latest/symbolication.html).
 - `/source/v1`: Request source code for a file. Not supported in Tecken.
 - `/asm/v1`: Request assembly code for parts of a binary. Not supported in Tecken.

### `/symbolicate/v5`

Example request JSON:

```json
{
  "jobs": [
    {
      "memoryMap": [
        ["libc.so.6", "627B03B886604653802321DD2256B8AD0"],
        ["combase.pdb", "071849A7C75FD246A3367704EE1CA85B1"]
      ],
      "stacks": [
        [[0, 1716093], [0, 1186125], [0, 1162463], [0, 639041],
         [1, 677976]]
      ]
    }
  ]
}
```

Example response JSON:

```json
{
  "results": [
    {
      "stacks": [
        [
          {
            "frame": 0,
            "module": "libc.so.6",
            "module_offset": "0x1a2f7d",
            "function": "__memcpy_avx_unaligned_erms",
            "function_offset": "0xbd"
          },
          {
            "frame": 1,
            "module": "libc.so.6",
            "module_offset": "0x12194d",
            "function": "syscall",
            "function_offset": "0x1d"
          },
          {
            "frame": 2,
            "module": "libc.so.6",
            "module_offset": "0x11bcdf",
            "function": "__poll",
            "function_offset": "0x4f",
            "file": "sysdeps/unix/sysv/linux/poll.c",
            "line": 29
          },
          {
            "frame": 3,
            "module": "libc.so.6",
            "module_offset": "0x9c041",
            "function": "__GI___pthread_mutex_lock",
            "function_offset": "0x291",
            "file": "nptl/nptl/pthread_mutex_lock.c",
            "line": 141
          },
          {
            "frame": 4,
            "module_offset": "0xa5858",
            "module": "combase.pdb",
            "function": "CRIFTable::AddEntry(_GUID const&, _GUID const&, unsigned long, CRIFTable::tagRIFEntry**, bool, UniversalMarshalerType, ObjectLibrary::OpaqueString)",
            "function_offset": "0x28",
            "function_size": "0x131",
            "file": "onecore\\com\\combase\\dcomrem\\riftbl.cxx",
            "line": 2081,
            "inlines": [
              {
                "function": "operator&(WINDOWS_RUNTIME_HSTRING_FLAGS, WINDOWS_RUNTIME_HSTRING_FLAGS)",
                "file": "onecore\\com\\combase\\winrt\\string\\HstringHeaderInternal.h",
                "line": 40
              },
              {
                "function": "CHSTRINGUtil::IsStringReference() const",
                "file": "onecore\\com\\combase\\winrt\\string\\StringUtil.inl",
                "line": 135
              },
              {
                "function": "CHSTRINGUtil::Release()",
                "file": "onecore\\com\\combase\\winrt\\string\\StringUtil.inl",
                "line": 35
              },
              {
                "function": "WindowsDeleteString(HSTRING__*)",
                "file": "onecore\\com\\combase\\winrt\\string\\string.cpp"
              },
              {
                "function": "PrivMemAlloc(unsigned long long)",
                "file": "onecore\\Com\\combase\\ih\\memapi.hxx",
                "line": 72
              },
              {
                "function": "Microsoft::WRL::Wrappers::HString::{dtor}()",
                "file": "onecore\\external\\sdk\\inc\\wrl\\wrappers\\corewrappers.h"
              }
            ]
          }
        ]
      ],
      "found_modules": {
        "combase.pdb/071849A7C75FD246A3367704EE1CA85B1": true,
        "libc.so.6/627B03B886604653802321DD2256B8AD0": true
      }
    }
  ]
}
```

### `/source/v1`

Example request JSON:

```json
{
  "debugName": "XUL",
  "debugId": "2DC09FF43A4231FC9C34BB3CFE464B2C0",
  "moduleOffset": "0x4b8fb3f",
  "file": "/Users/mstange/code/mozilla/gfx/wr/webrender/src/renderer/upload.rs"
}
```

Example response JSON:

```json
{
  "symbolsLastModified": null,
  "sourceLastModified": null,
  "file": "/Users/mstange/code/mozilla/gfx/wr/webrender/src/renderer/upload.rs",
  "source": "/* This Source Code Form is subject to the terms of the Mozilla Public\n * License, v. 2.0. If a copy of the MPL was not distributed with this\n * file, You can obtain one at http://mozilla.org/MPL/2.0/. */\n\n//! This module contains the convoluted logic that goes into uploading content into\n//! the texture cache's textures [...]"
}
```

This does the following:

 1. It looks up symbols for the address `0x4b8fb3f` in the XUL library, the same way as it would in the `/symbolicate/v5` entry point.
 2. It checks the filenames for the frames which `0x4b8fb3f` symbolicates to.
 3. If the requested filename is found, it reads the file and returns it.

This way, the API can only be used to access files which are referred to from the debug data of the symbol information, and not arbitrary files.

Furthermore, there are two placeholder properties for last-modified timestamps. These are still null as of now, see [issue #26](https://github.com/mstange/profiler-get-symbols/issues/26) for updates.

### `/asm/v1`

Example request JSON:

```json
{
  "name": "libcorecrypto.dylib",
  "codeId": "6A5FFEB0E606324EB687DA95C362CE05",
  "startAddress": "0x5844",
  "size": "0x1c"
}
```

Example response JSON:

```json
{
  "startAddress": "0x5844",
  "size": "0x1c",
  "instructions": [
    [0, "hint #0x1b"],
    [4, "stp x29, x30, [sp, #-0x10]!"],
    [8, "mov x29, sp"],
    [12, "adrp x0, $+0x593f3000"],
    [16, "add x0, x0, #0x340"],
    [20, "ldr x8, [x0]"],
    [24, "blraaz x8"]
  ],
}
```

This finds the requested binary, reads the machine code bytes for the requested range, and disassembles them based on the binary's target architecture. The per-instruction offset is relative to the given `startAddress`.

## Special paths

The `/symbolicate/v5` API returns file paths in the `file` property of its response JSON. Such a file path can either be a regular path string (e.g. `/Users/mstange/code/mozilla/widget/cocoa/nsAppShell.mm`), or it can also a "special path", e.g. `hg:hg.mozilla.org/mozilla-central:mozglue/baseprofiler/core/ProfilerBacktrace.cpp:1706d4d54ec68fae1280305b70a02cb24c16ff68`.

The following special path formats are supported:

 - `hg:<repo>:<path>:<rev>`: Path in a mercurial repository.
 - `git:<repo>:<path>:<rev>`: Path in a git repository.
 - `s3:<bucket>:<digest>/<path>:`: Path in an AWS S3 bucket.
 - `cargo:<registry>:<crate_name>-<version>:<path>`: Path in a Rust package.

These special paths can be parsed and produced with the help of the [`MappedPath` type](https://docs.rs/samply-symbols/0.20.0/samply_symbols/enum.MappedPath.html).