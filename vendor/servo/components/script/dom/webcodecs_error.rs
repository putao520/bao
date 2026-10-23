/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! Shared DOM-side glue for the WebCodecs surface (REQ-BRW-051, Bao fork
//! self-build): component `WebCodecsError` → DOM `Error` mapping, decoder
//! config dictionary → engine struct conversion, buffer-source description
//! extraction, and the encoder syntax face.
//!
//! Error mapping (task contract for the WebCodecs impl wave):
//! - `Syntax` kind → TypeError (thrown by `configure()` and rejected by
//!   `isConfigSupported()` — the WPT audio/video-decoder oracle).
//! - `WebCodecsUnconfiguredDecoder` / `WebCodecsClosedDecoder` →
//!   InvalidStateError (`decode()`/`flush()` on a non-configured decoder).
//! - `WebCodecsFlushAborted` → AbortError (`reset()` while a flush is in
//!   flight).
//! - everything else (NotSupported kind) → NotSupportedError.
//!
//! Registered boundary: component decode failures (code
//! `WebCodecsDecodeError`) land on NotSupportedError here; the WPT
//! truncated-image case expects an EncodingError face. One-line follow-up
//! candidate, registered with the Commander (2026-10-21).

use std::ffi::CString;

use js::context::JSContext;

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
// All four WebCodecs codec interfaces (Audio/Video × Decoder/Encoder) and their
// dictionaries are declared in `WebCodecs.webidl`; the generated module is the
// file module `WebCodecsBinding`.
use crate::dom::bindings::codegen::Bindings::WebCodecsBinding::{
    AudioDecoderConfig as AudioDecoderConfigDict, AudioEncoderConfig as AudioEncoderConfigDict,
    VideoDecoderConfig as VideoDecoderConfigDict,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::str::DOMString;
use webcodecs::config::{
    AudioDecoderConfig as EngineAudioDecoderConfig, AudioEncoderConfig as EngineAudioEncoderConfig,
    VideoDecoderConfig as EngineVideoDecoderConfig,
};
use webcodecs::error::{WebCodecsError, WebCodecsErrorKind};

/// Map a component error onto the DOM `Error` face (module doc for the table).
pub(crate) fn map_error(error: &WebCodecsError) -> Error {
    match error.code {
        "WebCodecsUnconfiguredDecoder" | "WebCodecsClosedDecoder" => Error::InvalidState(None),
        "WebCodecsFlushAborted" => Error::Abort(None),
        _ => match error.kind {
            WebCodecsErrorKind::Syntax => Error::Type(
                CString::new(error.reason.replace('\0', " ")).unwrap_or_default(),
            ),
            WebCodecsErrorKind::NotSupported => Error::NotSupported(None),
        },
    }
}

/// Extract the optional `description` bytes of a decoder config dictionary
/// into engine form. A detached buffer yields zero bytes (SpiderMonkey returns
/// a null pointer for detached sources) — the codec-specific validators in
/// `webcodecs::config` reject the too-short forms, so detached descriptions
/// fail loudly rather than silently decoding without configuration data.
#[allow(unsafe_code)]
pub(crate) fn extract_description(
    description: Option<&ArrayBufferViewOrArrayBuffer>,
) -> Option<Vec<u8>> {
    description.map(|source| {
        let cx = unsafe { JSContext::get_from_thread() }.expect("no active JS context");
        let no_gc = cx.no_gc();
        get_buffer_source_slice(source, no_gc).to_vec()
    })
}

/// `AudioDecoderConfig` dictionary → engine mirror.
pub(crate) fn audio_config_to_engine(
    dict: &AudioDecoderConfigDict,
    description: Option<Vec<u8>>,
) -> EngineAudioDecoderConfig {
    EngineAudioDecoderConfig {
        codec: dict.codec.to_string(),
        sample_rate: Some(f64::from(dict.sampleRate)),
        number_of_channels: Some(f64::from(dict.numberOfChannels)),
        description,
    }
}

/// `AudioEncoderConfig` dictionary → engine mirror (Wave B). `bitrate` is the
/// only optional member the encoder engine consumes (`bitrateMode` is the
/// spec's hint and has no engine control in this wave).
pub(crate) fn audio_encoder_config_to_engine(
    dict: &AudioEncoderConfigDict,
) -> EngineAudioEncoderConfig {
    EngineAudioEncoderConfig {
        codec: dict.codec.to_string(),
        sample_rate: Some(f64::from(dict.sampleRate)),
        number_of_channels: Some(f64::from(dict.numberOfChannels)),
        bitrate: dict.bitrate,
    }
}

/// `VideoDecoderConfig` dictionary → engine mirror. The DOM's
/// `displayAspectWidth/Height` members map onto the engine's
/// `display_width/height` (the engine surface predates the rotation/flip
/// dictionary members, which the DOM layer carries directly).
pub(crate) fn video_config_to_engine(
    dict: &VideoDecoderConfigDict,
    description: Option<Vec<u8>>,
) -> EngineVideoDecoderConfig {
    EngineVideoDecoderConfig {
        codec: dict.codec.to_string(),
        description,
        coded_width: dict.codedWidth,
        coded_height: dict.codedHeight,
        display_width: dict.displayAspectWidth,
        display_height: dict.displayAspectHeight,
    }
}

/// Syntax face shared by the encoder control planes: the dictionary
/// constraints that are structurally invalid regardless of backend presence
/// (WPT audio/video-encoder-config "invalidConfigs" expect a synchronous
/// TypeError from `configure()` and a rejected promise from
/// `isConfigSupported()`). `numeric_members` are the required
/// zero-rejected members (`sampleRate`, `numberOfChannels`, `width`,
/// `height`) with their DOM values.
pub(crate) fn encoder_syntax_face(
    codec: &str,
    numeric_members: &[(&'static str, u64)],
) -> Fallible<()> {
    if codec.is_empty() {
        return Err(Error::Type(c"codec is required".to_owned()));
    }
    for (name, value) in numeric_members {
        if *value == 0 {
            return Err(Error::Type(
                CString::new(format!("{name} must be non-zero")).unwrap_or_default(),
            ));
        }
    }
    Ok(())
}

/// DOMString → engine String (kept here so the codec-string handling has a
/// single seam; `DOMString` derefs but does not convert by value).
pub(crate) fn codec_string(codec: &DOMString) -> String {
    codec.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Group 1: WebCodecsError → DOM Error mapping -----------------------

    fn is_type_error(error: &Error) -> bool {
        matches!(error, Error::Type(_))
    }
    fn is_invalid_state(error: &Error) -> bool {
        matches!(error, Error::InvalidState(_))
    }
    fn is_abort(error: &Error) -> bool {
        matches!(error, Error::Abort(_))
    }
    fn is_not_supported(error: &Error) -> bool {
        matches!(error, Error::NotSupported(_))
    }

    #[test]
    fn syntax_kind_maps_to_type_error() {
        let error = WebCodecsError::syntax("WebCodecsMissingCodec", "codec is required");
        assert!(is_type_error(&map_error(&error)));
    }

    #[test]
    fn unconfigured_and_closed_map_to_invalid_state() {
        let unconfigured =
            WebCodecsError::unsupported("WebCodecsUnconfiguredDecoder", "not configured");
        let closed = WebCodecsError::unsupported("WebCodecsClosedDecoder", "closed");
        assert!(is_invalid_state(&map_error(&unconfigured)));
        assert!(is_invalid_state(&map_error(&closed)));
    }

    #[test]
    fn flush_aborted_maps_to_abort() {
        let aborted = WebCodecsError::unsupported("WebCodecsFlushAborted", "aborted by reset");
        assert!(is_abort(&map_error(&aborted)));
    }

    #[test]
    fn remaining_not_supported_kind_maps_to_not_supported() {
        let error = WebCodecsError::unsupported("WebCodecsUnsupportedCodec", "no backend");
        assert!(is_not_supported(&map_error(&error)));
    }

    // ---- Group 2: dictionary → engine config conversion --------------------

    #[test]
    fn audio_dict_to_engine_carries_fields() {
        let dict = AudioDecoderConfigDict {
            codec: DOMString::from("opus"),
            sampleRate: 48000,
            numberOfChannels: 2,
            description: None,
        };
        let engine = audio_config_to_engine(&dict, Some(vec![1, 2, 3]));
        assert_eq!(engine.codec, "opus");
        assert_eq!(engine.sample_rate, Some(48000.0));
        assert_eq!(engine.number_of_channels, Some(2.0));
        assert_eq!(engine.description, Some(vec![1, 2, 3]));
    }

    #[test]
    fn audio_encoder_dict_to_engine_carries_fields() {
        let dict = AudioEncoderConfigDict {
            codec: DOMString::from("opus"),
            sampleRate: 48000,
            numberOfChannels: 2,
            bitrate: Some(128000),
            bitrateMode: crate::dom::bindings::codegen::Bindings::WebCodecsBinding::BitrateMode::Variable,
        };
        let engine = audio_encoder_config_to_engine(&dict);
        assert_eq!(engine.codec, "opus");
        assert_eq!(engine.sample_rate, Some(48000.0));
        assert_eq!(engine.number_of_channels, Some(2.0));
        assert_eq!(engine.bitrate, Some(128000));
        let no_bitrate = AudioEncoderConfigDict {
            codec: DOMString::from("mp4a.40.2"),
            sampleRate: 44100,
            numberOfChannels: 1,
            bitrate: None,
            bitrateMode: crate::dom::bindings::codegen::Bindings::WebCodecsBinding::BitrateMode::Variable,
        };
        assert_eq!(audio_encoder_config_to_engine(&no_bitrate).bitrate, None);
    }

    #[test]
    fn video_dict_to_engine_carries_fields() {
        let dict = VideoDecoderConfigDict {
            codec: DOMString::from("vp8"),
            description: None,
            codedWidth: Some(320),
            codedHeight: Some(240),
            displayAspectWidth: Some(640),
            displayAspectHeight: Some(480),
            colorSpace: None,
            hardwareAcceleration: crate::dom::bindings::codegen::Bindings::WebCodecsBinding::HardwareAcceleration::No_preference,
            optimizeForLatency: false,
            rotation: 0.0,
            flip: false,
        };
        let engine = video_config_to_engine(&dict, None);
        assert_eq!(engine.codec, "vp8");
        assert_eq!(engine.coded_width, Some(320));
        assert_eq!(engine.display_width, Some(640));
        assert_eq!(engine.description, None);
    }

    // ---- Group 3: encoder syntax face --------------------------------------

    #[test]
    fn empty_codec_is_a_type_error() {
        assert!(is_type_error(&encoder_syntax_face("", &[]).unwrap_err()));
    }

    #[test]
    fn zero_numeric_members_are_type_errors() {
        let error = encoder_syntax_face("opus", &[("sampleRate", 48000), ("numberOfChannels", 0)])
            .unwrap_err();
        assert!(is_type_error(&error));
        assert!(encoder_syntax_face("vp8", &[("width", 320), ("height", 240)]).is_ok());
    }

    #[test]
    fn codec_string_conversion() {
        assert_eq!(codec_string(&DOMString::from("pcm-f32")), "pcm-f32");
    }
}
