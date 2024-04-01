#!/bin/bash

mkdir -p out/with-dwp

# Compile source files into object files
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/main.cpp -o out/with-dwp/main.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file1.cpp -o out/with-dwp/file1.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file2.cpp -o out/with-dwp/file2.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file3.cpp -o out/with-dwp/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/with-dwp/libfile23.a out/with-dwp/file2.o out/with-dwp/file3.o

# Link libraries into executable
g++ out/with-dwp/main.o out/with-dwp/file1.o out/with-dwp/libfile23.a -o out/with-dwp/main

# Make dwp file
dwp -e out/with-dwp/main

# Remove .dwo, .o, .a files
rm out/with-dwp/*.dwo out/with-dwp/*.o out/with-dwp/*.a
