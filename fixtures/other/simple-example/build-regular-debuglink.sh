#!/bin/bash

mkdir -p out/regular-debuglink

# Compile source files into object files
g++ -c -g -Os -Wall -Wextra src/main.cpp -o out/regular-debuglink/main.o
g++ -c -g -Os -Wall -Wextra src/file1.cpp -o out/regular-debuglink/file1.o
g++ -c -g -Os -Wall -Wextra src/file2.cpp -o out/regular-debuglink/file2.o
g++ -c -g -Os -Wall -Wextra src/file3.cpp -o out/regular-debuglink/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/regular-debuglink/libfile23.a out/regular-debuglink/file2.o out/regular-debuglink/file3.o

# Link libraries into executable
g++ out/regular-debuglink/main.o out/regular-debuglink/file1.o out/regular-debuglink/libfile23.a -o out/regular-debuglink/main

# Create main.dbg with debug sections 
objcopy --only-keep-debug out/regular-debuglink/main out/regular-debuglink/main.dbg
strip -g out/regular-debuglink/main
objcopy --add-gnu-debuglink=out/regular-debuglink/main.dbg out/regular-debuglink/main

# Remove .o, .a files
rm out/regular-debuglink/*.o out/regular-debuglink/*.a
