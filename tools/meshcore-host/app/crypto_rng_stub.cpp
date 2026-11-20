// rweather/Crypto references a global `RNG` (only in key-generation paths MeshCore
// never calls - MeshCore supplies its own mesh::RNG). Back it with the OS CSPRNG anyway.
#include <RNG.h>
#include "portable.h"
RNGClass::RNGClass() {}
RNGClass::~RNGClass() {}
void RNGClass::rand(uint8_t* data, size_t len) {
  portable::randomBytes(data, len);
}
RNGClass RNG;
