// @trace REQ-BRW-051
//! Decoder config types + validation. Plain-data mirrors of the WebCodecs
//! `VideoDecoderConfig` / `AudioDecoderConfig` dictionaries: the DOM layer
//! converts JS dictionaries into these structs and calls
//! [`validate_video_config`] / [`validate_audio_config`].
//!
//! Validation split follows WPT webcodecs/video-decoder.https.any.js and
//! audio-decoder.https.any.js exactly:
//! - **TypeError** (thrown synchronously by `configure` /
//!   `isConfigSupported`): missing/empty codec, missing/zero sampleRate or
//!   numberOfChannels (audio), zero codedWidth/Height, video codec given to
//!   an audio decoder, detached `description` (an ArrayBuffer the page has
//!   already transferred away — the DOM layer detects this before calling us
//!   since we only see bytes).
//! - **`supported=false`** (no throw): well-formed codec string the registry
//!   knows but no backend for, future codec levels, ambiguous video codecs
//!   (`vp9` bare in the audio plane), MIME-form codec strings, codec-specific
//!   description validation failures (opus >2ch without description, vorbis
//!   /flac without description, hvcC/hevc without description).
//!
//! Codec-specific description validation follows the W3C WebCodecs Codec
//! Registry "description" requirements:
//! - opus: multichannel (>2) requires a >=10-byte OpusHead description
//!   (WPT: "at least 10 bytes are required for multichannel").
//! - vorbis / flac: description always required.
//! - avc1/avc3 (AVC format): `description` = avcC record. For `avc3` /
//!   Annex-B input, description is absent and SPS/PPS must be in-band.
//! - hev1/hvc1: `description` = hvcC record (required for hvc1; hev1 may
//!   carry in-band parameter sets).

use crate::error::WebCodecsError;
use crate::webcodecs_codec::{ParsedCodec, ParsedImageCodec, ParsedVideoCodec};

/// `VideoDecoderConfig` mirror (plain data; u16/u32 fields use `None` for
/// "absent" so the DOM layer can distinguish 0 from unset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDecoderConfig {
    pub codec: String,
    pub description: Option<Vec<u8>>,
    /// `None` = not supplied; `Some(0)` = supplied-as-zero (a TypeError).
    pub coded_width: Option<u32>,
    pub coded_height: Option<u32>,
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
}

/// `AudioDecoderConfig` mirror.
///
/// No `Eq`: `sampleRate` / `numberOfChannels` are `f64` (WebCodecs dictionaries
/// carry JS numbers), and `f64` only implements `PartialEq`.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDecoderConfig {
    pub codec: String,
    pub sample_rate: Option<f64>,
    pub number_of_channels: Option<f64>,
    pub description: Option<Vec<u8>>,
}

/// `AudioEncoderConfig` mirror (Wave B, REQ-BRW-051). Mirrors the WebCodecs
/// dictionary: `codec` + `sampleRate` + `numberOfChannels` are required,
/// `bitrate` is optional. `sampleRate`/`numberOfChannels` are `f64` for the
/// same reason as [`AudioDecoderConfig`] (they arrive as JS numbers; the
/// DOM layer narrows the `[EnforceRange] unsigned long` values).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioEncoderConfig {
    pub codec: String,
    pub sample_rate: Option<f64>,
    pub number_of_channels: Option<f64>,
    /// Bits per second; `None` = the backend's own default.
    pub bitrate: Option<u64>,
}

/// ImageDecoder init mirror (`data` is supplied by the DOM layer separately —
/// it may be a BufferSource or a ReadableStream; only the fully-buffered form
/// reaches this plane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDecoderConfig {
    pub mime_type: String,
    pub prefer_animation: bool,
}

/// Outcome of config validation: either the config is accepted for a concrete
/// backend, or it is rejected with an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValidation {
    /// Backend can decode this config. Carries the parsed codec for the
    /// backend's use.
    Supported(ParsedCodec),
    /// Well-formed config, no backend. `isConfigSupported` returns
    /// `supported=false`; `configure()` fires NotSupportedError and closes.
    Unsupported(WebCodecsError),
}

