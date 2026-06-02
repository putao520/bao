#ifndef _GCC_COMPAT_H
#define _GCC_COMPAT_H

// GCC compat: __has_feature is Clang-only. Define as 0 for GCC builds.
#ifndef __has_feature
#define __has_feature(x) 0
#endif

#endif
