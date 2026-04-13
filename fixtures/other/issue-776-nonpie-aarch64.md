Small non-PIE ELF fixture for issue #776.

It reproduces the case where `LookupAddress::Relative(0x4580)` used to resolve
to `fun_4580` instead of the real symbol `func_74`.

Generation command:

```sh
cc -O0 -g -fno-pie -no-pie -fasynchronous-unwind-tables \
  -Wl,-z,max-page-size=0x1000 -Wl,-Ttext-segment=0x4000 \
  -o issue-776-nonpie-aarch64 issue-776-nonpie-aarch64.c
```
