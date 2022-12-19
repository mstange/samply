https://share.firefox.dev/3WoTELe

Captured on Ubuntu 22.04.1 LTS (GNU/Linux 5.15.0-56-generic aarch64)

The raw, unsymbolicated profile is in `ls-profile.json`.

Debug symbols installed locally using `sudo apt install coreutils-dbgsym`. (8.32-4.1ubuntu1)

`/usr/bin/ls` -> `./ls`
`/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug` -> `./260a3e6e46db57abf718f6a3562c6eedccf269.debug`
`/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug` -> `./coreutils.debug`

With debuginfod, the symbols for `ls` can be obtained even on non-Linux systems.
For example, the following command gives a profile with symbols for `ls` when run on macOS:
`DEBUGINFOD_URLS="https://debuginfod.ubuntu.com" samply load ls-profile.json`

This is a case where two symbol files need to be downloaded:

 1. The debug file for `ls` itself, and
 2. The "supplementary" `coreutils.debug` file.

The Ubuntu debuginfod server has both files. Both files are found via their buildid; the first file's buildid is written down in the profile JSON (as `codeId`), and the second file's buildid is in the first file's `.gnu_debugaltlink` section.

To check that the supplementary file was found, look for the "gobble_file" function and its inlined "do_lstat" callee.
The function name strings for those functions come from the supplementary file.

Unfortunately, it seems that the kernel symbols are not on the ubuntu debuginfod server; `https://debuginfod.ubuntu.com/buildid/984b766f1cb5699c3b1b77b592983c22e9d197ad/debuginfo` does not exist. And I wasn't able to install the kernel symbols locally; I couldn't get `sudo apt-get install linux-image-$(uname -r)-dbgsym` to work. (`E: Unable to locate package linux-image-5.15.0-56-generic-dbgsym`)
