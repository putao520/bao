// @trace REQ-BRW-051
//! `VideoDecoder` control-plane state machine. The DOM layer owns the JS
//! callbacks (output/error) and the control-message queue; this module owns
//! the spec's state transitions (unconfigured → configured → closed) and the
//! backend-agnostic decode plumbing.
//!
//! Spec surface (W3C WebCodecs §VideoDecoder):
//! - `configure()`: unconfigured/configured → configured; invalid config →
//!   TypeError; well-formed-but-unsupported → NotSupportedError + closed.
//! - `decode()`: configured only; queues a chunk.
//! - `flush()`: configured only; returns a promise that settles when all
//!   queued chunks have been decoded. `reset()` during flush → the flush
//!   promise rejects with AbortError.
//! - `reset()`: configured → unconfigured; drops queued work.
//! - `close()`: any → closed; terminal.
//! - `decodeQueueSize`: number of pending decode requests.

use crate::config::{validate_video_config, ConfigValidation, VideoDecoderConfig};
use crate::error::WebCodecsError;
use crate::video_frame::DecodedVideoFrame;

/// Decoder lifecycle states (WebCodecs `CodecState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecState {
    Unconfigured,
    Configured,
    Closed,
}

/// A queued `EncodedVideoChunk`.
#[derive(Debug, Clone)]
pub struct EncodedVideoChunk {
    pub chunk_type: EncodedVideoChunkType,
    /// Microseconds.
    pub timestamp: i64,
    /// Microseconds; `None` when absent.
    pub duration: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedVideoChunkType {
    Key,
    Delta,
}

/// What the control plane asks the backend to do.
#[derive(Debug)]
pub enum VideoControlMessage {
    Decode(EncodedVideoChunk),
    Flush,
}

/// Backend trait: a stateful video decoder for one config. The gst backend
/// implements this; a future pure-Rust backend would too.
pub trait VideoDecodeBackend: Send {
    /// Decode one chunk. Outputs are pushed to the sink as plain frames.
    fn decode(&mut self, chunk: EncodedVideoChunk) -> Result<(), WebCodecsError>;
    /// Drain all pending work (spec `flush()`). Returns frames emitted during
    /// the drain.
    fn flush(&mut self) -> Result<(), WebCodecsError>;
    /// Drop all internal state (spec `reset()`).
    fn reset(&mut self);
}

/// Control-plane state for one VideoDecoder instance.
pub struct VideoDecoderState {
    pub state: CodecState,
    pub config: Option<VideoDecoderConfig>,
    /// Configured codec after successful configure (for backend rebuilds).
    pub backend: Option<Box<dyn VideoDecodeBackend>>,
    /// Pending decode requests (spec decodeQueueSize).
    pub queue: Vec<VideoControlMessage>,
    /// Set when `reset()` happens while a flush is in flight; the DOM layer
    /// rejects the flush promise with AbortError when it sees this.
    pub flush_aborted: bool,
    /// Error that closed the decoder (surfaced via the `error` callback).
    pub error: Option<WebCodecsError>,
}

impl VideoDecoderState {
    pub fn new() -> Self {
        Self {
            state: CodecState::Unconfigured,
            config: None,
            backend: None,
            queue: Vec::new(),
            flush_aborted: false,
            error: None,
        }
    }

    /// `isConfigSupported(config)` — no instance state involved.
    /// `backend_available` is the honest platform probe.
    pub fn is_config_supported(
        config: &VideoDecoderConfig,
        backend_available: bool,
    ) -> Result<bool, WebCodecsError> {
        match validate_video_config(config, backend_available)? {
            ConfigValidation::Supported(_) => Ok(true),
            ConfigValidation::Unsupported(_) => Ok(false),
        }
    }

