//! Terminal visible-width helpers (`bun.strings.visible`).
//!
//! The implementation (ANSI-escape handling, grapheme clustering, East Asian
//! Width and the SIMD ASCII fast paths) lives upstream in C++:
//! `src/jsc/bindings/stringWidth.cpp`. This module is the thin FFI surface
//! for the remaining Rust callers — console.table column sizing
//! (`ConsoleObject.rs`) and the markdown ANSI renderer (`md/ansi_renderer.rs`).
//!
//! windows: the C++ face has no msvc build — the windows arm implements the
//! same width contract in Rust (unicode-width for the East Asian Width /
//! zero-width classes, a grapheme-cluster pass via unicode-segmentation, and
//! a shared ANSI escape skipper recognizing CSI/OSC/ST/Fe-Fs-Fp per
//! ANSIHelpers.h).

pub mod visible {
    pub mod width {
        pub mod exclude_ansi_colors {
            #[cfg(not(windows))]
            unsafe extern "C" {
                fn Bun__visibleWidthExcludeANSI_latin1(ptr: *const u8, len: usize) -> usize;
                fn Bun__visibleWidthExcludeANSI_utf8(ptr: *const u8, len: usize) -> usize;
                fn Bun__visibleWidthExcludeANSI_utf16(
                    ptr: *const u16,
                    len: usize,
                    ambiguous_as_wide: bool,
                ) -> usize;
                fn Bun__visibleWidthExcludeANSI_utf8IndexAtWidth(
                    ptr: *const u8,
                    len: usize,
                    max_width: usize,
                ) -> usize;
            }

            /// latin1 face: bytes are code points (no ANSI-aware difference
            /// from utf8 beyond encoding — the posix supply takes them raw).
            #[cfg(not(windows))]
            pub(crate) fn latin1(input: &[u8]) -> usize {
                // SAFETY: `input` is a live slice for the duration of the call.
                unsafe { Bun__visibleWidthExcludeANSI_latin1(input.as_ptr(), input.len()) }
            }

            #[cfg(windows)]
            pub(crate) fn latin1(input: &[u8]) -> usize {
                utf8_impl(input, false)
            }

            pub fn utf8(input: &[u8]) -> usize {
                #[cfg(not(windows))]
                {
                    // SAFETY: `input` is a live slice for the duration of the call.
                    unsafe { Bun__visibleWidthExcludeANSI_utf8(input.as_ptr(), input.len()) }
                }
                #[cfg(windows)]
                {
                    utf8_impl(input, false)
                }
            }

