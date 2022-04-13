# linux-perf-data

This repo contains a parser for the perf.data format which is output by the Linux `perf` tool.

It also contains a `main.rs` which acts similarly to `perf script` and does symbolication, but with the advantage that it is much much faster than `perf script`.

The end goal of this project is to create a fast drop-in replacement for `perf script`, implementing just a basic subset of functionality, but having super fast symbolication. But that replacement will move to a separate repo. This repo should just contain a library crate for parsing tha data.

## Acknowledgements

Some of the code in this repo was based on [**@koute**'s `not-perf` project](https://github.com/koute/not-perf/tree/20e4ddc2bf8895d96664ab839a64c36f416023c8/perf_event_open/src).

## Run

```
% cargo run --release -- perf.data
Hostname: ubuildu
OS release: 5.13.0-35-generic
Perf version: 5.13.19
Arch: x86_64
CPUs: 16 online (16 available)
Comm: {"pid": 212227, "tid": 212227, "name": "perf-exec"}
Comm: {"pid": 212227, "tid": 212227, "name": "dump_syms"}
file "/etc/ld.so.cache" had unrecognized format
Have 19833 events, converted into 15830 processed samples.
0x000055ba9eb4d000-0x000055ba9f07e000 "/home/mstange/code/dump_syms/target/release/dump_syms"
0x00007f76b8720000-0x00007f76b8749000 "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"
0x00007f76b8711000-0x00007f76b8720000 "/etc/ld.so.cache"
0x00007f76b84f6000-0x00007f76b8692000 "/usr/lib/x86_64-linux-gnu/libstdc++.so.6.0.29"
0x00007f76b845e000-0x00007f76b84d0000 "/usr/lib/x86_64-linux-gnu/libssl.so.1.1"
0x00007f76b8183000-0x00007f76b839e000 "/usr/lib/x86_64-linux-gnu/libcrypto.so.1.1"
0x00007f76b8169000-0x00007f76b817e000 "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1"
0x00007f76b8085000-0x00007f76b810c000 "/usr/lib/x86_64-linux-gnu/libm.so.6"
0x00007f76b7e5d000-0x00007f76b8019000 "/usr/lib/x86_64-linux-gnu/libc.so.6"
0x000055ba9f22b000-0x000055ba9f2a8000 "/home/mstange/code/dump_syms/target/release/dump_syms"
0x00007f76b8753000-0x00007f76b8755000 "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"
Sample at t=1223104025585523 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104025590131 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104025592387 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104025594614 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104025596893 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104025616225 pid=542572 tid=542572
  0x7f337451033b

Sample at t=1223104026572279 pid=542572 tid=542572
  fun_24430
  fun_dca0
  fun_25c0
  fun_1f4b0
  fun_1e30
  0x10d7 (in "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2")

Sample at t=1223104033727818 pid=542572 tid=542572
  read
  std::sys::unix::fd::FileDesc::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys/unix/fd.rs:120)
  std::sys::unix::fs::File::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys/unix/fs.rs:870)
  <std::fs::File as std::io::Read>::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/fs.rs:627)
  std::io::default_read_to_end (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/io/mod.rs:378)
  <std::fs::File as std::io::Read>::read_to_end (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/fs.rs:638)
  dump_syms::utils::read_file (/home/mstange/code/dump_syms/src/utils.rs:29)
  dump_syms::dumper::get_from_id (/home/mstange/code/dump_syms/src/dumper.rs:195)
  dump_syms::dumper::single_file (/home/mstange/code/dump_syms/src/dumper.rs:202)
  dump_syms::main (/home/mstange/code/dump_syms/src/main.rs:248)
  core::ops::function::FnOnce::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:227)
  std::sys_common::backtrace::__rust_begin_short_backtrace (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys_common/backtrace.rs:123)
  std::rt::lang_start::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:145)
  core::ops::function::impls::<impl core::ops::function::FnOnce<A> for &F>::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:259)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  main
  fun_29f50
  __libc_start_main
  _start
  0x7ffdb4824837

[...]

Sample at t=1223108807419100 pid=542572 tid=542572
  fun_a1b30
  fun_a3290
  fun_a4360
  realloc
  alloc::alloc::realloc (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:124)
  alloc::alloc::Global::grow_impl (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:201)
  <alloc::alloc::Global as core::alloc::Allocator>::grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:254)
  alloc::raw_vec::finish_grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:466)
  alloc::raw_vec::RawVec<T,A>::grow_amortized (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:402)
  alloc::raw_vec::RawVec<T,A>::reserve_for_push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:300)
  alloc::vec::Vec<T,A>::push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/vec/mod.rs:1726)
  cpp_demangle::ast::one_or_more (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:7700)
  <cpp_demangle::ast::BareFunctionType as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:4285)
  <cpp_demangle::ast::Encoding as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:1435)
  <cpp_demangle::ast::MangledName as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:1341)
  cpp_demangle::Symbol<T>::new_with_options (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/lib.rs:238)
  symbolic_demangle::try_demangle_cpp (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-demangle-8.3.0/src/lib.rs:212)
  <symbolic_common::types::Name as symbolic_demangle::Demangle>::demangle (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-demangle-8.3.0/src/lib.rs:410)
  dump_syms::linux::elf::Collector::demangle (/home/mstange/code/dump_syms/src/linux/elf.rs:217)
  dump_syms::linux::elf::Collector::collect_function (/home/mstange/code/dump_syms/src/linux/elf.rs:281)
  dump_syms::linux::elf::Collector::collect_functions (/home/mstange/code/dump_syms/src/linux/elf.rs:308)
  dump_syms::linux::elf::ElfInfo::from_object (/home/mstange/code/dump_syms/src/linux/elf.rs:388)
  dump_syms::linux::elf::ElfInfo::new (/home/mstange/code/dump_syms/src/linux/elf.rs:368)
  <dump_syms::linux::elf::ElfInfo as dump_syms::dumper::Creator>::get_dbg (/home/mstange/code/dump_syms/src/dumper.rs:71)
  dump_syms::dumper::single_file (/home/mstange/code/dump_syms/src/dumper.rs:217)
  dump_syms::main (/home/mstange/code/dump_syms/src/main.rs:248)
  core::ops::function::FnOnce::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:227)
  std::sys_common::backtrace::__rust_begin_short_backtrace (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys_common/backtrace.rs:123)
  std::rt::lang_start::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:145)
  core::ops::function::impls::<impl core::ops::function::FnOnce<A> for &F>::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:259)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  main
  fun_29f50
  __libc_start_main
  _start
  0x7ffdb4824837

[...]

Sample at t=1223117554603746 pid=542572 tid=542572
  fun_a3290
  fun_a4360
  realloc
  alloc::alloc::realloc (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:124)
  alloc::alloc::Global::grow_impl (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:201)
  <alloc::alloc::Global as core::alloc::Allocator>::grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:254)
  alloc::raw_vec::finish_grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:466)
  alloc::raw_vec::RawVec<T,A>::grow_amortized (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:402)
  alloc::raw_vec::RawVec<T,A>::reserve_for_push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:300)
  alloc::vec::Vec<T,A>::push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/vec/mod.rs:1726)
  symbolic_minidump::cfi::AsciiCfiWriter<W>::process_fde (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-minidump-8.3.0/src/cfi.rs:613)
  symbolic_minidump::cfi::AsciiCfiWriter<W>::read_cfi (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-minidump-8.3.0/src/cfi.rs:580)
  <truncated stack>

[...]

Sample at t=1223121940171375 pid=542572 tid=542572
  _Exit
```
