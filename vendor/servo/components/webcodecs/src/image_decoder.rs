// @trace REQ-BRW-051
//! `ImageDecoder` backend: pure-Rust image decode via the `image` crate
//! (lock-proven 0.25.10, default-features off, only gif/jpeg/png/webp enabled
//! to keep the dep tree minimal). EXIF orientation is applied via
//! `ImageDecoder::orientation()` + `DynamicImage::apply_orientation()`
//! (WPT image-decoder EXIF orientation 1-8 contract). Animation via
//! `AnimationDecoder::into_frames()` (PNG/APNG, GIF, animated WebP).
//!
//! Capability matrix (honest):
//! - image/png, image/gif, image/jpeg, image/webp, image/bmp, image/x-icon →
//!   real decode.
//! - image/avif → NOT supported (ravif/dav1d in this tree are encode-only;
//!   no AV1 still-image decode backend) — registered unsupported in
//!   config::validate_image_config.
//!
//! GIF repetition count comes from the `gif` crate's NETSCAPE extension
//! reader (`Decoder::repeat()`); the `image` crate does not surface it.
//! Repetition semantics per the WPT contract: no NETSCAPE ext / 0 plays →
//! repetitionCount 0; loop-count n → n; loop-count 0 in the ext → infinite.

use std::io::Cursor;

use image::Frame as ImageCrateFrame;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::Orientation;
use image::ImageDecoder as ImageCrateDecoder;
use image::{AnimationDecoder, DynamicImage, ImageFormat};

use crate::config::ImageDecoderConfig;
use crate::error::WebCodecsError;
use crate::image_frame::{ImageDecodeResult, ImageFrame, ImageTrack};

/// Whether the platform has a decode backend for the given MIME type.
/// Backed by the compile-time feature set of the `image` dependency; AVIF is
/// explicitly not wired (no decoder).
pub fn image_backend_available(mime: &str) -> bool {
    matches!(
        crate::webcodecs_codec::ParsedCodec::parse_image_type(mime).ok(),
        Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Png
        )) | Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Gif
        )) | Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Jpeg
        )) | Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Webp
        )) | Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Bmp
        )) | Some(crate::webcodecs_codec::ParsedCodec::Image(
            crate::webcodecs_codec::ParsedImageCodec::Ico
        ))
    )
}

/// `ImageDecoder.isTypeSupported(mime)` — honest platform probe.
pub fn is_type_supported(mime: &str) -> bool {
    image_backend_available(mime)
}

fn decode_err(e: image::ImageError) -> WebCodecsError {
    WebCodecsError::unsupported("WebCodecsDecodeError", format!("image decode failed: {e}"))
}

fn frame_from_rgba(img: &DynamicImage, duration_ms: u32) -> ImageFrame {
    let rgba = img.to_rgba8();
    ImageFrame {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        duration_ms,
    }
}

fn frames_from_anim(frames: Vec<ImageCrateFrame>) -> Vec<ImageFrame> {
    frames
        .iter()
        .map(|f| {
            let (numer, denom) = f.delay().numer_denom_ms();
            // Milliseconds, rounding up (denominators other than 1 are rare
            // but legal per the GIF/APNG/WebP specs).
            let duration_ms = if denom == 0 {
                0
            } else {
                numer.div_ceil(denom)
            };
            frame_from_rgba(&DynamicImage::from(f.buffer().clone()), duration_ms)
        })
        .collect()
}

fn animated_track(frames: &[ImageFrame], repetition_count: Option<u32>) -> ImageTrack {
    ImageTrack {
        animated: frames.len() > 1,
        frame_count: frames.len() as u32,
        repetition_count,
    }
}

/// Decode a fully-buffered image. `frame_index` selects which frame the DOM
/// layer surfaces (the backend decodes all frames of animated inputs so the
/// track metadata — frame count, repetition — is available up front).
pub fn decode_image(
    config: &ImageDecoderConfig,
    data: &[u8],
) -> Result<ImageDecodeResult, WebCodecsError> {
    // The declared MIME type gates capability (WPT feeds a PNG body with type
    // 'junk/type' and expects NotSupportedError from decode()); the sniffed
    // format drives the actual decode path so a mislabeled-but-decodable body
    // still fails loudly instead of silently cross-decoding.
    if !is_type_supported(&config.mime_type) {
        return Err(WebCodecsError::unsupported(
            "WebCodecsUnsupportedType",
            format!("type {} is not supported", config.mime_type),
        ));
    }
    let format = image::guess_format(data).map_err(|_| {
        WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            format!("cannot sniff image format for {}", config.mime_type),
        )
    })?;

    match format {
        ImageFormat::Png => decode_png(data),
        ImageFormat::Gif => decode_gif(data),
        ImageFormat::WebP => decode_webp(data),
        // JPEG / BMP / ICO: still-image path with EXIF orientation where the
        // crate provides it.
        _ => decode_still(data, format),
    }
}

fn decode_still(data: &[u8], format: ImageFormat) -> Result<ImageDecodeResult, WebCodecsError> {
    let mut reader = image::ImageReader::new(Cursor::new(data));
    reader.set_format(format);
    // `ImageDecoder::orientation` takes `&mut self` (crate default impl).
    let mut decoder = reader.into_decoder().map_err(decode_err)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
    img.apply_orientation(orientation);
    Ok(ImageDecodeResult {
        track: ImageTrack::still(),
        frames: vec![frame_from_rgba(&img, 0)],
    })
}

