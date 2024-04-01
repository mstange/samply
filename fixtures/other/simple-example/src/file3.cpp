#include <iostream>

#include "file3.h"

int file3_func1(int x) {
  std::cout << "Hello from file3.cpp file3_func1().\n";
  return file3_func2(x + 2);
}

int file3_func2(int y) {
  return file3_func3(y, 4) + 5;
}
