# mach_ipc

This directory has some code that allows establishing two-way mach communication between
a parent process and a child process.

This code was originally written by pcwalton for ipc-channel. I needed some extra
functionality to be able to send raw ports, namely `mach_task_self()`, so I forked
the code.

The `samply-mac-preload` directory has another copied implentation of this with large
pieces of functionality removed, to reduce code size.
