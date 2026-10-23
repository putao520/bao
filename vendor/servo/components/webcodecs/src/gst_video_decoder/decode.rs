// @trace REQ-BRW-051
//! The GStreamer video decode pipeline driver.
//!
//! Pipeline per codec: `appsrc → <decoder> → videoconvert → appsink`. The
//! appsink caps request `video/x-raw,format=I420` (8-bit planar 4:2:0) so
//! every codec + color depth narrows to the one portable output shape
//! (WebCodecs "valid, but lossy" contract for high bit depth). Timestamps
//! pass through exactly (microseconds; `ClockTime::from_useconds` / the
//! bare `ClockTime::nseconds()` verified against the cached crate source).
//!
//! Decode failure contract: a GStreamer `error` bus message →
//! [`WebCodecsError`] → the control plane closes the decoder, fires
//! `error`, and rejects the in-flight flush (WPT "corrupt chunk → error()").
//! An `eos` bus message marks the deterministic drain point: we pull until
//! appsink reports EOS, then restart for the next round (WPT decodes twice
//! on one decoder).

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_video::{VideoFrameExt, VideoFrameRef};

use crate::config::VideoDecoderConfig;
use crate::error::WebCodecsError;
use crate::video_decoder::{EncodedVideoChunk, EncodedVideoChunkType};
use crate::video_frame::{DecodedVideoFrame, FrameColorSpace, FrameFormat};
use crate::webcodecs_codec::{ParsedCodec, ParsedVideoCodec};

use super::caps::{is_available_for, source_caps};
use super::iso14496;

/// One configured decoder pipeline.
pub struct Pipeline {
    appsrc: AppSrc,
    appsink: AppSink,
    pipeline: gst::Pipeline,
    bus: gst::Bus,
    is_avc_with_description: bool,
    seen_key: bool,
}

