// Minimal stub for WebKit's wtf/Assertions.h
// Only provides ASSERT macro used by uWS LoopData.h and AsyncSocket.h.
// No JSC/WebKit dependency — just wraps standard C++ assert().

#ifndef WTF_ASSERTIONS_H
#define WTF_ASSERTIONS_H

#include <cassert>

#define ASSERT(assertion) assert(assertion)

#define ASSERT_NOT_REACHED() do { assert(false); __builtin_unreachable(); } while (0)

#define RELEASE_ASSERT(assertion) assert(assertion)

#define LOG_ASSERTION_FAILURE(message) ((void)0)

#endif // WTF_ASSERTIONS_H
