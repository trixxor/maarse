#pragma once
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#define DEC 10
#define HEX 16
#define OCT 8
#define BIN 2

class Print {
public:
  virtual ~Print() = default;
  virtual size_t write(uint8_t b) = 0;
  virtual size_t write(const uint8_t* buffer, size_t size) {
    size_t t = 0;
    for (size_t i = 0; i < size; i++) t += write(buffer[i]);
    return t;
  }
  size_t write(const char* str) { return str ? write((const uint8_t*)str, strlen(str)) : 0; }
  size_t write(const char* buffer, size_t size) { return write((const uint8_t*)buffer, size); }

  virtual size_t print(const char* s) { return write(s); }
  size_t print(char c) { return write((uint8_t)c); }
  virtual size_t print(int v, int r = DEC) { return 0; }
  virtual size_t print(unsigned int v, int r = DEC) { return 0; }
  virtual size_t print(long v, int r = DEC) { return 0; }
  virtual size_t print(unsigned long v, int r = DEC) { return 0; }
  virtual size_t print(double v, int p = 2) { return 0; }
  virtual size_t println() { return write((const uint8_t*)"\n", 1); }
  virtual size_t println(const char* s) { return print(s) + println(); }
  virtual void flush() {}
};

class Stream : public Print {
public:
  virtual int available() { return 0; }
  virtual int read() { return -1; }
  virtual int peek() { return -1; }
  size_t readBytes(uint8_t* buffer, size_t length) {
    size_t i = 0;
    while (i < length && available()) buffer[i++] = (uint8_t)read();
    return i;
  }
  size_t readBytes(char* buffer, size_t length) { return readBytes((uint8_t*)buffer, length); }
};