impl Pipeline {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, WebCodecsError> {
        gst::init().map_err(|e| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", format!("gst::init: {e}"))
        })?;
        if !is_available_for(config) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst decoder for {}", config.codec),
            ));
        }
        let ParsedCodec::Video(video) =
            ParsedCodec::parse(&config.codec).map_err(|e| WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                e.reason,
            ))?
        else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                "not a video codec",
            ));
        };

        // avc1/avc3 with a description: the WebCodecs chunk payload is
        // length-prefixed (no start codes); convert to Annex B so appsrc can
        // feed `stream-format=avc` decoders a consistent byte stream.
        let is_avc_with_description = matches!(video, ParsedVideoCodec::Avc { .. })
            && config.description.is_some();

        let src_caps = source_caps(config)?;
        let decoder_element = decoder_element_name(&video);

        let src = gst::ElementFactory::make("appsrc")
            .build()
            .map_err(bus_error("appsrc"))?;
        let dec = gst::ElementFactory::make(decoder_element)
            .build()
            .map_err(bus_error(decoder_element))?;
        let convert = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(bus_error("videoconvert"))?;
        let sink = gst::ElementFactory::make("appsink")
            .build()
            .map_err(bus_error("appsink"))?;

        // `ElementFactory::make` yields a plain `gst::Element`; the concrete
        // element interface is reached by downcast (same shape as the
        // servo-media gstreamer backend precedent). Fail closed on a factory
        // that hands back a non-appsrc/appsink element.
        let appsrc = src
            .clone()
            .downcast::<AppSrc>()
            .map_err(downcast_error("appsrc"))?;
        let appsink = sink
            .clone()
            .downcast::<AppSink>()
            .map_err(downcast_error("appsink"))?;

        let out_caps = gst::Caps::builder("video/x-raw").field("format", "I420").build();
        appsink.set_caps(Some(&out_caps));
        appsink.set_max_buffers(0);
        appsink.set_drop(false);

        let pipeline = gst::Pipeline::new();
        pipeline
            .add_many([&src, &dec, &convert, &sink])
            .map_err(bus_error("add_many"))?;
        gst::Element::link_many([&src, &dec, &convert, &sink])
            .map_err(bus_error("link_many"))?;

        let bus = pipeline.bus().ok_or_else(|| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", "pipeline has no bus")
        })?;

        appsrc.set_caps(Some(&src_caps));
        appsrc.set_format(gst::Format::Time);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| WebCodecsError::unsupported("WebCodecsGstreamerInit", "did not start"))?;

        Ok(Self {
            appsrc,
            appsink,
            pipeline,
            bus,
            is_avc_with_description,
            seen_key: false,
        })
    }

    /// Feed one encoded chunk to the decoder.
    pub fn push_chunk(&mut self, chunk: EncodedVideoChunk) -> Result<(), WebCodecsError> {
        self.poll_bus_for_errors()?;
        if chunk.chunk_type == EncodedVideoChunkType::Key {
            self.seen_key = true;
        }
        // A delta before any keyframe is dropped silently (WebCodecs §decode;
        // the decoder has no reference state). WPT codec tests never send a
        // delta-first sequence, so no test-visible behavior rides on this —
        // kept spec-faithful.
        if chunk.chunk_type == EncodedVideoChunkType::Delta && !self.seen_key {
            return Ok(());
        }
        let payload = if self.is_avc_with_description {
            iso14496::length_prefixed_to_annexb(&chunk.data).unwrap_or_else(|| chunk.data.clone())
        } else {
            chunk.data.clone()
        };

        let mut buffer = gst::Buffer::from_slice(payload);
        {
            let buf = buffer.get_mut().expect("fresh buffer is writable");
            buf.set_pts(gst::ClockTime::from_useconds(chunk.timestamp.unsigned_abs()));
            if let Some(d) = chunk.duration {
                buf.set_duration(gst::ClockTime::from_useconds(d.unsigned_abs()));
            }
        }
        self.appsrc.push_buffer(buffer).map_err(|_| self.take_bus_error())?;
        Ok(())
    }

    /// Spec `flush()`: send EOS, drain every produced frame from appsink until
    /// the bus reports the matching `eos`, then restart the pipeline.
    pub fn drain(&mut self) -> Result<Vec<DecodedVideoFrame>, WebCodecsError> {
        self.appsrc
            .end_of_stream()
            .map_err(|_| self.take_bus_error())?;
        let mut frames = Vec::new();
        // Deterministic drain: consume frames as appsink emits them; stop on
        // the pipeline eos bus message. A pending gst error message → error.
        loop {
            // Non-blocking pull: a produced frame returns Some immediately.
            while let Some(sample) = self.appsink.try_pull_sample(gst::ClockTime::from_mseconds(1)) {
                frames.push(sample_to_frame(&sample)?);
            }
            match self
                .bus
                .timed_pop_filtered(Some(gst::ClockTime::from_mseconds(50)), &[
                    gst::MessageType::Eos,
                    gst::MessageType::Error,
                ]) {
                Some(msg) => match msg.view() {
                    gst::MessageView::Eos(_) => break,
                    gst::MessageView::Error(err) => {
                        return Err(WebCodecsError::unsupported(
                            "WebCodecsDecodeError",
                            format!("gst pipeline error: {}", err.error()),
                        ));
                    },
                    _ => {},
                },
                None => {
                    // No frame + no terminal message inside 50ms → keep
                    // draining (large frames can be mid-transit).
                    continue;
                },
            }
        }
        // Restart for the next round (WPT decodes twice with one decoder).
        self.seen_key = false;
        self.pipeline.set_state(gst::State::Playing).map_err(|_| self.take_bus_error())?;
        Ok(frames)
    }

    /// Spec `reset()`: stop the pipeline and drop queued state.
    pub fn reset(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        self.seen_key = false;
    }

    fn poll_bus_for_errors(&self) -> Result<(), WebCodecsError> {
        while let Some(msg) = self.bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(err) = msg.view() {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsDecodeError",
                    format!("gst pipeline error: {}", err.error()),
                ));
            }
        }
        Ok(())
    }

    fn take_bus_error(&self) -> WebCodecsError {
        self.poll_bus_for_errors().unwrap_err()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn decoder_element_name(video: &ParsedVideoCodec) -> &'static str {
    use ParsedVideoCodec as V;
    match video {
        V::Avc { .. } => "avdec_h264",
        V::Hevc { .. } => "avdec_h265",
        V::Vp8 => "vp8dec",
        V::Vp9 { .. } => "vp9dec",
        V::Av1 { .. } => "av1dec",
    }
}

fn bus_error(tag: &'static str) -> impl Fn(gst::glib::error::BoolError) -> WebCodecsError {
    move |e| WebCodecsError::unsupported("WebCodecsGstreamerInit", format!("{tag}: {e}"))
}

/// Error for a factory element that is not the concrete app interface we
/// require (`Cast::downcast` hands the original element back on failure).
fn downcast_error(what: &'static str) -> impl Fn(gst::Element) -> WebCodecsError {
    move |_| {
        WebCodecsError::unsupported(
            "WebCodecsGstreamerInit",
            format!("factory element is not {what}"),
        )
    }
}

