#ifndef _GCC_COMPAT_H
#define _GCC_COMPAT_H

// GCC compat: __has_feature is Clang-only. Define as 0 for GCC builds.
#ifndef __has_feature
#define __has_feature(x) 0
#endif

// BAO PORT (absorb oven-sh/bun 4af1842c8c): epoll_kqueue.c calls upstream
// mimalloc-fork's thread-idle scavenger hand-off. Bao's vendored mimalloc
// predates the API, so the symbols are supplied as no-ops from Rust
// (src/c_hooks.rs) — these prototypes keep the C99 build strict-error free.
// `mi_on_thread_idle_start` returning 0 is upstream's "scavenger declined"
// outcome, which selects the loop's own inline rate-limited sweep.
int mi_on_thread_idle_start(void);
void mi_on_thread_idle(void);
void mi_on_thread_idle_end(void);

#endif
