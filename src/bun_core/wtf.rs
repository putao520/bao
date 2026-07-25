//! `bun_core::wtf` — pure-Rust replacements for linked WTF (WebKit) utilities.
//!
//! Historically these called into `src/jsc/bindings/wtf-bindings.cpp`. Bao's
//! product path has no WebKit, so the parsers live here as pure Rust and the
//! residual `WTF__*` `#[no_mangle]` symbols in `bun_runtime::product_native_symbols`
//! forward into these functions for any remaining link-time declarers.
//!
//! @trace STUB-INVENTORY: pure-Rust owner for `WTF__parseES5Date` / parseDouble re-exports

/// Direct ES5 Date Time String parse. Returns NaN for any rejected input.
/// `s` is treated as Latin-1 / ASCII (digits and separators only).
///
/// Covers the ES5 simplified ISO-8601 forms used by npm metadata and YAML:
/// - `YYYY-MM-DDTHH:mm:ss.sssZ`
/// - `YYYY-MM-DDTHH:mm:ssZ`
/// - `YYYY-MM-DDTHH:mm:ss±HH:mm`
/// - `YYYY-MM-DD`
/// - `YYYY-MM-DDTHH:mm:ss` (treated as UTC, matching the C shim that discarded
///   `isLocalTime`)
#[inline]
pub fn parse_es5_date_raw(s: &[u8]) -> f64 {
    match parse_es5_date_ms(s) {
        Some(ms) => ms as f64,
        None => f64::NAN,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDate;

impl core::fmt::Display for InvalidDate {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("InvalidDate")
    }
}
impl core::error::Error for InvalidDate {}

impl From<InvalidDate> for crate::Error {
    fn from(_: InvalidDate) -> Self {
        crate::Error::from_name("InvalidDate")
    }
}

/// `bun.jsc.wtf.parseES5Date` shape — `Err` on empty input or non-finite result.
/// `2000-01-01T00:00:00.000Z` → `Ok(946684800000.0)`.
pub fn parse_es5_date(buf: &[u8]) -> Result<f64, InvalidDate> {
    if buf.is_empty() {
        return Err(InvalidDate);
    }
    let ms = parse_es5_date_raw(buf);
    if ms.is_finite() {
        Ok(ms)
    } else {
        Err(InvalidDate)
    }
}

// `WTF::parseDouble` — re-exported from the merged `string::wtf` module so
// `bun_core::wtf::parse_double` (formerly `bun_core::wtf::parse_double`)
// resolves unchanged.
pub use crate::string::wtf::{
    InvalidCharacter, RefPtr, StringImpl, WTFString, WTFStringImpl, WTFStringImplExt,
    WTFStringImplStruct, parse_double,
};

// ── ES5 date internals ────────────────────────────────────────────────────

fn parse_es5_date_ms(s: &[u8]) -> Option<i64> {
    // Trim ASCII whitespace.
    let mut b = s;
    while let Some((&c, rest)) = b.split_first() {
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((&c, rest)) = b.split_last() {
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            b = rest;
        } else {
            break;
        }
    }
    if b.len() < 4 {
        return None;
    }

    let year = parse_n_digits(b, 0, 4)? as i32;
    let mut i = 4;
    let (month, day) = if i < b.len() && b[i] == b'-' {
        i += 1;
        let m = parse_n_digits(b, i, 2)? as u32;
        i += 2;
        if i < b.len() && b[i] == b'-' {
            i += 1;
            let d = parse_n_digits(b, i, 2)? as u32;
            i += 2;
            (m, d)
        } else {
            (m, 1)
        }
    } else {
        (1, 1)
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut hour = 0u32;
    let mut min = 0u32;
    let mut sec = 0u32;
    let mut millis = 0u32;

    if i < b.len() && (b[i] == b'T' || b[i] == b' ' || b[i] == b't') {
        i += 1;
        hour = parse_n_digits(b, i, 2)? as u32;
        i += 2;
        if i < b.len() && b[i] == b':' {
            i += 1;
            min = parse_n_digits(b, i, 2)? as u32;
            i += 2;
        }
        if i < b.len() && b[i] == b':' {
            i += 1;
            sec = parse_n_digits(b, i, 2)? as u32;
            i += 2;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let frac = &b[start..i];
            if frac.is_empty() {
                return None;
            }
            // Take up to 3 digits; truncate extra / pad short (ES5).
            millis = scale_frac_millis(frac);
        }
    }

    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }

    // Timezone: Z / z / ±HH:mm / ±HHmm / empty (= UTC, matching C shim).
    let mut tz_min_offset: i32 = 0;
    if i < b.len() {
        match b[i] {
            b'Z' | b'z' => {
                i += 1;
            }
            b'+' | b'-' => {
                let sign = if b[i] == b'-' { -1 } else { 1 };
                i += 1;
                let th = parse_n_digits(b, i, 2)? as i32;
                i += 2;
                let mut tm = 0i32;
                if i < b.len() && b[i] == b':' {
                    i += 1;
                    tm = parse_n_digits(b, i, 2)? as i32;
                    i += 2;
                } else if i + 2 <= b.len() && b[i].is_ascii_digit() {
                    tm = parse_n_digits(b, i, 2)? as i32;
                    i += 2;
                }
                if th > 23 || tm > 59 {
                    return None;
                }
                tz_min_offset = sign * (th * 60 + tm);
            }
            _ => return None,
        }
    }
    // Trailing junk?
    if i != b.len() {
        return None;
    }

    let day_ms = days_from_civil(year, month, day)? * 86_400_000;
    let time_ms = (hour as i64) * 3_600_000
        + (min as i64) * 60_000
        + (sec as i64) * 1_000
        + (millis as i64);
    // Offset: local = UTC + offset ⇒ UTC = local - offset.
    Some(day_ms + time_ms - (tz_min_offset as i64) * 60_000)
}

fn scale_frac_millis(frac: &[u8]) -> u32 {
    let mut acc = 0u32;
    for &c in frac.iter().take(3) {
        acc = acc * 10 + (c - b'0') as u32;
    }
    for _ in frac.len().min(3)..3 {
        acc *= 10;
    }
    acc
}

fn parse_n_digits(b: &[u8], start: usize, n: usize) -> Option<u32> {
    if start + n > b.len() {
        return None;
    }
    let mut v = 0u32;
    for k in 0..n {
        let c = b[start + k];
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u32;
    }
    Some(v)
}

/// Howard Hinnant civil_from_days inverse: days since Unix epoch (1970-01-01).
fn days_from_civil(mut y: i32, m: u32, d: u32) -> Option<i64> {
    if m == 0 || m > 12 || d == 0 || d > 31 {
        return None;
    }
    // Shift so March is month 0 (Hinnant).
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 {
        y
    } else {
        y - 399
    } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    // Unix epoch is 1970-01-01 = days from civil algorithm epoch (0000-03-01).
    let days = (era as i64) * 146097 + doe as i64 - 719468;
    Some(days)
}

// ported from: src/jsc/WTF.zig
