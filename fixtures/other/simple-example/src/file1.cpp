#include <iostream>

#include "file1.h"
#include "file2.h"
#include "file3.h"

int file1_func1(int x) {
  std::cout << "Hello from file1.cpp file1_func1().\n";
  return (file1_func2(x + 2) + file2_func1(x + 4)) << 2;
}

int file1_func2(int y) {
  return file1_func3(y, file3_func1(y + 3)) + 5;
}
