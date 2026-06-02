// @trace REQ-ENG-004 [algorithm:simdutf] SIMD-accelerated UTF validation/conversion.
// Binding file: pedantic clippy lints suppressed because bit-packed decode logic
// intentionally mixes precedence (added parens would harm readability) and unsafe
// blocks are prevalent FFI patterns.
#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::manual_div_ceil,
    clippy::precedence
)]

use core::ffi::{c_int, c_uint};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SIMDUTFResult {
    pub status: Status,
    pub count: usize,
}

impl SIMDUTFResult {
    pub fn is_successful(&self) -> bool {
        self.status == Status::SUCCESS
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Status(pub i32);

impl Status {
    pub const SUCCESS: Status = Status(0);
    pub const TOO_SHORT: Status = Status(2);
    pub const TOO_LONG: Status = Status(3);
    pub const OVERLONG: Status = Status(4);
    pub const TOO_LARGE: Status = Status(5);
    pub const SURROGATE: Status = Status(6);
    pub const INVALID_BASE64_CHARACTER: Status = Status(7);
    pub const BASE64_INPUT_REMAINDER: Status = Status(8);
    pub const OUTPUT_BUFFER_TOO_SMALL: Status = Status(9);
}

// ---------------------------------------------------------------------------
// Internal helpers — pure Rust UTF validation and conversion
// ---------------------------------------------------------------------------

#[inline]
fn validate_utf8_impl(buf: &[u8]) -> bool {
    core::str::from_utf8(buf).is_ok()
}

#[inline]
fn validate_utf8_with_errors_impl(buf: &[u8]) -> SIMDUTFResult {
    match core::str::from_utf8(buf) {
        Ok(_) => SIMDUTFResult { status: Status::SUCCESS, count: buf.len() },
        Err(e) => SIMDUTFResult { status: Status::TOO_SHORT, count: e.valid_up_to() },
    }
}

#[inline]
fn validate_ascii_impl(buf: &[u8]) -> bool {
    buf.iter().all(|&b| b < 0x80)
}

#[inline]
fn validate_ascii_with_errors_impl(buf: &[u8]) -> SIMDUTFResult {
    for (i, &b) in buf.iter().enumerate() {
        if b >= 0x80 {
            return SIMDUTFResult { status: Status::TOO_LONG, count: i };
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: buf.len() }
}

fn validate_utf16le_impl(buf: &[u16]) -> bool {
    let mut i = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            i += 1;
        } else if unit >= 0xD800 && unit <= 0xDBFF {
            i += 1;
            if i >= buf.len() {
                return false;
            }
            let low = buf[i];
            if low < 0xDC00 || low > 0xDFFF {
                return false;
            }
            i += 1;
        } else {
            return false;
        }
    }
    true
}

fn validate_utf16be_impl(buf: &[u16]) -> bool {
    validate_utf16le_impl(buf)
}

fn validate_utf16le_with_errors_impl(buf: &[u16]) -> SIMDUTFResult {
    let mut i = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            i += 1;
        } else if unit >= 0xD800 && unit <= 0xDBFF {
            i += 1;
            if i >= buf.len() {
                return SIMDUTFResult { status: Status::SURROGATE, count: i - 1 };
            }
            let low = buf[i];
            if low < 0xDC00 || low > 0xDFFF {
                return SIMDUTFResult { status: Status::SURROGATE, count: i };
            }
            i += 1;
        } else {
            return SIMDUTFResult { status: Status::SURROGATE, count: i };
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: buf.len() }
}

fn validate_utf16be_with_errors_impl(buf: &[u16]) -> SIMDUTFResult {
    validate_utf16le_with_errors_impl(buf)
}

fn validate_utf32_impl(buf: &[u32]) -> bool {
    buf.iter().all(|&cp| cp <= 0x10FFFF && !(cp >= 0xD800 && cp <= 0xDFFF))
}