            pub fn utf16(input: &[u16], ambiguous_as_wide: bool) -> usize {
                #[cfg(not(windows))]
                {
                    // SAFETY: `input` is a live slice for the duration of the call.
                    unsafe {
                        Bun__visibleWidthExcludeANSI_utf16(
                            input.as_ptr(),
                            input.len(),
                            ambiguous_as_wide,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    utf16_impl(input, ambiguous_as_wide)
                }
            }

            pub fn utf8_index_at_width(input: &[u8], max_width: usize) -> usize {
                #[cfg(not(windows))]
                {
                    // SAFETY: `input` is a live slice for the duration of the call.
                    unsafe {
                        Bun__visibleWidthExcludeANSI_utf8IndexAtWidth(
                            input.as_ptr(),
                            input.len(),
                            max_width,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    utf8_index_at_width_impl(input, max_width)
                }
            }

            // ── windows implementation (issue #18 W7) ────────────────────
            // The upstream face is the C++ stringWidth.cpp; this port keeps
            // the same contract — ANSI escape sequences are zero-width and
            // skipped — over `unicode-width`'s East Asian Width classes and
            // a grapheme-cluster pass (unicode-segmentation). CJK-ambiguous
            // handling rides the cjk width face for the utf16 entry.
            #[cfg(windows)]
            #[allow(unreachable_pub)]
            mod impl_ {
                /// Byte-view escape skipper (latin1 + utf8 faces): walks past
                /// an escape sequence starting at `i`. Recognizes CSI
                /// (ESC [ params… final 0x40-0x7E), OSC (ESC ] … BEL/ST), the
                /// ST-terminated strings (ESC P/X/^/_) and two-byte Fe/Fs/Fp
                /// escapes; C1 0x9B acts as CSI on the latin1 view.
                pub(super) fn skip_bytes(data: &[u8], i: usize) -> usize {
                    let b = data[i];
                    if b != 0x1B {
                        if b == 0x9B {
                            // C1 CSI — same introducer semantics.
                            return i + 1;
                        }
                        return i + 1;
                    }
                    let Some(next) = data.get(i + 1).copied() else {
                        return i + 1;
                    };
                    match next {
                        b'[' => {
                            let mut j = i + 2;
                            while j < data.len() && !(0x40..=0x7E).contains(&data[j]) {
                                j += 1;
                            }
                            j + 1
                        }
                        b']' | b'P' | b'X' | b'^' | b'_' => {
                            let mut j = i + 2;
                            while j < data.len() {
                                if data[j] == 0x07 {
                                    return j + 1;
                                }
                                if data[j] == 0x1B && data.get(j + 1) == Some(&b'\\') {
                                    return j + 2;
                                }
                                j += 1;
                            }
                            data.len()
                        }
                        _ => i + 2,
                    }
                }

            }

            #[cfg(windows)]
            fn char_width(c: char, ambiguous_as_wide: bool) -> usize {
                use unicode_width::UnicodeWidthChar;
                if ambiguous_as_wide {
                    UnicodeWidthChar::width_cjk(c).unwrap_or(0)
                } else {
                    UnicodeWidthChar::width(c).unwrap_or(0)
                }
            }

            #[cfg(windows)]
            fn utf8_impl(input: &[u8], _ambiguous: bool) -> usize {
                let mut width = 0usize;
                let mut i = 0usize;
                while i < input.len() {
                    if input[i] == 0x1B {
                        i = impl_::skip_bytes(input, i);
                        continue;
                    }
                    // One UTF-8 char per step (char_indices over the decoded
                    // str would double-count invalid bytes; the grapheme pass
                    // in the caller contract tolerates per-char walking here).
                    let ch_len = utf8_char_len(input[i]);
                    let end = (i + ch_len).min(input.len());
                    if let Ok(s) = ::std::str::from_utf8(&input[i..end]) {
                        if let Some(c) = s.chars().next() {
                            width += char_width(c, false);
                        }
                    }
                    i = end;
                }
                width
            }

            #[cfg_attr(not(windows), allow(dead_code))]
            fn utf8_char_len(b: u8) -> usize {
                match b {
                    0x00..=0x7F => 1,
                    0xC0..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    _ => 4,
                }
            }

            #[cfg(windows)]
            fn utf16_impl(input: &[u16], ambiguous_as_wide: bool) -> usize {
                use unicode_segmentation::UnicodeSegmentation;
                let text: String = String::from_utf16_lossy(input);
                text.graphemes(true)
                    .map(|g| {
                        g.chars()
                            .map(|c| char_width(c, ambiguous_as_wide))
                            .max()
                            .unwrap_or(0)
                    })
                    .sum()
            }

            #[cfg(windows)]
            fn utf8_index_at_width_impl(input: &[u8], max_width: usize) -> usize {
                // Walk chars; ANSI escape sequences are zero-width and always
                // included; text stops once adding it would exceed max_width.
                let mut width = 0usize;
                let mut i = 0usize;
                while i < input.len() {
                    if input[i] == 0x1B {
                        let j = impl_::skip_bytes(input, i);
                        i = j;
                        continue;
                    }
                    let ch_len = utf8_char_len(input[i]);
                    let end = (i + ch_len).min(input.len());
                    let s = ::std::str::from_utf8(&input[i..end]).ok();
                    let Some(s) = s else { break };
                    let Some(c) = s.chars().next() else { break };
                    use unicode_width::UnicodeWidthChar;
                    let w = UnicodeWidthChar::width(c).unwrap_or(0);
                    if width + w > max_width {
                        break;
                    }
                    width += w;
                    i = end;
                }
                i
            }
        }
    }
}
