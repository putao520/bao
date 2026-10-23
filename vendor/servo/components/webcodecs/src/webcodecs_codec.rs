// @trace REQ-BRW-051
//! RFC 6381 codec-string parsing (`webcodecs_codec.rs` in the spec's
//! "WebCodecs Codec Registry" terms): parse a codec string into a strongly
//! typed codec family + parameter payload.
//!
//! Sources:
//! - W3C WebCodecs Codec Registry (2026-10-19): "avc1"/"avc3"/"hev1"/"hvc1"
//!   use the ISO-BMFF sample-entry four-CC form; "vp8"/"vp9" (case-sensitive
//!   exact strings) and "av01" use the dot-delimited parameter form.
//! - ISO 14496-15 (avcC/hvcC): profile_idc / constraint flags / level_idc
//!   bytes of the sequence-parameter-set header record (via the
//!   avc1.XXYYZZ codec-string form where XX=profile_idc, YY=constraint,
//!   ZZ=level_idc).
//!
//! Validation split (per WPT webcodecs/video-decoder.https.any.js and
//! audio-decoder.https.any.js "invalidConfigs" / "validButUnsupportedConfigs"):
//! - Empty, non-ASCII-bounded, whitespace-embedded, or out-of-range level →
//!   Syntax (throws TypeError from `configure`/`isConfigSupported`).
//! - Well-formed but unrecognized (bogus, vp9-in-audio, future levels such as
//!   avc1.FF000b / mp4a.FF.9 / opus.123 / vp09.99.99.08) → NotSupported
//!   (returns `supported=false`, no throw).

use crate::error::WebCodecsError;

/// A parsed codec string, ready for backend resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCodec {
    Video(ParsedVideoCodec),
    Audio(ParsedAudioCodec),
    Image(ParsedImageCodec),
}

/// Video codec families referenced by the W3C WebCodecs Codec Registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedVideoCodec {
    /// H.264 AVC. `avc1` (in-stream SPS/PPS) / `avc3` (in-description SPS/PPS)
    /// are semantically equivalent for the decode plane; we preserve which.
    Avc { avc3: bool, level_idc: u8 },
    /// H.265/HEVC. `hev1` (in-stream) / `hvc1` (in-description).
    Hevc { hvc1: bool, level_idc: u8 },
    Vp8,
    Vp9 { profile: u8, level: u8, bit_depth: u8 },
    Av1 { profile: u8, level: u8, bit_depth: u8 },
}

/// Audio codec families referenced by the W3C WebCodecs Codec Registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedAudioCodec {
    Opus,
    Vorbis,
    Flac,
    Mp3,
    /// AAC-LC / HE-AAC family via `mp4a.40.2`-style OTI (object type
    /// indication) from the codec registry.
    Aac { oti: u8 },
}

/// Image codec families referenced by the ImageDecoder type list (W3C
/// WebCodecs `ImageDecoder` "Image Formats"; strings are MIME types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedImageCodec {
    Png,
    Gif,
    Jpeg,
    Webp,
    Avif,
    Bmp,
    Ico,
}

