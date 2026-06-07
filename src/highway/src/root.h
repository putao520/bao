#pragma once
// Minimal root.h for highway_strings.cpp — provides OS() macros, assertions, HWY_RESTRICT.

#ifdef __linux__
#define OS_LINUX 1
#define OS(LINUX) OS_LINUX
#elif defined(__APPLE__)
#define OS_DARWIN 1
#define OS(DARWIN) OS_DARWIN
#elif defined(_WIN32)
#define OS_WINDOWS 1
#define OS(WINDOWS) OS_WINDOWS
#endif

#ifndef HWY_RESTRICT
#define HWY_RESTRICT __restrict__
#endif

#include <cassert>
#include <cstdio>
#include <cstdlib>

// WebKit WTF assertion macros — map to standard assert/abort
#define ASSERT(expr) assert(expr)
#define ASSERT_NOT_REACHED_WITH_MESSAGE(...) \
    do { std::fprintf(stderr, "ASSERT_NOT_REACHED: "); \
         std::fprintf(stderr, __VA_ARGS__); \
         std::fputc('\n', stderr); \
         std::abort(); } while (0)
#define RELEASE_ASSERT(expr) assert(expr)
