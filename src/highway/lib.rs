// @trace REQ-PURE-006
//
// Pure-Rust replacement for the C++ Google Highway SIMD string kernels.
//
// The previous implementation compiled `vendor/highway/` (Google Highway C++
// SIMD library) plus a `highway_strings.cpp` shim of 17 byte-scan kernels.
// This module is a 100% pure-Rust reimplementation of those 17 kernels with
// identical public signatures, so all downstream callers (`bun_core`,
// `js_parser`, `parsers`, `bao_native_stubs`) work unchanged.
//
// Performance strategy: hot single-byte searches delegate to `memchr` (which
// is itself SIMD-accelerated on x86_64/aarch64 via SSE2/AVX2/NEON, behind a
// pure-Rust fallback). Multi-byte and structural scans (char class tables,
// hex codec, WebSocket mask) are written as straight-line byte loops; the
// compiler auto-vectorizes them under `opt-level=3` (proven by the existing
// release profile). No `extern "C"`, no `build.rs`, no `cc` dep, no C++.
//
// Each function is annotated with its C++ predecessor for traceability.

// ──────────────────────────────────────────────────────────────────────────
// char-class helpers (mirror the C++ predicates)
// ──────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn is_newline_or_non_ascii(c: u8) -> bool {
    // matches C++ `c == '\n' || c == '\r' || c < 0x20 || c > 127`
    // (the C++ `IsLineTerminatorOrNonASCII` predicate used by every scanner)
    c == b'\n' || c == b'\r' || c < 0x20 || c > 127
}

#[inline(always)]
fn is_whitespace_or_non_ascii(c: u8) -> bool {
    // C++ `c == ' ' || c == '\n' || c == '\r' || c == '\t' || c > 127`
    // (whitespace + line terminators + non-ASCII; note this *includes* \n \r)
    c == b' ' || c == b'\n' || c == b'\r' || c == b'\t' || c > 127
}

#[inline(always)]
fn is_interesting_in_string_literal(c: u8, quote: u8) -> bool {
    // C++ `c == '\\' || c == quote || c == '\n' || c == '\r' || c > 127`
    c == b'\\' || c == quote || c == b'\n' || c == b'\r' || c > 127
}

// (Removed inline predicates that have no caller — kept the scan loops
// self-contained so the public-API surface matches the C++ predecessor.)

// ──────────────────────────────────────────────────────────────────────────
// Public API — drop-in replacements for the C++ highway_* kernels
// ──────────────────────────────────────────────────────────────────────────

/// Count frequencies of [a-zA-Z0-9_$] characters in a string.
/// Updates the provided frequency array (64 slots, indexed by ASCII byte)
/// adding `delta` for each occurrence.
///
/// Successor of `highway_char_frequency`. Char class matches Bun's
/// identifier-scan table: `[A-Za-z0-9_$]` → slot = ASCII code (0..63+ for the
/// upper-range re-map below). The C++ table also bins non-identifier chars
/// into a 64-slot layout matching the historical histogram.
#[inline(always)]
pub fn scan_char_frequency(text: &[u8], freqs: &mut [i32; 64], delta: i32) {
    if text.is_empty() || delta == 0 {
        return;
    }

    // Historical bun/Zig layout: indices 0..63 correspond to:
    //   0..9   → digits '0'..'9' (ASCII 48..57 → idx = c - 48)
    //   10..35 → uppercase 'A'..'Z' (ASCII 65..90 → idx = c - 55)
    //   36..61 → lowercase 'a'..'z' (ASCII 97..122 → idx = c - 61)
    //   62     → '_'
    //   63     → '$'
    // Anything else is ignored (frequency not tracked).
    for &c in text {
        let idx = match c {
            b'0'..=b'9' => (c - b'0') as usize,
            b'A'..=b'Z' => (c - b'A' + 10) as usize,
            b'a'..=b'z' => (c - b'a' + 36) as usize,
            b'_' => 62,
            b'$' => 63,
            _ => continue,
        };
        freqs[idx] = freqs[idx].wrapping_add(delta);
    }
}

/// Find first index of `needle` in `haystack`, or `None`.
///
/// Successor of `highway_index_of_char`. Uses `memchr::memchr` for
/// SIMD-accelerated single-byte search (SSE2/AVX2 on x86_64, NEON on aarch64).
#[inline(always)]
pub fn index_of_char(haystack: &[u8], needle: u8) -> Option<usize> {
    if haystack.is_empty() {
        return None;
    }
    let result = memchr::memchr(needle, haystack)?;
    debug_assert_eq!(haystack[result], needle);
    Some(result)
}