/// Validate a `VideoDecoderConfig` against the registry + backend presence.
///
/// `backend_available` is the honest capability probe (gst element presence on
/// this platform) — the validator never fabricates support.
pub fn validate_video_config(
    config: &VideoDecoderConfig,
    backend_available: bool,
) -> Result<ConfigValidation, WebCodecsError> {
    // TypeError face: missing/empty codec, zero dimensions.
    if config.codec.is_empty() {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingCodec",
            "codec is required",
        ));
    }
    if let (Some(0), _) | (_, Some(0)) = (config.coded_width, config.coded_height) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroDimensions",
            "codedWidth/codedHeight must be non-zero",
        ));
    }
    if let (Some(0), _) | (_, Some(0)) = (config.display_width, config.display_height) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroDimensions",
            "displayWidth/displayHeight must be non-zero",
        ));
    }

    let parsed = ParsedCodec::parse(&config.codec)?;
    let ParsedCodec::Video(video) = parsed else {
        return Err(WebCodecsError::syntax(
            "WebCodecsAudioInVideo",
            format!("codec {} is not a video codec", config.codec),
        ));
    };

    // Codec-specific description requirements (codec registry §description).
    validate_video_description(&video, config.description.as_deref())?;

    if !backend_available {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("no decode backend for {}", config.codec),
        )));
    }
    Ok(ConfigValidation::Supported(ParsedCodec::Video(video)))
}

/// Validate a `AudioDecoderConfig`.
pub fn validate_audio_config(
    config: &AudioDecoderConfig,
    backend_available: bool,
) -> Result<ConfigValidation, WebCodecsError> {
    if config.codec.is_empty() {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingCodec",
            "codec is required",
        ));
    }
    // sampleRate / numberOfChannels: required, > 0. The WPT contract supplies
    // them as numbers; the DOM layer converts to f64 (a non-numeric type is a
    // TypeError raised in the DOM layer before reaching here).
    let Some(rate) = config.sample_rate else {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingSampleRate",
            "sampleRate is required",
        ));
    };
    if !(rate > 0.0) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroSampleRate",
            "sampleRate must be non-zero",
        ));
    }
    let Some(channels) = config.number_of_channels else {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingChannels",
            "numberOfChannels is required",
        ));
    };
    if !(channels > 0.0) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroChannels",
            "numberOfChannels must be non-zero",
        ));
    }

    let parsed = ParsedCodec::parse(&config.codec)?;
    // "Video codec" (vp8) and "Ambiguous codec" (vp9 bare) in the audio plane
    // are NotSupported, not Syntax — a bare "vp9" parses as a video codec and
    // is therefore simply not an audio codec.
    let ParsedCodec::Audio(audio) = parsed else {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("codec {} is not an audio codec", config.codec),
        )));
    };

    // Codec-specific description requirements.
    validate_audio_description(&audio, channels, config.description.as_deref())?;

    if !backend_available {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("no decode backend for {}", config.codec),
        )));
    }
    Ok(ConfigValidation::Supported(ParsedCodec::Audio(audio)))
}

