// Minimal stub for WebKit's wtf/SIMDUTF.h
// Provides simdutf::validate_utf8 via bun_simdutf_sys.
// No JSC/WebKit dependency.

#ifndef WTF_SIMDUTF_H
#define WTF_SIMDUTF_H

#include <cstddef>
#include <cstdint>

namespace simdutf {

inline bool validate_utf8(const char *buf, size_t len) {
    // Use bun_simdutf_sys (already compiled in workspace)
    extern bool bun_simdutf_validate_utf8(const char *buf, size_t len);
    return bun_simdutf_validate_utf8(buf, len);
}

} // namespace simdutf

#endif // WTF_SIMDUTF_H