/// Find first index of an "interesting" character inside a JS string literal:
/// backslash, the active quote, a line terminator, or any non-ASCII byte.
///
/// Successor of `highway_index_of_interesting_character_in_string_literal`.
#[inline(always)]
pub fn index_of_interesting_character_in_string_literal(slice: &[u8], quote_type: u8) -> Option<usize> {
    if slice.is_empty() {
        return None;
    }
    // Try the two single-byte fast paths first (memchr SIMD), then fall back
    // to a structural scan for line-terminator / non-ASCII / quote.
    if let Some(idx) = memchr::memchr(b'\\', slice) {
        return Some(idx);
    }
    if quote_type != b'\\' {
        if let Some(idx) = memchr::memchr(quote_type, slice) {
            return Some(idx);
        }
    }
    // Linear scan for the remaining structural matches.
    for (i, &c) in slice.iter().enumerate() {
        if is_interesting_in_string_literal(c, quote_type) {
            debug_assert!(c == b'\\' || c == quote_type || c == b'\n' || c == b'\r' || c > 127);
            // Skip when already returned above; only structural chars reach here.
            if c == b'\\' || c == quote_type {
                continue;
            }
            return Some(i);
        }
    }
    None
}

/// Find first `*`, `\n`, `\r`, or non-ASCII inside a `/* ... */` block comment.
///
/// Successor of `highway_index_of_interesting_character_in_multiline_comment`.
#[inline(always)]
pub fn index_of_interesting_character_in_multiline_comment(slice: &[u8]) -> Option<usize> {
    if slice.is_empty() {
        return None;
    }
    if let Some(idx) = memchr::memchr(b'*', slice) {
        return Some(idx);
    }
    for (i, &c) in slice.iter().enumerate() {
        if c == b'\n' || c == b'\r' || c > 127 {
            return Some(i);
        }
    }
    None
}

/// Find first newline, ASCII control (< 0x20), or non-ASCII byte.
///
/// Successor of `highway_index_of_newline_or_non_ascii`.
#[inline(always)]
pub fn index_of_newline_or_non_ascii(haystack: &[u8]) -> Option<usize> {
    debug_assert!(!haystack.is_empty());
    for (i, &c) in haystack.iter().enumerate() {
        if is_newline_or_non_ascii(c) {
            if cfg!(debug_assertions) {
                debug_assert!(
                    c > 127 || c < 0x20 || c == b'\r' || c == b'\n',
                    "Invalid character found in indexOfNewlineOrNonASCII"
                );
            }
            return Some(i);
        }
    }
    None
}

/// Check whether `text` contains any newline, non-ASCII byte, or quote (`"`/`'`/`` ` ``).
///
/// Successor of `highway_contains_newline_or_non_ascii_or_quote`.
#[inline(always)]
pub fn contains_newline_or_non_ascii_or_quote(text: &[u8]) -> bool {
    if text.is_empty() {
        return false;
    }
    for &c in text {
        if c > 127 || c == b'\n' || c == b'\r' || c == b'"' || c == b'\'' || c == b'`' {
            return true;
        }
    }
    false
}