/// Visible rows for one output plane of a planar YUV frame.
fn plane_visible_rows(plane: usize, height: u32, format: FrameFormat) -> u32 {
    match format {
        FrameFormat::I420 if plane > 0 => height.div_ceil(2),
        FrameFormat::I422 if plane > 0 => height.div_ceil(2),
        _ => height,
    }
}

/// Visible bytes-per-row for one output plane.
fn plane_visible_bpp_row(plane: usize, width: u32, format: FrameFormat) -> usize {
    match format {
        FrameFormat::I420 if plane > 0 => (width.div_ceil(2)) as usize,
        FrameFormat::I422 if plane > 0 => (width.div_ceil(2)) as usize,
        FrameFormat::Rgba | FrameFormat::Bgra => width as usize * 4,
        _ => width as usize,
    }
}

fn pts_micros(buffer: &gst::BufferRef) -> i64 {
    buffer.pts().map(|t| (t.nseconds() / 1000) as i64).unwrap_or(0)
}

fn duration_micros(buffer: &gst::BufferRef) -> Option<i64> {
    buffer.duration().map(|t| (t.nseconds() / 1000) as i64)
}

/// Convert one appsink sample into a plain [`DecodedVideoFrame`].
fn sample_to_frame(sample: &gst::Sample) -> Result<DecodedVideoFrame, WebCodecsError> {
    let Some(buffer) = sample.buffer() else {
        return Err(WebCodecsError::unsupported("WebCodecsDecodeError", "empty sample"));
    };
    let Some(caps) = sample.caps() else {
        return Err(WebCodecsError::unsupported("WebCodecsDecodeError", "no caps"));
    };
    let Ok(info) = gstreamer_video::VideoInfo::from_caps(caps) else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "caps are not video",
        ));
    };
    // Readable-map the sample buffer. The constructor lives on
    // `VideoFrameRef<&BufferRef>` (gstreamer-video 0.25.3 video_frame.rs:688);
    // `VideoFrame`/`VideoFrameRef` share the `VideoFrameExt` accessor surface.
    let frame = VideoFrameRef::from_buffer_ref_readable(buffer, &info).map_err(|_| {
        WebCodecsError::unsupported("WebCodecsDecodeError", "not video-mappable")
    })?;

    let format = match info.format() {
        gstreamer_video::VideoFormat::I420 | gstreamer_video::VideoFormat::Yv12 => FrameFormat::I420,
        gstreamer_video::VideoFormat::Y42b => FrameFormat::I422,
        gstreamer_video::VideoFormat::Y444 => FrameFormat::I444,
        gstreamer_video::VideoFormat::Rgba | gstreamer_video::VideoFormat::Rgbx => FrameFormat::Rgba,
        gstreamer_video::VideoFormat::Bgra | gstreamer_video::VideoFormat::Bgrx => FrameFormat::Bgra,
        other => {
            return Err(WebCodecsError::unsupported(
                "WebCodecsDecodeError",
                format!("appsink returned unsupported format {other:?}"),
            ));
        },
    };
    let coded_width = frame.width();
    let coded_height = frame.height();
    let n_planes = frame.n_planes();
    let mut planes = Vec::with_capacity(n_planes as usize);
    let mut strides = Vec::with_capacity(n_planes as usize);
    for p in 0..n_planes {
        let data = frame
            .plane_data(p)
            .map_err(|_| WebCodecsError::unsupported("WebCodecsDecodeError", "unmapped plane"))?;
        let stride = frame.plane_stride()[p as usize];
        let visible_rows = plane_visible_rows(p as usize, coded_height, format) as usize;
        let row_bytes = plane_visible_bpp_row(p as usize, coded_width, format);
        let mut packed = Vec::with_capacity(row_bytes * visible_rows);
        for row in 0..visible_rows {
            let start = row * stride as usize;
            let end = start + row_bytes;
            if end <= data.len() {
                packed.extend_from_slice(&data[start..end]);
            }
        }
        planes.push(packed);
        strides.push(row_bytes as u32);
    }

    let timestamp = pts_micros(buffer);
    let duration = duration_micros(buffer);

    Ok(DecodedVideoFrame {
        format,
        coded_width,
        coded_height,
        display_width: coded_width,
        display_height: coded_height,
        timestamp,
        duration,
        color_space: FrameColorSpace::default(),
        planes,
        strides,
    })
}
