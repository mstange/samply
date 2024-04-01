#!/bin/bash

mkdir -p out/dwp-debuglink

# Compile source files into object files
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/main.cpp -o out/dwp-debuglink/main.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file1.cpp -o out/dwp-debuglink/file1.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file2.cpp -o out/dwp-debuglink/file2.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file3.cpp -o out/dwp-debuglink/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/dwp-debuglink/libfile23.a out/dwp-debuglink/file2.o out/dwp-debuglink/file3.o

# Link libraries into executable
g++ out/dwp-debuglink/main.o out/dwp-debuglink/file1.o out/dwp-debuglink/libfile23.a -o out/dwp-debuglink/main

# Make dwp file
dwp -e out/dwp-debuglink/main

# Extract dwp sections into individual files
objcopy --dump-section .debug_abbrev.dwo=out/dwp-debuglink/dwp-debug_abbrev.dwo.bin \
        --dump-section .debug_line.dwo=out/dwp-debuglink/dwp-debug_line.dwo.bin \
        --dump-section .debug_loc.dwo=out/dwp-debuglink/dwp-debug_loc.dwo.bin \
        --dump-section .debug_str_offsets.dwo=out/dwp-debuglink/dwp-debug_str_offsets.dwo.bin \
        --dump-section .debug_info.dwo=out/dwp-debuglink/dwp-debug_info.dwo.bin \
        --dump-section .debug_str.dwo=out/dwp-debuglink/dwp-debug_str.dwo.bin \
        --dump-section .debug_cu_index=out/dwp-debuglink/dwp-debug_cu_index.bin \
        --dump-section .debug_tu_index=out/dwp-debuglink/dwp-debug_tu_index.bin \
        out/dwp-debuglink/main.dwp

# Create main.dbg with debug sections from main and from dwp
objcopy --only-keep-debug \
        --add-section .debug_abbrev.dwo=out/dwp-debuglink/dwp-debug_abbrev.dwo.bin \
        --add-section .debug_line.dwo=out/dwp-debuglink/dwp-debug_line.dwo.bin \
        --add-section .debug_loc.dwo=out/dwp-debuglink/dwp-debug_loc.dwo.bin \
        --add-section .debug_str_offsets.dwo=out/dwp-debuglink/dwp-debug_str_offsets.dwo.bin \
        --add-section .debug_info.dwo=out/dwp-debuglink/dwp-debug_info.dwo.bin \
        --add-section .debug_str.dwo=out/dwp-debuglink/dwp-debug_str.dwo.bin \
        --add-section .debug_cu_index=out/dwp-debuglink/dwp-debug_cu_index.bin \
        --add-section .debug_tu_index=out/dwp-debuglink/dwp-debug_tu_index.bin \
        out/dwp-debuglink/main out/dwp-debuglink/main.dbg
strip -g out/dwp-debuglink/main
objcopy --add-gnu-debuglink=out/dwp-debuglink/main.dbg out/dwp-debuglink/main

# Remove .dwo, .dwp, .bin, .o, .a files
rm out/dwp-debuglink/*.dwo out/dwp-debuglink/*.dwp out/dwp-debuglink/*.bin out/dwp-debuglink/*.o out/dwp-debuglink/*.a