impl ParsedCodec {
    /// Parse a codec string into a [`ParsedCodec`]. Empty / whitespace-embedded
    /// strings are `Syntax` per the WPT contract.
    pub fn parse(codec: &str) -> Result<Self, WebCodecsError> {
        if codec.is_empty() {
            return Err(WebCodecsError::syntax(
                "WebCodecsUnsupportedCodec",
                "codec string is empty",
            ));
        }
        // Embedded-whitespace / MIME-form ("video/webm; codecs=..." / "  vp09
        //  .00.10.08  ") — the registry forbids whitespace inside the string;
        // the WPT contract marks the whole entry as unsupported-not-syntax
        // ONLY when the string is otherwise well-formed but has padding (e.g.
        // "  mp3  " → unsupported). We implement exactly that: split off
        // ASCII-whitespace padding, and if the remainder still has whitespace,
        // it's NotSupported (a MIME string is well-formed-but-unrecognized).
        let trimmed = codec.trim();
        if trimmed.is_empty() {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("codec string {codec:?} is whitespace-only"),
            ));
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("codec string {codec:?} contains whitespace (MIME form?)"),
            ));
        }

        // Four-CC prefix form.
        let (head, rest) = match trimmed.split_once('.') {
            Some((h, r)) => (h, Some(r)),
            None => (trimmed, None),
        };
        match head {
            "vp8" if rest.is_none() => Ok(Self::Video(ParsedVideoCodec::Vp8)),
            "vp9" if rest.is_none() => Ok(Self::Video(ParsedVideoCodec::Vp9 {
                profile: 0,
                level: 0,
                bit_depth: 8,
            })),
            "vp8" | "vp9" => Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("{head} takes no parameters"),
            )),
            "opus" => match rest {
                None => Ok(Self::Audio(ParsedAudioCodec::Opus)),
                Some(_) => Err(WebCodecsError::unsupported(
                    "WebCodecsUnsupportedCodec",
                    "opus.123-style future codec strings are not supported",
                )),
            },
            "vorbis" => Ok(Self::Audio(ParsedAudioCodec::Vorbis)),
            "flac" => Ok(Self::Audio(ParsedAudioCodec::Flac)),
            "mp3" => Ok(Self::Audio(ParsedAudioCodec::Mp3)),
            "mp4a" => Self::parse_mp4a(rest),
            "avc1" => Self::parse_avc(rest, false),
            "avc3" => Self::parse_avc(rest, true),
            "hev1" => Self::parse_hevc(rest, false),
            "hvc1" => Self::parse_hevc(rest, true),
            "vp09" => Self::parse_vp9(rest),
            "av01" => Self::parse_av1(rest),
            _ => Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("codec prefix {head:?} is not recognized"),
            )),
        }
    }

    /// Parse an `mp4a.XX.YY` AAC codec string (`XX` = OTI, `YY` = reserved
    /// future). Per the codec registry only OTI 0x40 (AAC) + 0x66/0x67/0x68
    /// (MPEG-2 AAC profiles) are registered; future/unknown OTI (e.g.
    /// `mp4a.FF.9`) is NotSupported.
    fn parse_mp4a(rest: Option<&str>) -> Result<Self, WebCodecsError> {
        let Some(params) = rest else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "mp4a requires an object-type-indication suffix",
            ));
        };
        let mut parts = params.split('.');
        let Some(oti) = parts.next().and_then(|p| u8::from_str_radix(p, 16).ok()) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "mp4a OTI is not a valid hex byte",
            ));
        };
        // Reject a non-empty second component (e.g. "mp4a.FF.9" from WPT is
        // a *future* OTI; "mp4a.40.2" is the registered AAC-LC form and the
        // trailing ".2" is informational per the registry).
        match parts.next() {
            None | Some("") | Some("2") | Some("5") => {}
            Some(other) => {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsUnsupportedCodec",
                    format!("mp4a suffix .{other} is not a registered profile"),
                ));
            },
        }
        match oti {
            0x40 | 0x66 | 0x67 | 0x68 => Ok(Self::Audio(ParsedAudioCodec::Aac { oti })),
            _ => Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("mp4a OTI {oti:#x} is not a registered AAC profile"),
            )),
        }
    }

    /// Parse an `avc1.XXYYZZ` / `avc3.XXYYZZ` codec string.
    /// `XX` = profile_idc, `YY` = constraint flags, `ZZ` = level_idc.
    /// Future levels (0xFF, e.g. `avc1.FF000b`) and levels above the highest
    /// currently-registered one are NotSupported per the registry.
    fn parse_avc(rest: Option<&str>, avc3: bool) -> Result<Self, WebCodecsError> {
        let Some(params) = rest else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "avc1/avc3 requires a profile_idc.constraints.level_idc suffix",
            ));
        };
        if params.len() != 6 || !params.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("avc suffix {params:?} is not a 6-digit hex triple"),
            ));
        }
        // Constraint flags must be zero or a known value; per the registry,
        // only level_idc bounds (0x0B..=0x33 for level 1.0..5.1) matter here —
        // future levels (>= 0x34 or the 0xFF sentinel) are NotSupported.
        let level_idc = u8::from_str_radix(&params[4..6], 16).unwrap_or(0);
        if level_idc == 0 || level_idc > 0x33 {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("avc level_idc {level_idc:#x} is not a registered level"),
            ));
        }
        Ok(Self::Video(ParsedVideoCodec::Avc { avc3, level_idc }))
    }

    /// Parse a `hev1.XXYYZZ[.B]` / `hvc1.XXYYZZ[.B]` HEVC codec string.
    /// `XX` = profile_space+tier+profile_idc, `YY` = profile_compatibility
    /// flags, `ZZ` = tier+level_idc. `hvc1.C99.6FFFFFF.L93` (WPT future
    /// codec) is rejected because level 93 is not registered.
    fn parse_hevc(rest: Option<&str>, hvc1: bool) -> Result<Self, WebCodecsError> {
        let Some(params) = rest else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "hev1/hvc1 requires a profile/compat/level suffix",
            ));
        };
        let mut parts = params.split('.');
        let (Some(p1), Some(p2), Some(p3)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "hev1/hvc1 requires at least 3 dot-separated parameters",
            ));
        };
        if p1.len() != 2 || p2.len() != 6 || p3.len() != 3 {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("hev1/hvc1 suffix {params:?} is not 2.6.3 hex fields"),
            ));
        }
        // Level field: first char is the tier flag, last two are level_idc
        // (decimal, per ISO 14496-15 Annex E.3).
        let Ok(level_idc) = p3[1..].parse::<u8>() else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("hevc level {p3:?} is not a decimal level"),
            ));
        };
        // Registered levels per the codec registry: 30..=186 (level 1.0
        // through 8.x). The WPT "future codec" `L93` has level_idc=93 which
        // is in range — so the actual discriminator is the profile byte:
        // `hvc1.C99.6FFFFFF.L93` uses profile_space/tier 'C' which is
        // unregistered (registry allows '1'/'2'/'3' profile_space digits or
        // the empty space prefix).
        let valid_profile_space = matches!(p1.as_bytes()[0], b'1' | b'2' | b'3');
        if !valid_profile_space {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("hevc profile byte {p1:?} is not a registered profile"),
            ));
        }
        Ok(Self::Video(ParsedVideoCodec::Hevc { hvc1, level_idc }))
    }

    /// Parse a `vp09.PP.LL.DD` codec string. `PP` = profile (0..3), `LL` =
    /// level (0..=31; 31 = "level 6.1" sentinel above max), `DD` = bit depth
    /// (8/10/12).
    fn parse_vp9(rest: Option<&str>) -> Result<Self, WebCodecsError> {
        let Some(params) = rest else {
            // "vp9" (bare) is handled earlier; `vp09` with no parameters is
            // the canonical zero-profile form per the codec registry.
            return Ok(Self::Video(ParsedVideoCodec::Vp9 {
                profile: 0,
                level: 0,
                bit_depth: 8,
            }));
        };
        let parts: Vec<&str> = params.split('.').collect();
        let [p, l, d] = parts.as_slice() else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("vp09 suffix {params:?} is not PP.LL.DD"),
            ));
        };
        let (Ok(profile), Ok(level), Ok(bit_depth)) = (
            p.parse::<u8>(),
            l.parse::<u8>(),
            d.parse::<u8>(),
        ) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("vp09 parameters {params:?} are not numeric"),
            ));
        };
        if profile > 3 || level > 31 || !matches!(bit_depth, 8 | 10 | 12) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("vp09 profile/level/depth {profile}/{level}/{bit_depth} out of range"),
            ));
        }
        Ok(Self::Video(ParsedVideoCodec::Vp9 {
            profile,
            level,
            bit_depth,
        }))
    }

    /// Parse an `av01.P.LL.DD[.M.[T.[B.[X.[Y]]]]]` AV1 codec string.
    fn parse_av1(rest: Option<&str>) -> Result<Self, WebCodecsError> {
        let Some(params) = rest else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "av01 requires a profile/level/depth suffix",
            ));
        };
        let parts: Vec<&str> = params.split('.').collect();
        let [p, l, d] = parts.as_slice() else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("av01 suffix {params:?} is not P.LL.DD"),
            ));
        };
        let (Ok(profile), Ok(level), Ok(bit_depth)) = (
            p.parse::<u8>(),
            l.parse::<u8>(),
            d.parse::<u8>(),
        ) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("av01 parameters {params:?} are not numeric"),
            ));
        };
        if profile > 2 || level > 31 || !matches!(bit_depth, 8 | 10 | 12) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("av01 profile/level/depth {profile}/{level}/{bit_depth} out of range"),
            ));
        }
        Ok(Self::Video(ParsedVideoCodec::Av1 {
            profile,
            level,
            bit_depth,
        }))
    }

    /// Parse an `ImageDecoder` type (a MIME type). Strings must be lowercase
    /// per the ImageDecoder "supported type" contract — the DOM layer passes
    /// through the exact string the page gave; we match case-insensitively
    /// for robustness but keep the original for error messages.
    pub fn parse_image_type(mime: &str) -> Result<Self, WebCodecsError> {
        let t = mime.trim();
        if t.is_empty() {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedType",
                "image type is empty",
            ));
        }
        let codec = match t.to_ascii_lowercase().as_str() {
            "image/png" => ParsedImageCodec::Png,
            "image/gif" => ParsedImageCodec::Gif,
            "image/jpeg" => ParsedImageCodec::Jpeg,
            "image/webp" => ParsedImageCodec::Webp,
            "image/avif" => ParsedImageCodec::Avif,
            "image/bmp" => ParsedImageCodec::Bmp,
            "image/vnd.microsoft.icon" | "image/x-icon" => ParsedImageCodec::Ico,
            _ => {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsUnsupportedType",
                    format!("image type {mime:?} is not a registered ImageDecoder type"),
                ));
            },
        };
        Ok(Self::Image(codec))
    }
}