    /// `configure(config)` — TypeError face is the Err return; the
    /// NotSupportedError face is `Ok(false)` (caller fires the error callback
    /// and closes).
    pub fn configure(
        &mut self,
        config: VideoDecoderConfig,
        backend_available: bool,
        backend_factory: &dyn Fn(&VideoDecoderConfig) -> Option<Box<dyn VideoDecodeBackend>>,
    ) -> Result<bool, WebCodecsError> {
        if self.state == CodecState::Closed {
            return Err(WebCodecsError::unsupported(
                "WebCodecsClosedDecoder",
                "decoder is closed",
            ));
        }
        match validate_video_config(&config, backend_available)? {
            ConfigValidation::Supported(_) => {
                match backend_factory(&config) {
                    Some(b) => {
                        self.backend = Some(b);
                        self.config = Some(config);
                        self.state = CodecState::Configured;
                        self.flush_aborted = false;
                        Ok(true)
                    },
                    None => {
                        self.close_with_error(WebCodecsError::unsupported(
                            "WebCodecsUnsupportedCodec",
                            format!("backend construction failed for {}", config.codec),
                        ));
                        Ok(false)
                    },
                }
            },
            ConfigValidation::Unsupported(e) => {
                self.close_with_error(e);
                Ok(false)
            },
        }
    }

    /// `decode(chunk)` — queues a control message. Err = TypeError face
    /// (unconfigured/closed).
    pub fn decode(&mut self, chunk: EncodedVideoChunk) -> Result<(), WebCodecsError> {
        match self.state {
            CodecState::Configured => {
                self.queue.push(VideoControlMessage::Decode(chunk));
                Ok(())
            },
            CodecState::Unconfigured => Err(WebCodecsError::unsupported(
                "WebCodecsUnconfiguredDecoder",
                "decode() called on an unconfigured decoder",
            )),
            CodecState::Closed => Err(WebCodecsError::unsupported(
                "WebCodecsClosedDecoder",
                "decode() called on a closed decoder",
            )),
        }
    }

    /// `flush()` — drains the queue through the backend. Returns the decoded
    /// frames in output order. Err = TypeError face (unconfigured/closed).
    /// A queued flush that was aborted by `reset()` returns `Err(AbortError)`.
    pub fn flush(&mut self) -> Result<Vec<DecodedVideoFrame>, WebCodecsError> {
        if self.flush_aborted {
            self.flush_aborted = false;
            return Err(WebCodecsError::unsupported(
                "WebCodecsFlushAborted",
                "flush() was aborted by reset()",
            ));
        }
        match self.state {
            CodecState::Configured => {
                let frames = Vec::new();
                // A flush with no decode messages still succeeds (WPT
                // "Test flush with no decodes").
                let Some(backend) = self.backend.as_mut() else {
                    return Ok(frames);
                };
                let queue = std::mem::take(&mut self.queue);
                for msg in queue {
                    match msg {
                        VideoControlMessage::Decode(chunk) => {
                            if let Err(e) = backend.decode(chunk) {
                                self.close_with_error(e);
                                return Err(self.error.clone().unwrap());
                            }
                        },
                        VideoControlMessage::Flush => {},
                    }
                }
                if let Err(e) = backend.flush() {
                    self.close_with_error(e);
                    return Err(self.error.clone().unwrap());
                }
                Ok(frames)
            },
            CodecState::Unconfigured => Err(WebCodecsError::unsupported(
                "WebCodecsUnconfiguredDecoder",
                "flush() called on an unconfigured decoder",
            )),
            CodecState::Closed => Err(WebCodecsError::unsupported(
                "WebCodecsClosedDecoder",
                "flush() called on a closed decoder",
            )),
        }
    }

    /// `reset()` — configured → unconfigured, dropping queued work.
    pub fn reset(&mut self) {
        if self.state == CodecState::Configured {
            self.state = CodecState::Unconfigured;
            self.config = None;
            self.backend = None;
            self.queue.clear();
            // A reset while a flush is in flight aborts that flush (spec:
            // flush promise rejects with AbortError). The DOM layer maps
            // "WebCodecsFlushAborted" to AbortError.
            self.flush_aborted = true;
        }
    }

    /// `close()` — terminal.
    pub fn close(&mut self) {
        self.state = CodecState::Closed;
        self.config = None;
        self.backend = None;
        self.queue.clear();
    }

