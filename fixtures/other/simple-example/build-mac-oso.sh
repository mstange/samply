#!/bin/bash

mkdir -p out/mac-oso

# Compile source files into object files
clang++ -c -g -Os -Wall -Wextra src/main.cpp -o out/mac-oso/main.o
clang++ -c -g -Os -Wall -Wextra src/file1.cpp -o out/mac-oso/file1.o
clang++ -c -g -Os -Wall -Wextra src/file2.cpp -o out/mac-oso/file2.o
clang++ -c -g -Os -Wall -Wextra src/file3.cpp -o out/mac-oso/file3.o

# Create libfile23.a from file2.o and file3.o
ar rcs out/mac-oso/libfile23.a out/mac-oso/file2.o out/mac-oso/file3.o

# Link libraries into executable
clang++ out/mac-oso/main.o out/mac-oso/file1.o out/mac-oso/libfile23.a -o out/mac-oso/main
