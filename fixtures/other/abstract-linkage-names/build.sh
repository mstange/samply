#!/bin/bash

# Builds the fixtures for the `abstract_linkage_names` integration tests.
#
# Run this from this directory. The absolute paths of the outputs get baked into
# the linked binaries (as OSO entries in the debug map, and as DW_AT_comp_dir),
# and the tests redirect those paths back to the fixture directory, so the paths
# in the test expectations only match if this was run from a checkout at
# /Users/mstange/code/samply.

set -euo pipefail

# The flags Firefox uses. -dwarf-linkage-names=Abstract is the interesting one:
# it drops DW_AT_linkage_name from concrete subprogram DIEs.
DEBUG_FLAGS=(
    -g
    -gdwarf-4
    -gsimple-template-names
    -mllvm=-dwarf-linkage-names=Abstract
)

# Variant 1: debug info left behind in the .o file, reached through the debug map
# ("OSO" stabs) in the linked binary. This is what a local Firefox build looks
# like on macOS.
mkdir -p out/mac-oso
clang++ -c -Os -Wall -Wextra "${DEBUG_FLAGS[@]}" src/main.cpp -o out/mac-oso/main.o
clang++ out/mac-oso/main.o -o out/mac-oso/main

# Variant 2: debug info copied into a .dSYM bundle by dsymutil.
mkdir -p out/mac-dsym
clang++ -c -Os -Wall -Wextra "${DEBUG_FLAGS[@]}" src/main.cpp -o out/mac-dsym/main.o
clang++ out/mac-dsym/main.o -o out/mac-dsym/main
dsymutil out/mac-dsym/main
rm out/mac-dsym/main.o
