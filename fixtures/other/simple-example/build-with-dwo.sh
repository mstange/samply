#!/bin/bash

mkdir -p out/with-dwo

# Compile source files into object files
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/main.cpp -o out/with-dwo/main.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file1.cpp -o out/with-dwo/file1.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file2.cpp -o out/with-dwo/file2.o
g++ -c -g -Os -Wall -Wextra -gsplit-dwarf src/file3.cpp -o out/with-dwo/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/with-dwo/libfile23.a out/with-dwo/file2.o out/with-dwo/file3.o

# Link libraries into executable
g++ out/with-dwo/main.o out/with-dwo/file1.o out/with-dwo/libfile23.a -o out/with-dwo/main
