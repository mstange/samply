#!/bin/bash

mkdir -p out/mac-dsym

# Compile source files into object files
clang++ -c -g -Os -Wall -Wextra src/main.cpp -o out/mac-dsym/main.o
clang++ -c -g -Os -Wall -Wextra src/file1.cpp -o out/mac-dsym/file1.o
clang++ -c -g -Os -Wall -Wextra src/file2.cpp -o out/mac-dsym/file2.o
clang++ -c -g -Os -Wall -Wextra src/file3.cpp -o out/mac-dsym/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/mac-dsym/libfile23.a out/mac-dsym/file2.o out/mac-dsym/file3.o

# Link libraries into executable
clang++ out/mac-dsym/main.o out/mac-dsym/file1.o out/mac-dsym/libfile23.a -o out/mac-dsym/main

# Link debuginfo into dSYM
dsymutil out/mac-dsym/main

# Remove .o, .a files
rm out/mac-dsym/*.o out/mac-dsym/*.a