/// Validate an `AudioEncoderConfig` (Wave B, REQ-BRW-051).
///
/// Same split as the decode plane: structurally invalid dictionaries are
/// `Err(Syntax)` (the DOM layer turns them into a TypeError from `configure()`
/// and a rejected `isConfigSupported()` promise); well-formed configs without
/// a backend are `Ok(Unsupported)`, which the DOM layer reports as
/// `supported=false` / `error(NotSupportedError)` — never as a fabricated
/// `supported=true`.
///
/// `backend_available` is the honest platform probe (gst encoder element
/// presence); the validator never second-guesses it.
pub fn validate_audio_encoder_config(
    config: &AudioEncoderConfig,
    backend_available: bool,
) -> Result<ConfigValidation, WebCodecsError> {
    if config.codec.is_empty() {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingCodec",
            "codec is required",
        ));
    }
    let Some(rate) = config.sample_rate else {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingSampleRate",
            "sampleRate is required",
        ));
    };
    if !(rate > 0.0) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroSampleRate",
            "sampleRate must be non-zero",
        ));
    }
    let Some(channels) = config.number_of_channels else {
        return Err(WebCodecsError::syntax(
            "WebCodecsMissingChannels",
            "numberOfChannels is required",
        ));
    };
    if !(channels > 0.0) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroChannels",
            "numberOfChannels must be non-zero",
        ));
    }

    let parsed = ParsedCodec::parse(&config.codec)?;
    let ParsedCodec::Audio(audio) = parsed else {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("codec {} is not an audio codec", config.codec),
        )));
    };

    // Verified encoder mappings in this wave: opus (opusenc) + AAC
    // (avenc_aac). vorbis/flac/mp3 encoders exist on many hosts but their
    // configuration contracts are outside this slice — claiming them would be
    // a capability lie.
    if matches!(
        audio,
        crate::webcodecs_codec::ParsedAudioCodec::Vorbis |
            crate::webcodecs_codec::ParsedAudioCodec::Flac |
            crate::webcodecs_codec::ParsedAudioCodec::Mp3
    ) {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("no encoder backend for {}", config.codec),
        )));
    }

    if !backend_available {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("no encode backend for {}", config.codec),
        )));
    }
    Ok(ConfigValidation::Supported(ParsedCodec::Audio(audio)))
}

/// Codec-registry video description requirements.
fn validate_video_description(
    video: &ParsedVideoCodec,
    description: Option<&[u8]>,
) -> Result<(), WebCodecsError> {
    match video {
        // hvc1 REQUIRES a description (hvcC record); hev1 may run Annex-B
        // style with in-band parameter sets. With no description and no
        // in-band knowledge we cannot verify — fail closed as Unsupported
        // (WPT h265 tests always pass a description).
        ParsedVideoCodec::Hevc { hvc1, .. } => {
            if *hvc1 && description.is_none() {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsMissingDescription",
                    "hvc1 requires an hvcC description",
                ));
            }
            Ok(())
        },
        // avc1/avc3: description optional (Annex-B in-band is legal).
        ParsedVideoCodec::Avc { .. } => Ok(()),
        ParsedVideoCodec::Vp8 => Ok(()),
        ParsedVideoCodec::Vp9 { .. } => Ok(()),
        ParsedVideoCodec::Av1 { .. } => Ok(()),
    }
}

/// Codec-registry audio description requirements.
fn validate_audio_description(
    audio: &crate::webcodecs_codec::ParsedAudioCodec,
    channels: f64,
    description: Option<&[u8]>,
) -> Result<(), WebCodecsError> {
    use crate::webcodecs_codec::ParsedAudioCodec as A;
    match audio {
        // Opus multichannel (>2) requires a >=10 byte OpusHead description
        // (WPT: "at least 10 bytes are required for multichannel").
        A::Opus => {
            if channels > 2.0 && (description.is_none() || description.is_some_and(|d| d.len() < 10)) {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsInvalidDescription",
                    "opus with >2 channels requires a >=10 byte OpusHead description",
                ));
            }
            Ok(())
        },
        A::Vorbis | A::Flac => {
            if description.is_none() {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsMissingDescription",
                    format!("{audio:?} requires a description"),
                ));
            }
            Ok(())
        },
        A::Mp3 | A::Aac { .. } => Ok(()),
    }
}

