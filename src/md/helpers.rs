//! Shared helpers for markdown renderers.
//!
//! Contains heading-ID tracking, entity decoding, and slug generation
//! used by the HTML renderer, ANSI renderer, and React/JS callback renderers.

use super::entity as entity_mod;
use super::types::TextType;

/// Encode a Unicode codepoint as UTF-8.
pub(crate) fn encode_utf8(codepoint: u32, buf: &mut [u8; 4]) -> u8 {
    bun_core::strings::encode_wtf8_rune(buf, codepoint) as u8
}

/// Case-insensitive ASCII comparison.
pub use bun_core::strings::eql_case_insensitive_ascii_check_length as ascii_case_eql;

/// Parse a numeric character reference (&#DDD; or &#xHHH;) and return the codepoint.
pub fn parse_entity_codepoint(entity_text: &[u8]) -> Option<u32> {
    if entity_text.len() < 4 || entity_text[0] != b'&' || entity_text[1] != b'#' {
        return None;
    }
    let mut cp: u32 = 0;
    if entity_text[2] == b'x' || entity_text[2] == b'X' {
        for &ec in &entity_text[3..] {
            if ec == b';' {
                break;
            }
            cp = cp
                .wrapping_mul(16)
                .wrapping_add(u32::from(bun_core::fmt::hex_digit_value(ec).unwrap_or(0)));
        }
    } else {
        for &ec in &entity_text[2..] {
            if ec == b';' {
                break;
            }
            cp = cp.wrapping_mul(10).wrapping_add((ec - b'0') as u32);
        }
    }
    if cp == 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) {
        cp = 0xFFFD;
    }
    Some(cp)
}

/// Decode an HTML entity to raw UTF-8 bytes.
/// Returns decoded bytes as a slice of `out`, or None for unknown entities.
pub fn decode_entity_to_utf8<'a>(entity_text: &[u8], out: &'a mut [u8; 8]) -> Option<&'a [u8]> {
    if let Some(cp) = parse_entity_codepoint(entity_text) {
        let len = encode_utf8(
            cp,
            (&mut out[0..4])
                .try_into()
                .expect("infallible: size matches"),
        );
        return Some(&out[..len as usize]);
    }
    if let Some(codepoints) = entity_mod::lookup(entity_text) {
        let len1 = encode_utf8(
            codepoints[0],
            (&mut out[0..4])
                .try_into()
                .expect("infallible: size matches"),
        ) as usize;
        if codepoints[1] != 0 {
            let mut tmp: [u8; 4] = [0; 4];
            let len2 = encode_utf8(codepoints[1], &mut tmp) as usize;
            out[len1..][..len2].copy_from_slice(&tmp[..len2]);
            return Some(&out[..len1 + len2]);
        }
        return Some(&out[..len1]);
    }
    None
}

/// Generate a GitHub-compatible slug from text content.
/// Modifies text_buf in-place. Uses slug_counts for -N deduplication.
pub fn generate_slug<'a>(
    text_buf: &'a mut Vec<u8>,
    slug_counts: &mut bun_collections::StringHashMap<u32>,
) -> &'a [u8] {
    let text_len = text_buf.len();
    let mut out_len: usize = 0;
    let mut prev_hyphen: bool = true;

    for idx in 0..text_len {
        let c = text_buf[idx];
        if c >= b'A' && c <= b'Z' {
            text_buf[out_len] = c + 32;
            out_len += 1;
            prev_hyphen = false;
        } else if (c >= b'a' && c <= b'z') || (c >= b'0' && c <= b'9') {
            text_buf[out_len] = c;
            out_len += 1;
            prev_hyphen = false;
        } else if c == b'-' || c == b' ' {
            if !prev_hyphen {
                text_buf[out_len] = b'-';
                out_len += 1;
                prev_hyphen = true;
            }
        }
    }

    while out_len > 0 && text_buf[out_len - 1] == b'-' {
        out_len -= 1;
    }

    if let Some(value) = slug_counts.get_mut(&text_buf[..out_len]) {
        let count = *value + 1;
        *value = count;
        text_buf.truncate(out_len);
        text_buf.push(b'-');

        let mut dec_buf = bun_core::fmt::ItoaBuf::new();
        text_buf.extend_from_slice(bun_core::fmt::itoa(&mut dec_buf, count));
        return text_buf.as_slice();
    }

    slug_counts.put_assume_capacity(&text_buf[..out_len], 0);
    &text_buf[..out_len]
}

/// Shared heading-ID state used by all renderers (HTML, React AST, JS callbacks).
/// Tracks whether we're inside a heading, accumulates text for slug generation,
/// and deduplicates slugs via a count map.
#[derive(Default)]
pub struct HeadingIdTracker {
    pub enabled: bool,
    pub in_heading: bool,
    pub text_buf: Vec<u8>,
    pub slug_counts: bun_collections::StringHashMap<u32>,
}

impl HeadingIdTracker {
    pub fn init(enabled: bool) -> HeadingIdTracker {
        HeadingIdTracker {
            enabled,
            ..Default::default()
        }
    }

    /// Call on entering a heading block.
    pub fn enter_heading(&mut self) {
        if self.enabled {
            self.in_heading = true;
        }
    }

    /// Call from text callback to accumulate text for slug.
    /// No-op if not inside a heading or disabled.
    pub fn track_text(&mut self, text_type: TextType, content: &[u8]) {
        if !self.in_heading {
            return;
        }
        match text_type {
            TextType::NullChar => self.text_buf.extend_from_slice(b"\xEF\xBF\xBD"),
            TextType::Br | TextType::Softbr => self.text_buf.extend_from_slice(b" "),
            TextType::Html => {}
            TextType::Entity => {
                let mut buf: [u8; 8] = [0; 8];
                match decode_entity_to_utf8(content, &mut buf) {
                    Some(decoded) => self.text_buf.extend_from_slice(decoded),
                    None => self.text_buf.extend_from_slice(content),
                }
            }
            _ => self.text_buf.extend_from_slice(content),
        }
    }

    /// Call on leaving a heading block. Returns slug (valid until clear_after_heading).
    pub fn leave_heading(&mut self) -> Option<&[u8]> {
        if !self.enabled {
            return None;
        }
        self.in_heading = false;
        Some(generate_slug(&mut self.text_buf, &mut self.slug_counts))
    }

    /// Call after using the slug to reset text buffer.
    pub fn clear_after_heading(&mut self) {
        self.text_buf.clear();
    }
}