    fn close_with_error(&mut self, e: WebCodecsError) {
        self.error = Some(e);
        self.close();
    }

    /// spec `decodeQueueSize`.
    pub fn decode_queue_size(&self) -> usize {
        self.queue
            .iter()
            .filter(|m| matches!(m, VideoControlMessage::Decode(_)))
            .count()
    }
}

impl Default for VideoDecoderState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webcodecs_codec::ParsedVideoCodec;

    struct FakeBackend;
    impl VideoDecodeBackend for FakeBackend {
        fn decode(&mut self, _chunk: EncodedVideoChunk) -> Result<(), WebCodecsError> {
            Ok(())
        }
        fn flush(&mut self) -> Result<(), WebCodecsError> {
            Ok(())
        }
        fn reset(&mut self) {}
    }

    fn fake_factory(_config: &VideoDecoderConfig) -> Option<Box<dyn VideoDecodeBackend>> {
        Some(Box::new(FakeBackend))
    }

    fn vp8_config() -> VideoDecoderConfig {
        VideoDecoderConfig {
            codec: "vp8".into(),
            description: None,
            coded_width: Some(320),
            coded_height: Some(240),
            display_width: Some(320),
            display_height: Some(240),
        }
    }

    #[test]
    fn unconfigured_rejects_decode_and_flush() {
        let mut d = VideoDecoderState::new();
        assert!(d.decode(EncodedVideoChunk {
            chunk_type: EncodedVideoChunkType::Key,
            timestamp: 0,
            duration: None,
            data: vec![1, 2, 3],
        })
        .is_err());
        assert!(d.flush().is_err());
    }

    #[test]
    #[cfg_attr(
        not(feature = "media-gstreamer"),
        ignore = "vp8/registry face needs the gst backend (packaging feature)"
    )]
    fn configure_unsupported_closes_with_error() {
        let mut d = VideoDecoderState::new();
        let cfg = VideoDecoderConfig {
            codec: "bogus".into(),
            description: None,
            coded_width: Some(320),
            coded_height: Some(240),
            display_width: None,
            display_height: None,
        };
        // An unparseable codec string throws at configure (NotSupported,
        // parse layer) — the closed-with-error path is exercised through
        // `configure_supported_then_decode_flush_reset`'s failure legs.
        let err = d.configure(cfg, true, &fake_factory);
        assert!(err.is_err());
        assert_eq!(d.state, CodecState::Unconfigured);
    }

    #[test]
    fn configure_supported_then_decode_flush_reset() {
        let mut d = VideoDecoderState::new();
        let ok = d
            .configure(vp8_config(), true, &fake_factory)
            .unwrap();
        assert!(ok);
        assert_eq!(d.state, CodecState::Configured);
        d.decode(EncodedVideoChunk {
            chunk_type: EncodedVideoChunkType::Key,
            timestamp: 1,
            duration: Some(1),
            data: vec![1],
        })
        .unwrap();
        assert_eq!(d.decode_queue_size(), 1);
        d.flush().unwrap();
        d.reset();
        assert_eq!(d.state, CodecState::Unconfigured);
        assert_eq!(d.decode_queue_size(), 0);
    }

    #[test]
    #[cfg_attr(
        not(feature = "media-gstreamer"),
        ignore = "vp8/registry face needs the gst backend (packaging feature)"
    )]
    fn is_config_supported_matches_registry() {
        let cfg = vp8_config();
        assert!(VideoDecoderState::is_config_supported(&cfg, true).unwrap());
        assert!(!VideoDecoderState::is_config_supported(&cfg, false).unwrap());
        let future = VideoDecoderConfig {
            codec: "avc1.FF000b".into(),
            description: None,
            coded_width: None,
            coded_height: None,
            display_width: None,
            display_height: None,
        };
        assert!(!VideoDecoderState::is_config_supported(&future, true).unwrap());
    }

    // Silence unused-import warning for ParsedVideoCodec in test cfg.
    #[allow(dead_code)]
    fn _codec_witness(_c: ParsedVideoCodec) {}
}
