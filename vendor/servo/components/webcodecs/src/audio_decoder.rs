// @trace REQ-BRW-051
//! `AudioDecoder` control-plane state machine (mirrors video_decoder.rs).
//!
//! Codec-specific configure validation lives in config.rs (opus multichannel
//! needs >=10-byte description, vorbis/flac need a description). PCM passthrough
//! is exercised in the gst audio wave via the configured sample format.

use crate::audio_data::DecodedAudioFrame;
use crate::config::{validate_audio_config, ConfigValidation, AudioDecoderConfig};
use crate::error::WebCodecsError;
use crate::video_decoder::CodecState;

/// A queued `EncodedAudioChunk`.
#[derive(Debug, Clone)]
pub struct EncodedAudioChunk {
    /// Microseconds.
    pub timestamp: i64,
    /// Microseconds; `None` when absent.
    pub duration: Option<i64>,
    pub data: Vec<u8>,
}

/// What the control plane asks the backend to do.
#[derive(Debug)]
pub enum AudioControlMessage {
    Decode(EncodedAudioChunk),
}

/// Backend trait: stateful audio decoder for one config.
pub trait AudioDecodeBackend: Send {
    fn decode(&mut self, chunk: EncodedAudioChunk) -> Result<(), WebCodecsError>;
    /// Drain (spec `flush()`); the backend emits frames to the sink.
    fn flush(&mut self) -> Result<(), WebCodecsError>;
    fn reset(&mut self);
}

/// Control-plane state for one AudioDecoder instance.
pub struct AudioDecoderState {
    pub state: CodecState,
    pub config: Option<AudioDecoderConfig>,
    pub backend: Option<Box<dyn AudioDecodeBackend>>,
    pub queue: Vec<AudioControlMessage>,
    pub flush_aborted: bool,
    pub error: Option<WebCodecsError>,
}

impl AudioDecoderState {
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

    /// `isConfigSupported(config)`.
    pub fn is_config_supported(
        config: &AudioDecoderConfig,
        backend_available: bool,
    ) -> Result<bool, WebCodecsError> {
        match validate_audio_config(config, backend_available)? {
            ConfigValidation::Supported(_) => Ok(true),
            ConfigValidation::Unsupported(_) => Ok(false),
        }
    }

    /// `configure(config)`. Same contract as video_decoder::configure.
    pub fn configure(
        &mut self,
        config: AudioDecoderConfig,
        backend_available: bool,
        backend_factory: &dyn Fn(&AudioDecoderConfig) -> Option<Box<dyn AudioDecodeBackend>>,
    ) -> Result<bool, WebCodecsError> {
        if self.state == CodecState::Closed {
            return Err(WebCodecsError::unsupported(
                "WebCodecsClosedDecoder",
                "decoder is closed",
            ));
        }
        match validate_audio_config(&config, backend_available)? {
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

    pub fn decode(&mut self, chunk: EncodedAudioChunk) -> Result<(), WebCodecsError> {
        match self.state {
            CodecState::Configured => {
                self.queue.push(AudioControlMessage::Decode(chunk));
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

    pub fn flush(&mut self) -> Result<Vec<DecodedAudioFrame>, WebCodecsError> {
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
                let Some(backend) = self.backend.as_mut() else {
                    return Ok(frames);
                };
                let queue = std::mem::take(&mut self.queue);
                for msg in queue {
                    match msg {
                        AudioControlMessage::Decode(chunk) => {
                            if let Err(e) = backend.decode(chunk) {
                                self.close_with_error(e);
                                return Err(self.error.clone().unwrap());
                            }
                        },
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

    pub fn reset(&mut self) {
        if self.state == CodecState::Configured {
            self.state = CodecState::Unconfigured;
            self.config = None;
            self.backend = None;
            self.queue.clear();
            self.flush_aborted = true;
        }
    }

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

    pub fn decode_queue_size(&self) -> usize {
        self.queue.len()
    }
}

impl Default for AudioDecoderState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend;
    impl AudioDecodeBackend for FakeBackend {
        fn decode(&mut self, _chunk: EncodedAudioChunk) -> Result<(), WebCodecsError> {
            Ok(())
        }
        fn flush(&mut self) -> Result<(), WebCodecsError> {
            Ok(())
        }
        fn reset(&mut self) {}
    }

    fn fake_factory(_config: &AudioDecoderConfig) -> Option<Box<dyn AudioDecodeBackend>> {
        Some(Box::new(FakeBackend))
    }

    fn opus_config(channels: f64, description: Option<Vec<u8>>) -> AudioDecoderConfig {
        AudioDecoderConfig {
            codec: "opus".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(channels),
            description,
        }
    }

    #[test]
    fn missing_sample_rate_is_syntax_error() {
        let cfg = AudioDecoderConfig {
            codec: "opus".into(),
            sample_rate: None,
            number_of_channels: Some(2.0),
            description: None,
        };
        assert!(AudioDecoderState::is_config_supported(&cfg, true).is_err());
    }

    #[test]
    fn opus_multichannel_needs_description() {
        let cfg = opus_config(3.0, None);
        // >2ch opus without a description throws (WebCodecsInvalidDescription)
        // — the honest gate is the error face, not `supported=false`.
        assert!(AudioDecoderState::is_config_supported(&cfg, true).is_err());
        let too_short = opus_config(3.0, Some(vec![0u8; 9]));
        // A <10-byte OpusHead description is an invalid-description *throw*
        // (WebCodecsInvalidDescription), not `supported=false`.
        assert!(AudioDecoderState::is_config_supported(&too_short, true).is_err());
        let ok = opus_config(3.0, Some(vec![0u8; 19]));
        assert!(AudioDecoderState::is_config_supported(&ok, true).unwrap());
        // Stereo without description is fine.
        let stereo = opus_config(2.0, None);
        assert!(AudioDecoderState::is_config_supported(&stereo, true).unwrap());
    }

    #[test]
    fn vorbis_requires_description() {
        let cfg = AudioDecoderConfig {
            codec: "vorbis".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(2.0),
            description: None,
        };
        // Vorbis without a description is a *throw* (NotSupported,
        // WebCodecsMissingDescription) per the spec's description-required
        // clause — not a `supported=false` return.
        assert!(AudioDecoderState::is_config_supported(&cfg, true).is_err());
    }

    #[test]
    fn configure_supported_then_flush_reset() {
        let mut d = AudioDecoderState::new();
        let ok = d
            .configure(opus_config(2.0, None), true, &fake_factory)
            .unwrap();
        assert!(ok);
        d.decode(EncodedAudioChunk {
            timestamp: 0,
            duration: None,
            data: vec![1, 2, 3],
        })
        .unwrap();
        assert_eq!(d.decode_queue_size(), 1);
        d.flush().unwrap();
        assert_eq!(d.decode_queue_size(), 0);
        d.reset();
        assert_eq!(d.state, CodecState::Unconfigured);
    }
}