fn validate_utf32_with_errors_impl(buf: &[u32]) -> SIMDUTFResult {
    for (i, &cp) in buf.iter().enumerate() {
        if cp > 0x10FFFF {
            return SIMDUTFResult { status: Status::TOO_LARGE, count: i };
        }
        if cp >= 0xD800 && cp <= 0xDFFF {
            return SIMDUTFResult { status: Status::SURROGATE, count: i };
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: buf.len() }
}

fn convert_utf8_to_utf16le_impl(buf: &[u8], out: &mut [u16]) -> usize {
    let s = match core::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut written = 0;
    for c in s.chars() {
        let cp = c as u32;
        if cp <= 0xFFFF {
            out[written] = cp as u16;
            written += 1;
        } else {
            let cp = cp - 0x10000;
            out[written] = (0xD800 + (cp >> 10)) as u16;
            out[written + 1] = (0xDC00 + (cp & 0x3FF)) as u16;
            written += 2;
        }
    }
    written
}

fn convert_utf8_to_utf16be_impl(buf: &[u8], out: &mut [u16]) -> usize {
    let count = convert_utf8_to_utf16le_impl(buf, out);
    for i in 0..count {
        out[i] = out[i].to_be();
    }
    count
}

fn convert_utf8_to_utf16le_with_errors_impl(buf: &[u8], out: &mut [u16]) -> SIMDUTFResult {
    let s = match core::str::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => return SIMDUTFResult { status: Status::TOO_SHORT, count: e.valid_up_to() },
    };
    let mut written = 0;
    for c in s.chars() {
        let cp = c as u32;
        if cp <= 0xFFFF {
            out[written] = cp as u16;
            written += 1;
        } else {
            let cp = cp - 0x10000;
            out[written] = (0xD800 + (cp >> 10)) as u16;
            out[written + 1] = (0xDC00 + (cp & 0x3FF)) as u16;
            written += 2;
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf8_to_utf16be_with_errors_impl(buf: &[u8], out: &mut [u16]) -> SIMDUTFResult {
    let result = convert_utf8_to_utf16le_with_errors_impl(buf, out);
    if result.status == Status::SUCCESS {
        for i in 0..result.count {
            out[i] = out[i].to_be();
        }
    }
    result
}

fn convert_utf8_to_utf32_impl(buf: &[u8], out: &mut [u32]) -> usize {
    let s = match core::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut written = 0;
    for c in s.chars() {
        out[written] = c as u32;
        written += 1;
    }
    written
}

fn convert_utf8_to_utf32_with_errors_impl(buf: &[u8], out: &mut [u32]) -> SIMDUTFResult {
    let s = match core::str::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => return SIMDUTFResult { status: Status::TOO_SHORT, count: e.valid_up_to() },
    };
    let mut written = 0;
    for c in s.chars() {
        out[written] = c as u32;
        written += 1;
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf16le_to_utf8_impl(buf: &[u16], out: &mut [u8]) -> usize {
    let mut i = 0;
    let mut written = 0;
    while i < buf.len() {
        let unit = buf[i];
        let cp = if unit < 0xD800 || unit > 0xDFFF {
            i += 1;
            unit as u32
        } else if unit <= 0xDBFF && i + 1 < buf.len() {
            let low = buf[i + 1];
            if low >= 0xDC00 && low <= 0xDFFF {
                i += 2;
                0x10000 + (((unit as u32) - 0xD800) << 10) | ((low as u32) - 0xDC00)
            } else {
                i += 1;
                0xFFFD
            }
        } else {
            i += 1;
            0xFFFD
        };
        written += encode_utf8_char(cp, &mut out[written..]);
    }
    written
}

fn convert_utf16be_to_utf8_impl(buf: &[u16], out: &mut [u8]) -> usize {
    let le: Vec<u16> = buf.iter().map(|&v| u16::from_be(v)).collect();
    convert_utf16le_to_utf8_impl(&le, out)
}

fn convert_utf16le_to_utf8_with_errors_impl(buf: &[u16], out: &mut [u8]) -> SIMDUTFResult {
    let mut i = 0;
    let mut written = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            written += encode_utf8_char(unit as u32, &mut out[written..]);
            i += 1;
        } else if unit <= 0xDBFF {
            i += 1;
            if i >= buf.len() {
                return SIMDUTFResult { status: Status::SURROGATE, count: written };
            }
            let low = buf[i];
            if low < 0xDC00 || low > 0xDFFF {
                return SIMDUTFResult { status: Status::SURROGATE, count: written };
            }
            let cp = 0x10000 + (((unit as u32) - 0xD800) << 10) | ((low as u32) - 0xDC00);
            written += encode_utf8_char(cp, &mut out[written..]);
            i += 1;
        } else {
            return SIMDUTFResult { status: Status::SURROGATE, count: written };
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf16be_to_utf8_with_errors_impl(buf: &[u16], out: &mut [u8]) -> SIMDUTFResult {
    let le: Vec<u16> = buf.iter().map(|&v| u16::from_be(v)).collect();
    convert_utf16le_to_utf8_with_errors_impl(&le, out)
}

fn convert_utf32_to_utf8_impl(buf: &[u32], out: &mut [u8]) -> usize {
    let mut written = 0;
    for &cp in buf {
        let cp = if cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) {
            0xFFFD
        } else {
            cp
        };
        written += encode_utf8_char(cp, &mut out[written..]);
    }
    written
}

fn convert_utf32_to_utf8_with_errors_impl(buf: &[u32], out: &mut [u8]) -> SIMDUTFResult {
    let mut written = 0;
    for (i, &cp) in buf.iter().enumerate() {
        if cp > 0x10FFFF {
            return SIMDUTFResult { status: Status::TOO_LARGE, count: i };
        }
        if cp >= 0xD800 && cp <= 0xDFFF {
            return SIMDUTFResult { status: Status::SURROGATE, count: i };
        }
        written += encode_utf8_char(cp, &mut out[written..]);
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf32_to_utf16le_impl(buf: &[u32], out: &mut [u16]) -> usize {
    let mut written = 0;
    for &cp in buf {
        let cp = if cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) { 0xFFFD } else { cp };
        if cp <= 0xFFFF {
            out[written] = cp as u16;
            written += 1;
        } else {
            let cp = cp - 0x10000;
            out[written] = (0xD800 + (cp >> 10)) as u16;
            out[written + 1] = (0xDC00 + (cp & 0x3FF)) as u16;
            written += 2;
        }
    }
    written
}

fn convert_utf32_to_utf16be_impl(buf: &[u32], out: &mut [u16]) -> usize {
    let count = convert_utf32_to_utf16le_impl(buf, out);
    for i in 0..count {
        out[i] = out[i].to_be();
    }
    count
}

fn convert_utf32_to_utf16le_with_errors_impl(buf: &[u32], out: &mut [u16]) -> SIMDUTFResult {
    let mut written = 0;
    for (i, &cp) in buf.iter().enumerate() {
        if cp > 0x10FFFF {
            return SIMDUTFResult { status: Status::TOO_LARGE, count: i };
        }
        if cp >= 0xD800 && cp <= 0xDFFF {
            return SIMDUTFResult { status: Status::SURROGATE, count: i };
        }
        if cp <= 0xFFFF {
            out[written] = cp as u16;
            written += 1;
        } else {
            let cp = cp - 0x10000;
            out[written] = (0xD800 + (cp >> 10)) as u16;
            out[written + 1] = (0xDC00 + (cp & 0x3FF)) as u16;
            written += 2;
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf32_to_utf16be_with_errors_impl(buf: &[u32], out: &mut [u16]) -> SIMDUTFResult {
    let result = convert_utf32_to_utf16le_with_errors_impl(buf, out);
    if result.status == Status::SUCCESS {
        for i in 0..result.count {
            out[i] = out[i].to_be();
        }
    }
    result
}

fn convert_utf16le_to_utf32_impl(buf: &[u16], out: &mut [u32]) -> usize {
    let mut i = 0;
    let mut written = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            out[written] = unit as u32;
            written += 1;
            i += 1;
        } else if unit <= 0xDBFF && i + 1 < buf.len() {
            let low = buf[i + 1];
            if low >= 0xDC00 && low <= 0xDFFF {
                out[written] = 0x10000 + (((unit as u32) - 0xD800) << 10) | ((low as u32) - 0xDC00);
                written += 1;
                i += 2;
            } else {
                out[written] = 0xFFFD;
                written += 1;
                i += 1;
            }
        } else {
            out[written] = 0xFFFD;
            written += 1;
            i += 1;
        }
    }
    written
}

fn convert_utf16be_to_utf32_impl(buf: &[u16], out: &mut [u32]) -> usize {
    let le: Vec<u16> = buf.iter().map(|&v| u16::from_be(v)).collect();
    convert_utf16le_to_utf32_impl(&le, out)
}

fn convert_utf16le_to_utf32_with_errors_impl(buf: &[u16], out: &mut [u32]) -> SIMDUTFResult {
    let mut i = 0;
    let mut written = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            out[written] = unit as u32;
            written += 1;
            i += 1;
        } else if unit <= 0xDBFF {
            i += 1;
            if i >= buf.len() {
                return SIMDUTFResult { status: Status::SURROGATE, count: written };
            }
            let low = buf[i];
            if low < 0xDC00 || low > 0xDFFF {
                return SIMDUTFResult { status: Status::SURROGATE, count: written };
            }
            out[written] = 0x10000 + (((unit as u32) - 0xD800) << 10) | ((low as u32) - 0xDC00);
            written += 1;
            i += 1;
        } else {
            return SIMDUTFResult { status: Status::SURROGATE, count: written };
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn convert_utf16be_to_utf32_with_errors_impl(buf: &[u16], out: &mut [u32]) -> SIMDUTFResult {
    let le: Vec<u16> = buf.iter().map(|&v| u16::from_be(v)).collect();
    convert_utf16le_to_utf32_with_errors_impl(&le, out)
}

fn convert_latin1_to_utf8_impl(buf: &[u8], out: &mut [u8]) -> usize {
    let mut written = 0;
    for &b in buf {
        let cp = b as u32;
        written += encode_utf8_char(cp, &mut out[written..]);
    }
    written
}

#[inline]
fn encode_utf8_char(cp: u32, out: &mut [u8]) -> usize {
    if cp <= 0x7F {
        out[0] = cp as u8;
        1
    } else if cp <= 0x7FF {
        out[0] = 0xC0 | ((cp >> 6) as u8);
        out[1] = 0x80 | ((cp & 0x3F) as u8);
        2
    } else if cp <= 0xFFFF {
        out[0] = 0xE0 | ((cp >> 12) as u8);
        out[1] = 0x80 | (((cp >> 6) & 0x3F) as u8);
        out[2] = 0x80 | ((cp & 0x3F) as u8);
        3
    } else {
        out[0] = 0xF0 | ((cp >> 18) as u8);
        out[1] = 0x80 | (((cp >> 12) & 0x3F) as u8);
        out[2] = 0x80 | (((cp >> 6) & 0x3F) as u8);
        out[3] = 0x80 | ((cp & 0x3F) as u8);
        4
    }
}

fn count_utf8_impl(buf: &[u8]) -> usize {
    match core::str::from_utf8(buf) {
        Ok(s) => s.chars().count(),
        Err(_) => 0,
    }
}

fn count_utf16le_impl(buf: &[u16]) -> usize {
    let mut i = 0;
    let mut count = 0;
    while i < buf.len() {
        let unit = buf[i];
        if unit < 0xD800 || unit > 0xDFFF {
            count += 1;
            i += 1;
        } else if unit <= 0xDBFF && i + 1 < buf.len() && buf[i + 1] >= 0xDC00 && buf[i + 1] <= 0xDFFF {
            count += 1;
            i += 2;
        } else {
            count += 1;
            i += 1;
        }
    }
    count
}

fn utf8_length_from_utf16le_impl(buf: &[u16]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < buf.len() {
        let unit = buf[i];
        let cp = if unit < 0xD800 || unit > 0xDFFF {
            i += 1;
            unit as u32
        } else if unit <= 0xDBFF && i + 1 < buf.len() {
            let low = buf[i + 1];
            if low >= 0xDC00 && low <= 0xDFFF {
                i += 2;
                0x10000 + (((unit as u32) - 0xD800) << 10) | ((low as u32) - 0xDC00)
            } else {
                i += 1;
                0xFFFD
            }
        } else {
            i += 1;
            0xFFFD
        };
        len += if cp <= 0x7F { 1 } else if cp <= 0x7FF { 2 } else if cp <= 0xFFFF { 3 } else { 4 };
    }
    len
}

fn utf32_length_from_utf16le_impl(buf: &[u16]) -> usize {
    count_utf16le_impl(buf)
}

fn utf16_length_from_utf8_impl(buf: &[u8]) -> usize {
    match core::str::from_utf8(buf) {
        Ok(s) => s.chars().map(|c| if c as u32 > 0xFFFF { 2 } else { 1 }).sum(),
        Err(_) => 0,
    }
}

fn utf8_length_from_utf32_impl(buf: &[u32]) -> usize {
    let mut len = 0;
    for &cp in buf {
        let cp = if cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) { 0xFFFD } else { cp };
        len += if cp <= 0x7F { 1 } else if cp <= 0x7FF { 2 } else if cp <= 0xFFFF { 3 } else { 4 };
    }
    len
}

fn utf16_length_from_utf32_impl(buf: &[u32]) -> usize {
    let mut len = 0;
    for &cp in buf {
        let cp = if cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) { 0xFFFD } else { cp };
        len += if cp <= 0xFFFF { 1 } else { 2 };
    }
    len
}

fn utf32_length_from_utf8_impl(buf: &[u8]) -> usize {
    count_utf8_impl(buf)
}

#[allow(dead_code)]
fn utf8_length_from_latin1_impl(buf: &[u8]) -> usize {
    let mut len = 0;
    for &b in buf {
        len += if b < 0x80 { 1 } else { 2 };
    }
    len
}

#[allow(dead_code)]
fn utf16_length_from_latin1_impl(buf: &[u8]) -> usize {
    buf.len()
}

// ---------------------------------------------------------------------------
// FFI-compatible #[no_mangle] functions — pure Rust, no C library needed
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn simdutf__detect_encodings(input: *const u8, length: usize) -> c_int {
    if length == 0 {
        return 0;
    }
    let buf = unsafe { core::slice::from_raw_parts(input, length) };
    let is_ascii = validate_ascii_impl(buf);
    let is_utf8 = validate_utf8_impl(buf);
    let mut result = 0;
    if is_ascii { result |= 1; }
    if is_utf8 { result |= 2; }
    result
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf8(buf: *const u8, len: usize) -> bool {
    if len == 0 { return true; }
    validate_utf8_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf8_with_errors(buf: *const u8, len: usize) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    validate_utf8_with_errors_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_ascii(buf: *const u8, len: usize) -> bool {
    if len == 0 { return true; }
    validate_ascii_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_ascii_with_errors(buf: *const u8, len: usize) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    validate_ascii_with_errors_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf16le(buf: *const u16, len: usize) -> bool {
    if len == 0 { return true; }
    validate_utf16le_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf16be(buf: *const u16, len: usize) -> bool {
    if len == 0 { return true; }
    validate_utf16be_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf16le_with_errors(buf: *const u16, len: usize) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    validate_utf16le_with_errors_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf16be_with_errors(buf: *const u16, len: usize) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    validate_utf16be_with_errors_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf32(buf: *const c_uint, len: usize) -> bool {
    if len == 0 { return true; }
    validate_utf32_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__validate_utf32_with_errors(buf: *const c_uint, len: usize) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    validate_utf32_with_errors_impl(unsafe { core::slice::from_raw_parts(buf, len) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf16le(
    buf: *const u8, len: usize, utf16_output: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_output, max_out) };
    convert_utf8_to_utf16le_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf16be(
    buf: *const u8, len: usize, utf16_output: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_output, max_out) };
    convert_utf8_to_utf16be_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf16le_with_errors(
    buf: *const u8, len: usize, utf16_output: *mut u16,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_output, max_out) };
    convert_utf8_to_utf16le_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf16be_with_errors(
    buf: *const u8, len: usize, utf16_output: *mut u16,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_output, max_out) };
    convert_utf8_to_utf16be_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf8_to_utf16be(
    buf: *const u8, len: usize, utf16_buffer: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf8_to_utf16be_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf32(
    buf: *const u8, len: usize, utf32_output: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_output, max_out) };
    convert_utf8_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf8_to_utf32_with_errors(
    buf: *const u8, len: usize, utf32_output: *mut u32,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_output, max_out) };
    convert_utf8_to_utf32_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf8_to_utf32(
    buf: *const u8, len: usize, utf32_buffer: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf8_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16le_to_utf8(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16le_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16be_to_utf8(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16be_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16le_to_utf8_with_errors(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16le_to_utf8_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16be_to_utf8_with_errors(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16be_to_utf8_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf16le_to_utf8(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16le_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf16be_to_utf8(
    buf: *const u16, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf16be_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf8(
    buf: *const c_uint, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf32_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf8_with_errors(
    buf: *const c_uint, len: usize, utf8_buffer: *mut u8,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf32_to_utf8_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf32_to_utf8(
    buf: *const c_uint, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 4;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_utf32_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf16le(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16le_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf16be(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16be_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf16le_with_errors(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16le_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf32_to_utf16be_with_errors(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16be_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf32_to_utf16le(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16le_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf32_to_utf16be(
    buf: *const c_uint, len: usize, utf16_buffer: *mut u16,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf16_buffer, max_out) };
    convert_utf32_to_utf16be_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16le_to_utf32(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16le_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16be_to_utf32(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16be_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16le_to_utf32_with_errors(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16le_to_utf32_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_utf16be_to_utf32_with_errors(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> SIMDUTFResult {
    if len == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16be_to_utf32_with_errors_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf16le_to_utf32(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16le_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_valid_utf16be_to_utf32(
    buf: *const u16, len: usize, utf32_buffer: *mut u32,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() + 1;
    let out = unsafe { core::slice::from_raw_parts_mut(utf32_buffer, max_out) };
    convert_utf16be_to_utf32_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__convert_latin1_to_utf8(
    buf: *const u8, len: usize, utf8_buffer: *mut u8,
) -> usize {
    if len == 0 { return 0; }
    let input = unsafe { core::slice::from_raw_parts(buf, len) };
    let max_out = input.len() * 2;
    let out = unsafe { core::slice::from_raw_parts_mut(utf8_buffer, max_out) };
    convert_latin1_to_utf8_impl(input, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__change_endianness_utf16(
    buf: *const u16, length: usize, output: *mut u16,
) {
    if length == 0 { return; }
    let input = unsafe { core::slice::from_raw_parts(buf, length) };
    let out = unsafe { core::slice::from_raw_parts_mut(output, length) };
    for (i, &v) in input.iter().enumerate() {
        out[i] = v.swap_bytes();
    }
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__count_utf16le(buf: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    count_utf16le_impl(unsafe { core::slice::from_raw_parts(buf, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__count_utf16be(buf: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    let le: Vec<u16> = unsafe { core::slice::from_raw_parts(buf, length) }
        .iter().map(|&v| u16::from_be(v)).collect();
    count_utf16le_impl(&le)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__count_utf8(buf: *const u8, length: usize) -> usize {
    if length == 0 { return 0; }
    count_utf8_impl(unsafe { core::slice::from_raw_parts(buf, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf8_length_from_utf16le(input: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    utf8_length_from_utf16le_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf8_length_from_utf16be(input: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    let le: Vec<u16> = unsafe { core::slice::from_raw_parts(input, length) }
        .iter().map(|&v| u16::from_be(v)).collect();
    utf8_length_from_utf16le_impl(&le)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf32_length_from_utf16le(input: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    utf32_length_from_utf16le_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf32_length_from_utf16be(input: *const u16, length: usize) -> usize {
    if length == 0 { return 0; }
    let le: Vec<u16> = unsafe { core::slice::from_raw_parts(input, length) }
        .iter().map(|&v| u16::from_be(v)).collect();
    utf32_length_from_utf16le_impl(&le)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf16_length_from_utf8(input: *const u8, length: usize) -> usize {
    if length == 0 { return 0; }
    utf16_length_from_utf8_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf8_length_from_utf32(input: *const c_uint, length: usize) -> usize {
    if length == 0 { return 0; }
    utf8_length_from_utf32_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf16_length_from_utf32(input: *const c_uint, length: usize) -> usize {
    if length == 0 { return 0; }
    utf16_length_from_utf32_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf32_length_from_utf8(input: *const u8, length: usize) -> usize {
    if length == 0 { return 0; }
    utf32_length_from_utf8_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf8_length_from_latin1(input: *const u8, length: usize) -> usize {
    if length == 0 { return 0; }
    utf8_length_from_latin1_impl(unsafe { core::slice::from_raw_parts(input, length) })
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__utf16_length_from_latin1(_input: *const u8, length: usize) -> usize {
    length
}

// ---------------------------------------------------------------------------
// Base64 — pure Rust implementation
// ---------------------------------------------------------------------------

const B64_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_encode_table(is_urlsafe: bool) -> &'static [u8; 64] {
    if is_urlsafe { B64_URL } else { B64_STD }
}

fn b64_decode_value(byte: u8, is_urlsafe: bool) -> Option<u8> {
    const STD_DEC: [i8; 256] = {
        let mut table = [-1i8; 256];
        let mut i = 0;
        while i < 64 {
            table[b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"[i] as usize] = i as i8;
            i += 1;
        }
        table
    };
    const URL_DEC: [i8; 256] = {
        let mut table = [-1i8; 256];
        let mut i = 0;
        while i < 64 {
            table[b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"[i] as usize] = i as i8;
            i += 1;
        }
        table
    };
    let table = if is_urlsafe { &URL_DEC } else { &STD_DEC };
    let v = table[byte as usize];
    if v >= 0 { Some(v as u8) } else { None }
}

fn base64_encode_impl(input: &[u8], output: &mut [u8], is_urlsafe: bool) -> usize {
    let table = b64_encode_table(is_urlsafe);
    let mut i = 0;
    let mut written = 0;
    while i + 2 < input.len() {
        let b0 = input[i];
        let b1 = input[i + 1];
        let b2 = input[i + 2];
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        output[written] = table[((triple >> 18) & 0x3F) as usize];
        output[written + 1] = table[((triple >> 12) & 0x3F) as usize];
        output[written + 2] = table[((triple >> 6) & 0x3F) as usize];
        output[written + 3] = table[(triple & 0x3F) as usize];
        written += 4;
        i += 3;
    }
    let remaining = input.len() - i;
    if remaining == 1 {
        let b0 = input[i];
        output[written] = table[((b0 >> 2) & 0x3F) as usize];
        output[written + 1] = table[((b0 << 4) & 0x30) as usize];
        if !is_urlsafe {
            output[written + 2] = b'=';
            output[written + 3] = b'=';
        }
        written += if is_urlsafe { 2 } else { 4 };
    } else if remaining == 2 {
        let b0 = input[i];
        let b1 = input[i + 1];
        output[written] = table[((b0 >> 2) & 0x3F) as usize];
        output[written + 1] = table[(((b0 << 4) | (b1 >> 4)) & 0x3F) as usize];
        output[written + 2] = table[((b1 << 2) & 0x3C) as usize];
        if !is_urlsafe {
            output[written + 3] = b'=';
        }
        written += if is_urlsafe { 3 } else { 4 };
    }
    written
}

fn base64_decode_impl(input: &[u8], output: &mut [u8], is_urlsafe: bool) -> SIMDUTFResult {
    let mut acc: u32 = 0;
    let mut bits: i32 = 0;
    let mut written = 0;
    for &byte in input {
        if byte == b'=' { break; }
        match b64_decode_value(byte, is_urlsafe) {
            Some(v) => {
                acc = (acc << 6) | (v as u32);
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    if written >= output.len() {
                        return SIMDUTFResult { status: Status::OUTPUT_BUFFER_TOO_SMALL, count: written };
                    }
                    output[written] = ((acc >> bits) & 0xFF) as u8;
                    written += 1;
                }
            }
            None => {
                return SIMDUTFResult { status: Status::INVALID_BASE64_CHARACTER, count: written };
            }
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn base64_decode16_impl(input: &[u16], output: &mut [u8], is_urlsafe: bool) -> SIMDUTFResult {
    let bytes: Vec<u8> = input.iter().map(|&v| v as u8).collect();
    base64_decode_impl(&bytes, output, is_urlsafe)
}

fn base64_decode_lenient_impl(input: &[u8], output: &mut [u8]) -> SIMDUTFResult {
    let mut acc: u32 = 0;
    let mut bits: i32 = 0;
    let mut written = 0;
    for &byte in input {
        if byte == b'=' { break; }
        let v_std = b64_decode_value(byte, false);
        let v_url = b64_decode_value(byte, true);
        let v = match (v_std, v_url) {
            (Some(v), _) | (_, Some(v)) => Some(v),
            _ => None,
        };
        match v {
            Some(v) => {
                acc = (acc << 6) | (v as u32);
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    if written >= output.len() {
                        return SIMDUTFResult { status: Status::OUTPUT_BUFFER_TOO_SMALL, count: written };
                    }
                    output[written] = ((acc >> bits) & 0xFF) as u8;
                    written += 1;
                }
            }
            None => continue,
        }
    }
    SIMDUTFResult { status: Status::SUCCESS, count: written }
}

fn base64_length_from_binary_impl(length: usize, is_urlsafe: bool) -> usize {
    if is_urlsafe {
        (length * 4 + 2) / 3
    } else {
        ((length + 2) / 3) * 4
    }
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__base64_encode(
    input: *const u8, length: usize, output: *mut u8, is_urlsafe: c_int,
) -> usize {
    if length == 0 { return 0; }
    let inp = unsafe { core::slice::from_raw_parts(input, length) };
    let max_out = base64_length_from_binary_impl(length, is_urlsafe != 0) + 4;
    let out = unsafe { core::slice::from_raw_parts_mut(output, max_out) };
    base64_encode_impl(inp, out, is_urlsafe != 0)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__base64_decode_from_binary(
    input: *const u8, length: usize, output: *mut u8, outlen: usize, is_urlsafe: c_int,
) -> SIMDUTFResult {
    if length == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let inp = unsafe { core::slice::from_raw_parts(input, length) };
    let out = unsafe { core::slice::from_raw_parts_mut(output, outlen) };
    base64_decode_impl(inp, out, is_urlsafe != 0)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__base64_decode_from_binary16(
    input: *const u16, length: usize, output: *mut u8, outlen: usize, is_urlsafe: c_int,
) -> SIMDUTFResult {
    if length == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let inp = unsafe { core::slice::from_raw_parts(input, length) };
    let out = unsafe { core::slice::from_raw_parts_mut(output, outlen) };
    base64_decode16_impl(inp, out, is_urlsafe != 0)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__base64_decode_from_binary_lenient(
    input: *const u8, length: usize, output: *mut u8, outlen: usize,
) -> SIMDUTFResult {
    if length == 0 { return SIMDUTFResult { status: Status::SUCCESS, count: 0 }; }
    let inp = unsafe { core::slice::from_raw_parts(input, length) };
    let out = unsafe { core::slice::from_raw_parts_mut(output, outlen) };
    base64_decode_lenient_impl(inp, out)
}

#[no_mangle]
pub unsafe extern "C" fn simdutf__base64_length_from_binary(length: usize, options: c_int) -> usize {
    base64_length_from_binary_impl(length, options != 0)
}

// ---------------------------------------------------------------------------
// Safe wrapper modules — preserved for downstream compatibility
// ---------------------------------------------------------------------------

pub mod validate {
    use super::*;

    pub mod with_errors {
        use super::*;

        pub fn utf8(input: &[u8]) -> SIMDUTFResult {
            validate_utf8_with_errors_impl(input)
        }
        pub fn ascii(input: &[u8]) -> SIMDUTFResult {
            validate_ascii_with_errors_impl(input)
        }
        pub fn utf16le(input: &[u16]) -> SIMDUTFResult {
            validate_utf16le_with_errors_impl(input)
        }
        pub fn utf16be(input: &[u16]) -> SIMDUTFResult {
            validate_utf16be_with_errors_impl(input)
        }
    }

    pub fn utf8(input: &[u8]) -> bool {
        validate_utf8_impl(input)
    }
    pub fn ascii(input: &[u8]) -> bool {
        validate_ascii_impl(input)
    }
    pub fn utf16le(input: &[u16]) -> bool {
        validate_utf16le_impl(input)
    }
    pub fn utf16be(input: &[u16]) -> bool {
        validate_utf16be_impl(input)
    }
}

pub mod convert {
    use super::*;

    pub mod latin1 {
        use super::*;
        pub mod to {
            use super::*;
            pub fn utf8(input: &[u8], output: &mut [u8]) -> usize {
                convert_latin1_to_utf8_impl(input, output)
            }
        }
    }

    pub mod utf8 {
        use super::*;
        pub mod to {
            use super::*;
            pub mod utf16 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u8], output: &mut [u16]) -> SIMDUTFResult {
                        convert_utf8_to_utf16le_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u8], output: &mut [u16]) -> SIMDUTFResult {
                        convert_utf8_to_utf16be_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u8], output: &mut [u16]) -> usize {
                    convert_utf8_to_utf16le_impl(input, output)
                }
                pub fn be(input: &[u8], output: &mut [u16]) -> usize {
                    convert_utf8_to_utf16be_impl(input, output)
                }
            }

            pub mod utf32 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u8], output: &mut [u32]) -> SIMDUTFResult {
                        convert_utf8_to_utf32_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u8], output: &mut [u32]) -> SIMDUTFResult {
                        convert_utf8_to_utf32_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u8], output: &mut [u32]) -> usize {
                    convert_utf8_to_utf32_impl(input, output)
                }
                pub fn be(input: &[u8], output: &mut [u32]) -> usize {
                    convert_utf8_to_utf32_impl(input, output)
                }
            }
        }
    }

    pub mod utf16 {
        use super::*;
        pub mod to {
            use super::*;
            pub mod utf8 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u16], output: &mut [u8]) -> SIMDUTFResult {
                        convert_utf16le_to_utf8_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u16], output: &mut [u8]) -> SIMDUTFResult {
                        convert_utf16be_to_utf8_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u16], output: &mut [u8]) -> usize {
                    convert_utf16le_to_utf8_impl(input, output)
                }
                pub fn be(input: &[u16], output: &mut [u8]) -> usize {
                    convert_utf16be_to_utf8_impl(input, output)
                }
            }

            pub mod utf32 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u16], output: &mut [u32]) -> SIMDUTFResult {
                        convert_utf16le_to_utf32_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u16], output: &mut [u32]) -> SIMDUTFResult {
                        convert_utf16be_to_utf32_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u16], output: &mut [u32]) -> usize {
                    convert_utf16le_to_utf32_impl(input, output)
                }
                pub fn be(input: &[u16], output: &mut [u32]) -> usize {
                    convert_utf16be_to_utf32_impl(input, output)
                }
            }
        }
    }

    pub mod utf32 {
        use super::*;
        pub mod to {
            use super::*;
            pub mod utf8 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u32], output: &mut [u8]) -> SIMDUTFResult {
                        convert_utf32_to_utf8_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u32], output: &mut [u8]) -> SIMDUTFResult {
                        convert_utf32_to_utf8_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u32], output: &mut [u8]) -> usize {
                    convert_utf32_to_utf8_impl(input, output)
                }
                pub fn be(input: &[u32], output: &mut [u8]) -> usize {
                    convert_utf32_to_utf8_impl(input, output)
                }
            }

            pub mod utf16 {
                use super::*;
                pub mod with_errors {
                    use super::*;
                    pub fn le(input: &[u32], output: &mut [u16]) -> SIMDUTFResult {
                        convert_utf32_to_utf16le_with_errors_impl(input, output)
                    }
                    pub fn be(input: &[u32], output: &mut [u16]) -> SIMDUTFResult {
                        convert_utf32_to_utf16be_with_errors_impl(input, output)
                    }
                }

                pub fn le(input: &[u32], output: &mut [u16]) -> usize {
                    convert_utf32_to_utf16le_impl(input, output)
                }
                pub fn be(input: &[u32], output: &mut [u16]) -> usize {
                    convert_utf32_to_utf16be_impl(input, output)
                }
            }
        }
    }
}

pub mod length {
    use super::*;

    pub mod utf8 {
        use super::*;
        pub mod from {
            use super::*;
            pub mod utf16 {
                use super::*;
                pub fn le(input: &[u16]) -> usize {
                    utf8_length_from_utf16le_impl(input)
                }
                pub fn be(input: &[u16]) -> usize {
                    let le: Vec<u16> = input.iter().map(|&v| u16::from_be(v)).collect();
                    utf8_length_from_utf16le_impl(&le)
                }
            }

            pub fn latin1(input: &[u8]) -> usize {
                utf8_length_from_latin1_impl(input)
            }

            pub fn utf32(input: &[u32]) -> usize {
                utf8_length_from_utf32_impl(input)
            }
        }
    }

    pub mod utf16 {
        use super::*;
        pub mod from {
            use super::*;
            pub fn utf8(input: &[u8]) -> usize {
                utf16_length_from_utf8_impl(input)
            }

            pub fn utf32(input: &[u32]) -> usize {
                utf16_length_from_utf32_impl(input)
            }

            pub fn latin1(input: &[u8]) -> usize {
                input.len()
            }
        }
    }

    pub mod utf32 {
        use super::*;
        pub mod from {
            use super::*;
            pub mod utf8 {
                use super::*;
                pub fn le(input: &[u8]) -> usize {
                    utf32_length_from_utf8_impl(input)
                }
                pub fn be(input: &[u8]) -> usize {
                    utf32_length_from_utf8_impl(input)
                }
            }

            pub mod utf16 {
                use super::*;
                pub fn le(input: &[u16]) -> usize {
                    utf32_length_from_utf16le_impl(input)
                }
                pub fn be(input: &[u16]) -> usize {
                    let le: Vec<u16> = input.iter().map(|&v| u16::from_be(v)).collect();
                    utf32_length_from_utf16le_impl(&le)
                }
            }
        }
    }
}

pub mod trim {
    pub(crate) fn utf8_len(buf: &[u8]) -> usize {
        let len = buf.len();

        if len < 3 {
            match len {
                2 => {
                    if buf[len - 1] >= 0b11000000 {
                        return len - 1;
                    }
                    if buf[len - 2] >= 0b11100000 {
                        return len - 2;
                    }
                    return len;
                }
                1 => {
                    if buf[len - 1] >= 0b11000000 {
                        return len - 1;
                    }
                    return len;
                }
                0 => return len,
                _ => unreachable!(),
            }
        }

        if buf[len - 1] >= 0b11000000 {
            return len - 1;
        }
        if buf[len - 2] >= 0b11100000 {
            return len - 2;
        }
        if buf[len - 3] >= 0b11110000 {
            return len - 3;
        }
        len
    }

    pub(crate) fn utf16_len(buf: &[u16]) -> usize {
        let len = buf.len();

        if len == 0 {
            return 0;
        }
        if (buf[len - 1] >= 0xD800) && (buf[len - 1] <= 0xDBFF) {
            return len - 1;
        }
        len
    }

    pub fn utf16(buf: &[u16]) -> &[u16] {
        &buf[0..utf16_len(buf)]
    }

    pub fn utf8(buf: &[u8]) -> &[u8] {
        &buf[0..utf8_len(buf)]
    }
}

pub mod base64 {
    use super::{SIMDUTFResult, base64_encode_impl, base64_decode_impl, base64_decode16_impl, base64_decode_lenient_impl, base64_length_from_binary_impl};

    pub fn encode(input: &[u8], output: &mut [u8], is_urlsafe: bool) -> usize {
        base64_encode_impl(input, output, is_urlsafe)
    }

    pub unsafe fn encode_raw(input: &[u8], output: *mut u8, is_urlsafe: bool) -> usize {
        let max_out = base64_length_from_binary_impl(input.len(), is_urlsafe) + 4;
        let out = unsafe { core::slice::from_raw_parts_mut(output, max_out) };
        base64_encode_impl(input, out, is_urlsafe)
    }

    pub fn encode_len(input: usize, is_urlsafe: bool) -> usize {
        base64_length_from_binary_impl(input, is_urlsafe)
    }

    pub fn decode(input: &[u8], output: &mut [u8], is_urlsafe: bool) -> SIMDUTFResult {
        base64_decode_impl(input, output, is_urlsafe)
    }

    pub fn decode16(input: &[u16], output: &mut [u8], is_urlsafe: bool) -> SIMDUTFResult {
        base64_decode16_impl(input, output, is_urlsafe)
    }

    pub fn decode_lenient(input: &[u8], output: &mut [u8]) -> SIMDUTFResult {
        base64_decode_lenient_impl(input, output)
    }
}
