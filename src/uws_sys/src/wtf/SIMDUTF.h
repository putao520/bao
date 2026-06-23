// Minimal stub for WebKit's wtf/SIMDUTF.h
// Provides simdutf::validate_utf8 via bun_simdutf_sys.
// No JSC/WebKit dependency.

#ifndef WTF_SIMDUTF_H
#define WTF_SIMDUTF_H

#include <cstddef>
#include <cstdint>

// The Rust crate bun_simdutf_sys exports this as a C-linkage symbol
// (#[no_mangle pub unsafe extern "C" fn).  Must be declared outside any
// namespace so the C++ compiler looks for the unmangled C symbol.
extern "C" bool bun_simdutf_validate_utf8(const char *buf, size_t len);

namespace simdutf {

inline bool validate_utf8(const char *buf, size_t len) {
    return bun_simdutf_validate_utf8(buf, len);
}

} // namespace simdutf

#endif // WTF_SIMDUTF_H
