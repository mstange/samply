#include <iostream>

#include "file1.h"

int main() {
  std::cout << "Hello from main()\n";
  int r = file1_func1(15);
  std::cout << "Number: " << r << "\n";
  return 0;
}