/// ImageDecoder type + data validation. Returns the parsed image codec when
/// the platform can decode it; `Unsupported` otherwise.
pub fn validate_image_config(
    config: &ImageDecoderConfig,
    backend_available: bool,
) -> Result<ConfigValidation, WebCodecsError> {
    let parsed = ParsedCodec::parse_image_type(&config.mime_type)?;
    if !backend_available {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedType",
            format!("no image backend for {}", config.mime_type),
        )));
    }
    // Every parsed image codec has a pure-Rust decode path via the `image`
    // crate except AVIF (ravif/dav1d are encode-only in this tree's feature
    // set) — see image_decoder.rs for the per-type capability matrix.
    if matches!(parsed, ParsedCodec::Image(ParsedImageCodec::Avif)) {
        return Ok(ConfigValidation::Unsupported(WebCodecsError::unsupported(
            "WebCodecsUnsupportedType",
            "image/avif decode is not available (no AV1 still-image backend)",
        )));
    }
    Ok(ConfigValidation::Supported(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WebCodecsErrorKind;

    fn encoder_config(
        codec: &str,
        rate: Option<f64>,
        channels: Option<f64>,
    ) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec: codec.into(),
            sample_rate: rate,
            number_of_channels: channels,
            bitrate: None,
        }
    }

    #[test]
    fn audio_encoder_missing_required_members_are_syntax_errors() {
        assert!(matches!(
            validate_audio_encoder_config(&encoder_config("", Some(48000.0), Some(2.0)), true),
            Err(WebCodecsError { kind: WebCodecsErrorKind::Syntax, .. })
        ));
        assert!(matches!(
            validate_audio_encoder_config(&encoder_config("opus", None, Some(2.0)), true),
            Err(WebCodecsError { kind: WebCodecsErrorKind::Syntax, .. })
        ));
        assert!(matches!(
            validate_audio_encoder_config(&encoder_config("opus", Some(0.0), Some(2.0)), true),
            Err(WebCodecsError { kind: WebCodecsErrorKind::Syntax, .. })
        ));
        assert!(matches!(
            validate_audio_encoder_config(&encoder_config("opus", Some(48000.0), None), true),
            Err(WebCodecsError { kind: WebCodecsErrorKind::Syntax, .. })
        ));
        assert!(matches!(
            validate_audio_encoder_config(&encoder_config("opus", Some(48000.0), Some(0.0)), true),
            Err(WebCodecsError { kind: WebCodecsErrorKind::Syntax, .. })
        ));
    }

    #[test]
    fn audio_encoder_video_codec_is_unsupported_not_syntax() {
        let config = encoder_config("vp8", Some(48000.0), Some(2.0));
        assert!(matches!(
            validate_audio_encoder_config(&config, true),
            Ok(ConfigValidation::Unsupported(_))
        ));
    }

    #[test]
    fn audio_encoder_probe_governs_support() {
        let opus = encoder_config("opus", Some(48000.0), Some(2.0));
        assert!(matches!(
            validate_audio_encoder_config(&opus, true),
            Ok(ConfigValidation::Supported(ParsedCodec::Audio(_)))
        ));
        assert!(matches!(
            validate_audio_encoder_config(&opus, false),
            Ok(ConfigValidation::Unsupported(_))
        ));
        let aac = encoder_config("mp4a.40.2", Some(44100.0), Some(2.0));
        assert!(matches!(
            validate_audio_encoder_config(&aac, true),
            Ok(ConfigValidation::Supported(_))
        ));
    }

    #[test]
    fn audio_encoder_unverified_encoder_families_are_unsupported() {
        for codec in ["vorbis", "flac", "mp3"] {
            let config = encoder_config(codec, Some(48000.0), Some(2.0));
            assert!(
                matches!(
                    validate_audio_encoder_config(&config, true),
                    Ok(ConfigValidation::Unsupported(_))
                ),
                "{codec} must not be claimed as encodable"
            );
        }
        // Future/unregistered codec strings fail at the *parse* layer
        // (NotSupported throw), never reaching the encoder-family gate.
        let future = encoder_config("mp4a.FF.9", Some(48000.0), Some(2.0));
        assert!(matches!(
            validate_audio_encoder_config(&future, true),
            Err(_)
        ));
    }
}