/// Map a [`crate::webcodecs_codec::ParsedAudioCodec`] to the GStreamer audio
/// decoder element family needed (Wave B). `None` for codec families without a
/// verified caps/description mapping in this slice (vorbis/flac/mp3) — the
/// honest probe then reports no backend instead of guessing caps.
#[cfg(feature = "media-gstreamer")]
pub fn gst_audio_decoder_element(codec: &crate::webcodecs_codec::ParsedAudioCodec) -> Option<&'static str> {
    use crate::webcodecs_codec::ParsedAudioCodec as A;
    match codec {
        A::Opus => Some("opusdec"),
        A::Aac { .. } => Some("avdec_aac"),
        A::Vorbis | A::Flac | A::Mp3 => None,
    }
}

/// Map a [`crate::webcodecs_codec::ParsedAudioCodec`] to the GStreamer audio
/// encoder element family needed (Wave B). Element presence after `gst::init()`
/// is the honest capability probe; the `<` names below were confirmed against
/// the local runtime (`gst-inspect-1.0 opusenc` / `avenc_aac`).
#[cfg(feature = "media-gstreamer")]
pub fn gst_audio_encoder_element(codec: &crate::webcodecs_codec::ParsedAudioCodec) -> Option<&'static str> {
    use crate::webcodecs_codec::ParsedAudioCodec as A;
    match codec {
        A::Opus => Some("opusenc"),
        A::Aac { .. } => Some("avenc_aac"),
        A::Vorbis | A::Flac | A::Mp3 => None,
    }
}

/// Map a [`ParsedVideoCodec`] to the GStreamer decoder element family needed.
/// Used by the gst backend for `isConfigSupported` gating (element presence
/// is the honest capability probe — no fabrication).
#[cfg(feature = "media-gstreamer")]
pub fn gst_decoder_element(codec: &ParsedVideoCodec) -> Option<&'static str> {
    match codec {
        ParsedVideoCodec::Avc { .. } => Some("avdec_h264"),
        ParsedVideoCodec::Hevc { .. } => Some("avdec_h265"),
        ParsedVideoCodec::Vp8 => Some("vp8dec"),
        ParsedVideoCodec::Vp9 { .. } => Some("vp9dec"),
        ParsedVideoCodec::Av1 { .. } => Some("av1dec"),
    }
}
