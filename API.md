# Symbolication API

The `query_api` function in this project implements a server-like API: The input is a "path" string and a "request" JSON string, and the output is a "response" JSON string.

```rust
pub async fn query_api<'h>(
    request_url: &str,
    request_json_data: &str,
    helper: &'h impl FileAndPathHelper<'h>,
) -> String { ... }
```

This implementation is currently used in two projects:

 1. In [`profiler-symbol-server`](https://github.com/mstange/profiler-symbol-server/), which runs a local web server.
 2. In Firefox, in profiler support code ([`symbolication.jsm.js`](https://searchfox.org/mozilla-central/source/devtools/client/performance-new/symbolication.jsm.js)).

The user of these APIs is the [Firefox Profiler](https://github.com/firefox-devtools/profiler), a profiling UI written in HTML / CSS / JS. It accesses the server API via `fetch`, and the Firefox API via WebChannel messages. The fact that both of these methods use the same format is very convenient.

Furthermore, there is another implementation of the same API in [Tecken](https://github.com/mozilla-services/tecken), hosted at [symbolication.services.mozilla.com](https://symbolication.services.mozilla.com/). This is where the `/symbolicate/v5` API started.
The Firefox Profiler accesses Tecken via `fetch`.

## Supported APIs

`profiler-get-symbols` currently supports two "paths", or API entry points:

 - `/symbolicate/v5`: Symbolicate addresses to function names, file names and line numbers. The API matches [the Tecken API](https://tecken.readthedocs.io/en/latest/symbolication.html).
 - `/source/v1`: Request source code for a file. Not supported in Tecken.

### `/symbolicate/v5`

Example request JSON:

```json
{
  "jobs": [
    {
      "memoryMap": [
        ["libc.so.6", "627B03B886604653802321DD2256B8AD0"]
      ],
      "stacks": [
        [[0, 1716093], [0, 1186125], [0, 1162463], [0, 639041]]
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
          }
        ]
      ],
      "found_modules": {
        "libc.so.6/627B03B886604653802321DD2256B8AD0": true
      }
    }
  ]
}
```

Not shown here: Every frame can have an `inlines` property. This is currently supported in `profiler-get-symbols` but not in Tecken.

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

## Special paths

[To be written]
