/* -*- Mode: C++; tab-width: 8; indent-tabs-mode: nil; c-basic-offset: 2 -*-
 * vim: set ts=8 sts=2 et sw=2 tw=80:
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// BAO-8 patch: env-gated empty-chunk-pool cap, default OFF.
//
// Bao soak W5/W31 attribution: a chunk emptied at GC time is parked in
// GCRuntime::emptyChunks_ and is only unmapped inside the decommit pass
// (startDecommit -> BackgroundDecommitTask::run -> expireEmptyChunkPool ->
// FreeChunkPool/UnmapPages). GCOptions::Normal GCs skip that pass entirely
// while inHighFrequencyGCMode() is set (GCRuntime::shouldDecommit), so churn
// workloads park every freed chunk until some Shrink GC happens to run
// (measured: 202 MiB parked after 100 realm churns + 2 plain GCs).
//
// BAO_SM_CHUNK_POOL_MAX=N (N > 0) bounds that parked pool: pool pushes cap at
// N and excess chunks return to the OS immediately. 0/unset = upstream
// semantics (no cap). The value is read once per process.

#ifndef gc_BaoChunkPoolCap_h
#define gc_BaoChunkPoolCap_h

#include <cstdlib>

#include "js/Utility.h"

namespace js::gc {

inline size_t BaoChunkPoolMax() {
  static size_t cached = [] {
    if (const char* v = getenv("BAO_SM_CHUNK_POOL_MAX")) {
      char* end = nullptr;
      unsigned long long n = strtoull(v, &end, 10);
      if (end != v && *end == '\0' && n > 0 && n <= (1ull << 20)) {
        return static_cast<size_t>(n);
      }
    }
    return static_cast<size_t>(0);
  }();
  return cached;
}

}  // namespace js::gc

#endif  // gc_BaoChunkPoolCap_h