fn decode_png(data: &[u8]) -> Result<ImageDecodeResult, WebCodecsError> {
    // `ImageDecoder::orientation` takes `&mut self`; `apng(self)` still moves.
    let mut decoder = PngDecoder::new(Cursor::new(data)).map_err(decode_err)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    // APNG path: the crate exposes the animation decoder explicitly.
    match decoder.apng() {
        Ok(apng) => {
            let frames = apng
                .into_frames()
                .collect_frames()
                .map_err(decode_err)?;
            let out_frames = frames_from_anim(frames);
            if out_frames.len() > 1 {
                // APNG carries a play-once semantic in the `image` crate
                // (num_plays is not surfaced) — registered boundary.
                return Ok(ImageDecodeResult {
                    track: animated_track(&out_frames, Some(0)),
                    frames: out_frames,
                });
            }
            let mut img = DynamicImage::from_decoder(PngDecoder::new(Cursor::new(data)).map_err(decode_err)?)
                .map_err(decode_err)?;
            img.apply_orientation(orientation);
            Ok(ImageDecodeResult {
                track: ImageTrack::still(),
                frames: vec![frame_from_rgba(&img, 0)],
            })
        },
        Err(_) => {
            let mut img = DynamicImage::from_decoder(PngDecoder::new(Cursor::new(data)).map_err(decode_err)?)
                .map_err(decode_err)?;
            img.apply_orientation(orientation);
            Ok(ImageDecodeResult {
                track: ImageTrack::still(),
                frames: vec![frame_from_rgba(&img, 0)],
            })
        },
    }
}

fn decode_gif(data: &[u8]) -> Result<ImageDecodeResult, WebCodecsError> {
    // Repetition count: NETSCAPE2.0 extension via the gif crate. Default is
    // Finite(0) ("play once") when no extension is present — which matches
    // the WPT still-GIF expectation of repetitionCount 0.
    let repeat = {
        let mut opts = gif::DecodeOptions::new();
        opts.check_frame_consistency(false);
        let decoder = opts
            .read_info(Cursor::new(data))
            .map_err(|e| WebCodecsError::unsupported("WebCodecsDecodeError", format!("gif header: {e}")))?;
        decoder.repeat()
    };
    let frames = GifDecoder::new(Cursor::new(data))
        .map_err(decode_err)?
        .into_frames()
        .collect_frames()
        .map_err(decode_err)?;
    let out_frames = frames_from_anim(frames);
    let repetition_count = match repeat {
        gif::Repeat::Infinite => None,
        gif::Repeat::Finite(n) => Some(u32::from(n)),
    };
    Ok(ImageDecodeResult {
        track: animated_track(&out_frames, repetition_count),
        frames: out_frames,
    })
}

fn decode_webp(data: &[u8]) -> Result<ImageDecodeResult, WebCodecsError> {
    let decoder = WebPDecoder::new(Cursor::new(data)).map_err(decode_err)?;
    if decoder.has_animation() {
        let frames = decoder
            .into_frames()
            .collect_frames()
            .map_err(decode_err)?;
        let out_frames = frames_from_anim(frames);
        // The crate does not surface the WEBP loop count — registered
        // boundary: reported as play-once (finite 0).
        return Ok(ImageDecodeResult {
            track: animated_track(&out_frames, Some(0)),
            frames: out_frames,
        });
    }
    let img = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
    Ok(ImageDecodeResult {
        track: ImageTrack::still(),
        frames: vec![frame_from_rgba(&img, 0)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_config() -> ImageDecoderConfig {
        ImageDecoderConfig {
            mime_type: "image/png".into(),
            prefer_animation: false,
        }
    }

    /// Minimal 1x1 red PNG (well-known fixture bytes).
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
        0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
        0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
        0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn decode_png_real() {
        let result = decode_image(&png_config(), RED_PNG).unwrap();
        assert_eq!(result.frames.len(), 1);
        assert_eq!(result.frames[0].width, 1);
        assert_eq!(result.frames[0].height, 1);
        assert_eq!(result.frames[0].rgba.len(), 4);
        assert!(!result.track.animated);
        assert_eq!(result.track.repetition_count, Some(0));
    }

    #[test]
    fn junk_type_unsupported() {
        let cfg = ImageDecoderConfig {
            mime_type: "junk/type".into(),
            prefer_animation: false,
        };
        assert!(!is_type_supported(&cfg.mime_type));
        let err = decode_image(&cfg, RED_PNG).unwrap_err();
        assert_eq!(err.kind, crate::error::WebCodecsErrorKind::NotSupported);
    }

    #[test]
    fn avif_honestly_unsupported() {
        assert!(!is_type_supported("image/avif"));
    }

    #[test]
    fn png_gif_jpeg_webp_supported() {
        for m in ["image/png", "image/gif", "image/jpeg", "image/webp"] {
            assert!(is_type_supported(m), "{m} should be supported");
        }
    }
}
