// @trace REQ-BRW-051
//! Codec → GStreamer caps / decoder-element mapping + the honest platform
//! capability probe.
//!
//! Caps construction per the WebCodecs Codec Registry + ISO 14496-15:
//! - avc1/avc3 with `description` (avcC record) →
//!   `video/x-h264,stream-format=avc,alignment=au,codec_data=<avcC>`.
//! - avc1/avc3 without description → the chunk payload is Annex B
//!   (length-prefixed NALs are only decodable with an avcC), so
//!   `stream-format=byte-stream,alignment=au`.
//! - hev1/hvc1 with `description` (hvcC) →
//!   `video/x-h265,stream-format=hvc1,alignment=au,codec_data=<hvcC>`;
//!   without → `byte-stream` (Annex B).
//! - vp8 → `video/x-vp8`; vp09 → `video/x-vp9`; av01 → `video/x-av01`.

use gstreamer as gst;

use crate::config::VideoDecoderConfig;
use crate::error::WebCodecsError;
use crate::webcodecs_codec::{gst_decoder_element, ParsedCodec, ParsedVideoCodec};

/// The decoder element family for a config's codec (honest probe target).
fn element_for(codec: &ParsedVideoCodec) -> Option<&'static str> {
    gst_decoder_element(codec)
}

/// Platform probe: gst initialized + the decoder element exists.
pub fn is_available_for(config: &VideoDecoderConfig) -> bool {
    let Ok(ParsedCodec::Video(video)) = ParsedCodec::parse(&config.codec) else {
        return false;
    };
    let Some(element) = element_for(&video) else {
        return false;
    };
    if gstreamer::init().is_err() {
        return false;
    }
    gstreamer::ElementFactory::find(element).is_some()
}

/// Build source caps for the configured codec + description.
pub fn source_caps(config: &VideoDecoderConfig) -> Result<gst::Caps, WebCodecsError> {
    let ParsedCodec::Video(video) = ParsedCodec::parse(&config.codec)
        .map_err(|e| WebCodecsError::unsupported("WebCodecsUnsupportedCodec", e.reason))?
    else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            "not a video codec",
        ));
    };
    let (mime, stream_format, codec_data): (&str, Option<&str>, Option<&[u8]>) = match video {
        ParsedVideoCodec::Avc { .. } => {
            if config.description.is_some() {
                ("video/x-h264", Some("avc"), config.description.as_deref())
            } else {
                // No avcC: the payload must be Annex B (byte-stream).
                ("video/x-h264", Some("byte-stream"), None)
            }
        },
        ParsedVideoCodec::Hevc { .. } => {
            if config.description.is_some() {
                ("video/x-h265", Some("hvc1"), config.description.as_deref())
            } else {
                ("video/x-h265", Some("byte-stream"), None)
            }
        },
        ParsedVideoCodec::Vp8 => ("video/x-vp8", None, None),
        ParsedVideoCodec::Vp9 { .. } => ("video/x-vp9", None, None),
        ParsedVideoCodec::Av1 { .. } => ("video/x-av01", None, None),
    };

    let mut builder = gst::Caps::builder(mime);
    if let Some(sf) = stream_format {
        builder = builder.field("stream-format", sf);
    }
    // Decoders output full access units per chunk (WebCodecs feeds whole
    // frames, not fragments).
    builder = builder.field("alignment", "au");
    if let Some(data) = codec_data {
        builder = builder.field("codec_data", gst::Buffer::from_slice(data.to_vec()));
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(codec: &str, description: Option<Vec<u8>>) -> VideoDecoderConfig {
        VideoDecoderConfig {
            codec: codec.into(),
            description,
            coded_width: Some(320),
            coded_height: Some(240),
            display_width: Some(320),
            display_height: Some(240),
        }
    }

    #[test]
    fn avc_with_description_uses_avc_stream_format() {
        gst::init().unwrap();
        let caps = source_caps(&config("avc1.64000b", Some(vec![1, 2, 3]))).unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.get::<&str>("stream-format").unwrap(), "avc");
        assert!(s.has_field("codec_data"));
    }

    #[test]
    fn avc_without_description_is_annexb() {
        gst::init().unwrap();
        let caps = source_caps(&config("avc1.64000b", None)).unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.get::<&str>("stream-format").unwrap(), "byte-stream");
        assert!(!s.has_field("codec_data"));
    }

    #[test]
    fn vp8_caps() {
        gst::init().unwrap();
        let caps = source_caps(&config("vp8", None)).unwrap();
        assert_eq!(caps.structure(0).unwrap().name(), "video/x-vp8");
    }
}
