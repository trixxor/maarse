#pragma once
// The few places where Windows and POSIX (Linux) differ, so the worker builds on both.
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstdio>

#ifdef _WIN32
  #include <windows.h>
  #include <bcrypt.h>
  #include <direct.h>
  #include <io.h>
#else
  #include <fcntl.h>
  #include <sys/random.h>
  #include <sys/stat.h>
  #include <unistd.h>
#endif

namespace portable {

inline void sleepMs(unsigned long ms) {
#ifdef _WIN32
  Sleep((DWORD)ms);
#else
  usleep((useconds_t)(ms * 1000));
#endif
}

inline unsigned long processId() {
#ifdef _WIN32
  return (unsigned long)GetCurrentProcessId();
#else
  return (unsigned long)getpid();
#endif
}

// Fills `dest` with random bytes from the operating system. False on failure.
inline bool randomBytes(uint8_t* dest, size_t size) {
#ifdef _WIN32
  return BCryptGenRandom(nullptr, dest, (ULONG)size, BCRYPT_USE_SYSTEM_PREFERRED_RNG) == 0;
#else
  while (size > 0) {
    const ssize_t got = getrandom(dest, size, 0);
    if (got < 0) {
      if (errno == EINTR) continue;
      return false;
    }
    dest += got;
    size -= (size_t)got;
  }
  return true;
#endif
}

// Creates a folder (readable by the owner only on Linux). True when it exists afterwards.
inline bool makeDirectory(const char* path) {
#ifdef _WIN32
  return _mkdir(path) == 0 || errno == EEXIST;
#else
  return mkdir(path, 0700) == 0 || errno == EEXIST;
#endif
}

// Replaces `to` with `from` in one step.
inline bool replaceFile(const char* from, const char* to) {
#ifdef _WIN32
  return MoveFileExA(from, to, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != 0;
#else
  return rename(from, to) == 0;
#endif
}

// Flushes a file that has been written to disk. True on success.
inline bool syncFile(FILE* file) {
#ifdef _WIN32
  return _commit(_fileno(file)) == 0;
#else
  return fsync(fileno(file)) == 0;
#endif
}

}  // namespace portable
