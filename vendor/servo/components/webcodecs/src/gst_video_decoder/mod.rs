// @trace REQ-BRW-051
//! GStreamer video decode backend (feature `media-gstreamer`): one
//! `appsrc → <decoder> → appsink` pipeline per configured codec.
//!
//! Honest capability probe: `is_available_for` checks the GStreamer decoder
//! *element* exists at configure time (nothing is fabricated). Decode
//! failures surface as a [`WebCodecsError`] which the control plane
//! (video_decoder.rs) turns into the `error` callback + close + a rejected
//! flush — the WPT contract.
//!
//! Output format: appsink caps request `video/x-raw,format=I420` — the
//! decoder + videoconvert narrow everything (incl. 10/12-bit, 4:4:4) to
//! 8-bit planar I420 (WebCodecs "valid, but lossy" contract for high-bit-
//! depth sources; the timestamps and frame geometry stay exact).

use crate::error::WebCodecsError;
use crate::video_decoder::EncodedVideoChunk;
use crate::video_frame::DecodedVideoFrame;

pub mod caps;
pub mod decode;
pub mod iso14496;

/// Backend trait boundary used by the control plane (kept local so the
/// `media-gstreamer` feature fully owns the gst dependency).
pub trait VideoDecoderBackend: Send {
    /// Honest capability probe: is there a decoder element for this config's
    /// codec on this platform?
    fn is_available_for(config: &crate::config::VideoDecoderConfig) -> bool
    where
        Self: Sized;
    /// Build a backend for a validated config.
    fn new(config: &crate::config::VideoDecoderConfig) -> Result<Self, WebCodecsError>
    where
        Self: Sized;
    fn decode(&mut self, chunk: EncodedVideoChunk) -> Result<(), WebCodecsError>;
    /// Drain: pull every produced frame out of appsink.
    fn flush(&mut self) -> Result<Vec<DecodedVideoFrame>, WebCodecsError>;
    fn reset(&mut self);
}

/// The gst-backed implementation handle.
pub struct GstreamerVideoDecoderBackend {
    inner: decode::Pipeline,
}

impl VideoDecoderBackend for GstreamerVideoDecoderBackend {
    fn is_available_for(config: &crate::config::VideoDecoderConfig) -> bool {
        caps::is_available_for(config)
    }

    fn new(config: &crate::config::VideoDecoderConfig) -> Result<Self, WebCodecsError> {
        Ok(Self {
            inner: decode::Pipeline::new(config)?,
        })
    }

    fn decode(&mut self, chunk: EncodedVideoChunk) -> Result<(), WebCodecsError> {
        self.inner.push_chunk(chunk)
    }

    fn flush(&mut self) -> Result<Vec<DecodedVideoFrame>, WebCodecsError> {
        self.inner.drain()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
