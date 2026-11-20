#pragma once
// Minimal Arduino-API shim so the official MeshCore core compiles natively on a PC.
// Only what the core + BaseChatMesh actually use is provided.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdarg>
#include <cmath>
#include <algorithm>
#include "Stream.h"

using std::isnan;

unsigned long millis();
void delay(unsigned long ms);
long random(long min_v, long max_v);
long random(long max_v);
void randomSeed(unsigned long seed);

#ifndef PI
#define PI 3.14159265358979323846
#endif

#ifndef _WIN32
// Arduino and the Windows C runtime have ltoa(); glibc does not. The caller
// gives a buffer big enough for the number, as it does with the real one.
inline char* ltoa(long value, char* buffer, int radix) {
  if (radix < 2 || radix > 36) { buffer[0] = 0; return buffer; }
  char digits[66];
  int count = 0;
  unsigned long magnitude = value < 0 ? 0UL - (unsigned long)value : (unsigned long)value;
  do { digits[count++] = "0123456789abcdefghijklmnopqrstuvwxyz"[magnitude % radix]; magnitude /= radix; } while (magnitude);
  char* out = buffer;
  if (value < 0 && radix == 10) *out++ = '-';
  while (count) *out++ = digits[--count];
  *out = 0;
  return buffer;
}
#endif

// Console "Serial" -> stdout (stdin handled by the app itself)
class ConsoleSerial : public Stream {
public:
  void begin(unsigned long) {}
  size_t write(uint8_t b) override { fputc(b, stdout); return 1; }
  size_t write(const uint8_t* buf, size_t n) override { fwrite(buf, 1, n, stdout); return n; }
  using Print::write;
  int printf(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt); int r = vprintf(fmt, ap); va_end(ap); fflush(stdout); return r;
  }
  size_t println() { fputc('\n', stdout); fflush(stdout); return 1; }
  size_t println(const char* s) { fputs(s, stdout); return println() + strlen(s); }
  size_t println(int v) { return printf("%d\n", v); }
  size_t print(const char* s) override { fputs(s, stdout); return strlen(s); }
  size_t print(char c) { fputc(c, stdout); return 1; }
  size_t print(int v, int r = DEC) override { return r == HEX ? printf("%X", v) : printf("%d", v); }
  size_t print(unsigned int v, int r = DEC) override { return r == HEX ? printf("%X", v) : printf("%u", v); }
  size_t print(long v, int r = DEC) override { return printf("%ld", v); }
  size_t print(unsigned long v, int r = DEC) override { return printf("%lu", v); }
  size_t print(double v, int p = 2) override { return printf("%.*f", p, v); }
  void flush() override { fflush(stdout); }
};
extern ConsoleSerial Serial;