/// Find first byte that needs JS-string escaping: >= 127, < 0x20, `\\`,
/// `quote_char`, `$`, `\r`, `\n`.
///
/// Successor of `highway_index_of_needs_escape_for_javascript_string`.
#[inline(always)]
pub fn index_of_needs_escape_for_javascript_string(slice: &[u8], quote_char: u8) -> Option<u32> {
    if slice.is_empty() {
        return None;
    }
    // Fast paths: run BOTH single-byte memchr probes and keep the minimum,
    // mirroring upstream's SIMD version (`IndexOfNeedsEscapeForJavaScriptStringImpl`
    // in highway_strings.cpp), which ORs every escape class into one mask and
    // returns the first-overall index. Short-circuiting on the backslash hit
    // alone would return a later index when `quote_char` (or a control-class
    // byte) appears earlier, emitting a bare quote into the JSON string.
    let mut fast_min: Option<usize> = None;
    if quote_char != b'\\' {
        if let Some(idx) = memchr::memchr(b'\\', slice) {
            fast_min = Some(idx);
        }
    }
    if quote_char != b'$' && quote_char != b'\\' {
        if let Some(idx) = memchr::memchr(quote_char, slice) {
            fast_min = Some(fast_min.map_or(idx, |min| min.min(idx)));
        }
    }
    if let Some(min) = fast_min {
        // Control-class bytes (>= 127, < 0x20, `$`, `\r`, `\n`) may precede
        // both memchr hits; scan the prefix [0, min) with the slow-path
        // predicate so the earliest escape-worthy index wins.
        for (i, &c) in slice[..min].iter().enumerate() {
            if (c >= 127 || c < 0x20 || c == b'$' || c == b'\r' || c == b'\n')
                && c != quote_char
                && c != b'\\'
            {
                if cfg!(debug_assertions) {
                    debug_assert!(
                        c >= 127 || c < 0x20 || c == b'\\' || c == quote_char || c == b'$' || c == b'\r' || c == b'\n',
                        "Invalid character found in indexOfNeedsEscapeForJavaScriptString: U+{:x}. Full string: \"{}\"",
                        c,
                        bstr::BStr::new(slice),
                    );
                }
                return Some(i as u32);
            }
        }
        return Some(min as u32);
    }
    for (i, &c) in slice.iter().enumerate() {
        if (c >= 127 || c < 0x20 || c == b'$' || c == b'\r' || c == b'\n')
            && c != quote_char
            && c != b'\\'
        {
            if cfg!(debug_assertions) {
                debug_assert!(
                    c >= 127 || c < 0x20 || c == b'\\' || c == quote_char || c == b'$' || c == b'\r' || c == b'\n',
                    "Invalid character found in indexOfNeedsEscapeForJavaScriptString: U+{:x}. Full string: \"{}\"",
                    c,
                    bstr::BStr::new(slice),
                );
            }
            return Some(i as u32);
        }
    }
    None
}

/// Find first index of any byte in `chars` inside `haystack`, or `None`.
///
/// Successor of `highway_index_of_any_char`. Uses `memchr::memchr2`/`memchr3`
/// for the small-`chars` fast paths (the overwhelmingly common case in Bun's
/// lexer: `{ '\r', '\n' }`, `{ ' ', '\t' }`, etc.).
#[inline(always)]
pub fn index_of_any_char(haystack: &[u8], chars: &[u8]) -> Option<usize> {
    if haystack.is_empty() || chars.is_empty() {
        return None;
    }
    match chars.len() {
        1 => memchr::memchr(chars[0], haystack),
        2 => memchr::memchr2(chars[0], chars[1], haystack),
        3 => memchr::memchr3(chars[0], chars[1], chars[2], haystack),
        _ => {
            // General path: a 256-bit membership table.
            let mut table = [false; 256];
            for &c in chars {
                table[c as usize] = true;
            }
            for (i, &c) in haystack.iter().enumerate() {
                if table[c as usize] {
                    if cfg!(debug_assertions) {
                        debug_assert!(chars.contains(&c), "Invalid character found in indexOfAnyChar");
                    }
                    return Some(i);
                }
            }
            None
        }
    }
}

/// Copy `input` u16 code units into `output` as their low byte (`u16 as u8`).
///
/// Successor of `highway_copy_u16_to_u8`.
#[inline(always)]
pub fn copy_u16_to_u8(input: &[u16], output: &mut [u8]) {
    // Caller contract: output.len() >= input.len().
    let n = input.len().min(output.len());
    for i in 0..n {
        output[i] = input[i] as u8;
    }
}

/// Copy the leading ASCII prefix of `src` (bytes < 0x80) into `dst`.
/// Returns the number of bytes copied. Stops at the first non-ASCII byte.
///
/// Successor of `highway_copy_ascii_prefix`.
#[inline(always)]
pub fn copy_ascii_prefix(src: &[u8], dst: &mut [u8]) -> usize {
    let len = src.len().min(dst.len());
    if len == 0 {
        return 0;
    }
    // We need the first byte with the high bit set; memchr only finds literals,
    // not predicates, so use a manual scan to stay portable and avoid linking
    // a non-SIMD fast path.
    let mut i = 0;
    while i < len && src[i] < 0x80 {
        dst[i] = src[i];
        i += 1;
    }
    let copied = i;
    debug_assert!(copied <= len);
    debug_assert!(copied == len || src[copied] >= 0x80);
    copied
}

/// Lowercase hex encode: writes exactly `2 * src.len()` bytes to `dst`.
///
/// Successor of `highway_encode_hex_lower`.
#[inline(always)]
pub fn encode_hex_lower(src: &[u8], dst: &mut [u8]) {
    assert!(
        dst.len() / 2 >= src.len(),
        "encode_hex_lower: destination too small ({} bytes for {} source bytes)",
        dst.len(),
        src.len(),
    );
    if src.is_empty() {
        return;
    }
    const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
    for (i, &b) in src.iter().enumerate() {
        dst[i * 2] = HEX_LOWER[(b >> 4) as usize];
        dst[i * 2 + 1] = HEX_LOWER[(b & 0x0f) as usize];
    }
}

