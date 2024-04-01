#include <iostream>

#include "file2.h"

int file2_func1(int x) {
  std::cout << "Hello from file2.cpp file2_func1().\n";
  return file2_func2(x + 2);
}

int file2_func2(int y) {
  return file2_func3(y, 4) + 5;
}
