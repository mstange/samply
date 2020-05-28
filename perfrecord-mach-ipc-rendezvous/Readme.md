# perfrecord-mach-ipc-rendzvous

This crate has some code that allows establishing two-way mach communication between
a parent process and a child process.

This code was originally written by pcwalton for ipc-channel. I needed some extra
functionality to be able to send raw ports, namely `mach_task_self()`, so I forked
the code. I may also remove large pieces of functionality that I don't need, so
that the size of the perfrecord-preload library gets reduced.

This is a separate crate, rather than just a mod inside perfrecord, so that it can
also be used by perfrecord-preload.