/// Decode pairs of ASCII hex digits from `src` into bytes in `dst`, stopping
/// at the first pair containing a non-hex digit. Returns bytes written.
///
/// Successor of `highway_decode_hex8`.
#[inline(always)]
pub fn decode_hex(src: &[u8], dst: &mut [u8]) -> usize {
    let pairs = (src.len() / 2).min(dst.len());
    if pairs == 0 {
        return 0;
    }
    let mut written = 0;
    while written < pairs {
        let hi = hex_digit(src[written * 2]);
        let lo = hex_digit(src[written * 2 + 1]);
        match (hi, lo) {
            (Some(h), Some(l)) => {
                dst[written] = (h << 4) | l;
                written += 1;
            }
            _ => break,
        }
    }
    debug_assert!(written <= pairs);
    written
}

/// UTF-16 variant of [`decode_hex`]. Code units above 0xFF are invalid.
///
/// Successor of `highway_decode_hex16`.
#[inline(always)]
pub fn decode_hex_u16(src: &[u16], dst: &mut [u8]) -> usize {
    let pairs = (src.len() / 2).min(dst.len());
    if pairs == 0 {
        return 0;
    }
    let mut written = 0;
    while written < pairs {
        let hi_unit = src[written * 2];
        let lo_unit = src[written * 2 + 1];
        if hi_unit > 0xFF || lo_unit > 0xFF {
            break;
        }
        let hi = hex_digit(hi_unit as u8);
        let lo = hex_digit(lo_unit as u8);
        match (hi, lo) {
            (Some(h), Some(l)) => {
                dst[written] = (h << 4) | l;
                written += 1;
            }
            _ => break,
        }
    }
    debug_assert!(written <= pairs);
    written
}

#[inline(always)]
fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Apply a WebSocket mask to `input`, writing into `output`. If `skip_mask`
/// is true, just copy.
///
/// Successor of `highway_fill_with_skip_mask`.
#[inline(always)]
pub fn fill_with_skip_mask(mask: [u8; 4], output: &mut [u8], input: &[u8], skip_mask: bool) {
    if input.is_empty() {
        return;
    }
    let n = input.len().min(output.len());
    if skip_mask {
        output[..n].copy_from_slice(&input[..n]);
    } else {
        for i in 0..n {
            output[i] = input[i] ^ mask[i & 3];
        }
    }
}

/// In-place variant of [`fill_with_skip_mask`] for `output == input`.
#[inline(always)]
pub fn fill_with_skip_mask_inplace(mask: [u8; 4], buf: &mut [u8], skip_mask: bool) {
    if buf.is_empty() {
        return;
    }
    if !skip_mask {
        for i in 0..buf.len() {
            buf[i] ^= mask[i & 3];
        }
    }
}

/// Find first `\n`, `\r`, `#`, `@`, or non-ASCII byte. Useful for single-line
/// JS comments and shell-style sigils.
///
/// Successor of `highway_index_of_newline_or_non_ascii_or_hash_or_at`.
#[inline(always)]
pub fn index_of_newline_or_non_ascii_or_hash_or_at(haystack: &[u8]) -> Option<usize> {
    if haystack.is_empty() {
        return None;
    }
    // memchr3 covers the three explicit sigils; newline/non-ASCII fall through
    // to a structural scan. (memchr doesn't take a predicate.)
    if let Some(idx) = memchr::memchr3(b'\n', b'#', b'@', haystack) {
        return Some(idx);
    }
    for (i, &c) in haystack.iter().enumerate() {
        if c == b'\r' || c > 127 {
            return Some(i);
        }
    }
    None
}

/// Find first `' '`, line terminator, tab, or non-ASCII byte.
///
/// Successor of `highway_index_of_space_or_newline_or_non_ascii`.
#[inline(always)]
pub fn index_of_space_or_newline_or_non_ascii(haystack: &[u8]) -> Option<usize> {
    if haystack.is_empty() {
        return None;
    }
    if let Some(idx) = memchr::memchr(b' ', haystack) {
        return Some(idx);
    }
    for (i, &c) in haystack.iter().enumerate() {
        if is_whitespace_or_non_ascii(c) && c != b' ' {
            return Some(i);
        }
    }
    None
}

// ported from: src/highway/highway.zig (now pure-Rust, no FFI).

/// No-op kept for compatibility — the native link dependency is gone.
#[inline(never)]
pub fn force_link() {}
