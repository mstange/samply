# Symbolication of `ls` on Ubuntu

The files in this directory are used to test symbolication of `ls`, which requires both its regular debug file as well as a supplementary debug file (`coreutils.debug`). The supplementary file is referenced by path and build ID in the `gnu_debugaltlink` section of the binary and also of the regular debug file.

Here's a profile, captured with `perf` on Ubuntu 22.04.1 LTS (GNU/Linux 5.15.0-56-generic aarch64): https://share.firefox.dev/3WoTELe

The raw, unsymbolicated profile is in `ls-profile.json`.

I installed debug symbols on the Ubuntu machine using `sudo apt install coreutils-dbgsym`. (8.32-4.1ubuntu1, aarch64)

Then I copied the relevant files into this directory:

`/usr/bin/ls` -> `./ls`

`/usr/lib/debug/.build-id/63/260a3e6e46db57abf718f6a3562c6eedccf269.debug` -> `./260a3e6e46db57abf718f6a3562c6eedccf269.debug`

`/usr/lib/debug/.dwz/aarch64-linux-gnu/coreutils.debug` -> `./coreutils.debug`

These files allow successful symbolication of `ls`.

## Testing with `debuginfod`

With the help of debuginfod, the profile captured above can be symbolicated even if the original symbol files aren't locally available, because the files can be pulled from Ubuntu's `debuginfod` server.

For example, the following command gives a profile with symbols for `ls` when run on macOS:
`DEBUGINFOD_URLS="https://debuginfod.ubuntu.com" samply load ls-profile.json`

This is a case where two symbol files need to be downloaded:

 1. The debug file for `ls` itself, and
 2. The "supplementary" `coreutils.debug` file.

The Ubuntu debuginfod server has both files. Both files are found via their buildid; the first file's buildid is written down in the profile JSON (as `codeId`), and the second file's buildid is in the first file's `.gnu_debugaltlink` section.

To check that the supplementary file was found, look for the "gobble_file" function and its inlined "do_lstat" callee.
The function name strings for those functions come from the supplementary file.

### Why no kernel symbols in the profile?

Unfortunately, it seems that the kernel symbols are not on the ubuntu debuginfod server; `https://debuginfod.ubuntu.com/buildid/984b766f1cb5699c3b1b77b592983c22e9d197ad/debuginfo` does not exist. And I wasn't able to install the kernel symbols locally; I couldn't get `sudo apt-get install linux-image-$(uname -r)-dbgsym` to work. (`E: Unable to locate package linux-image-5.15.0-56-generic-dbgsym`)
