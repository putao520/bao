#ifndef _GCC_COMPAT_H
#define _GCC_COMPAT_H

// GCC compat: __has_feature is Clang-only. Define as 0 for GCC builds.
#ifndef __has_feature
#define __has_feature(x) 0
#endif

// NOTE: the absorbed epoll_kqueue.c mi_on_thread_idle_* prototypes live in
// that TU itself (next to the removed <mimalloc.h> include, issue #59), not
// here. This wrapper is force-included into both the C and C++ builds; a
// declaration kept here would carry C++ linkage in the C++ TU and desync
// from the Rust `extern "C"` no-op definitions in src/c_hooks.rs.

#endif
