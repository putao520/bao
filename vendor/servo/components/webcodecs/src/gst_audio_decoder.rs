// @trace REQ-BRW-051
//! GStreamer audio decode backend (feature `media-gstreamer`, Wave B): one
//! `appsrc → <decoder> → audioconvert → audioresample → appsink` pipeline per
//! configured codec.
//!
//! Pipeline shapes verified against the in-tree gstreamer-audio 0.25.3 registry
//! sources and the local GStreamer 1.24.2 runtime (pad templates + an
//! appsrc→decoder→appsink run; see the module comments on `source_caps` and
//! `output_caps` for the exact rows):
//! - opus: `appsrc(caps=audio/x-opus,channel-mapping-family=0) ! opusdec
//!   ! audioconvert ! audioresample ! appsink(caps=audio/x-raw,
//!   format=F32LE,layout=interleaved,rate=R,channels=C)`.
//! - mp4a (AAC): `appsrc(caps=audio/mpeg,mpegversion=4,stream-format=raw,
//!   codec_data=<AudioSpecificConfig>)` (or `stream-format=adts` when the
//!   config carries no description) `! avdec_aac ! audioconvert
//!   ! audioresample ! appsink(...)`.
//!
//! Honest capability probe: [`GstreamerAudioDecoderBackend::is_available_for`]
//! checks the decoder *element* exists at configure time (the same
//! element-presence contract as the video backend) — nothing is fabricated.
//! Codecs without a verified element mapping in this wave (vorbis/flac/mp3)
//! report no backend: their decoder elements exist on many hosts, but mapping
//! their caps/description contracts is not part of this slice, so claiming
//! support would be a capability lie.
//!
//! Timestamps: `EncodedAudioChunk.timestamp` is written to the appsrc buffer
//! PTS (microseconds, `ClockTime::from_useconds`) and read back from the
//! decoder's output buffer PTS; with no reordering in audio decoding the first
//! output buffer of a chunk carries exactly the chunk timestamp. Verified on
//! the local runtime: pushing packets with PTS 5_000_000 µs yields decoded
//! buffers starting at 5_000_000 µs at 20 ms cadence (opus measured 48 kHz
//! stereo; AAC 48 kHz stereo).
//!
//! Output shape: the appsink caps narrow every decoder to interleaved
//! `F32LE` at the configured sample rate/channels (`audioconvert` +
//! `audioresample` do the conversion; opusdec emits `S16LE` interleaved and
//! avdec_aac emits `F32LE` non-interleaved on this runtime, so the chain is
//! required, not decorative). The component frames therefore always carry
//! [`AudioSampleFormat::F32`] with a single interleaved plane — the DOM layer
//! wraps them in `AudioData` (`AudioData::from_decoded_frame`).
//!
//! Decode failure contract: a GStreamer `error` bus message →
//! [`WebCodecsError`] → the control plane closes the decoder, fires `error`,
//! and rejects the in-flight flush (WPT "corrupt chunk → error()"). An `eos`
//! bus message marks the deterministic drain point; the pipeline restarts for
//! the next round (WPT decodes twice on one decoder).

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_audio::{AudioFormat, AudioInfo, AudioLayout};

use crate::audio_data::{AudioSampleFormat, DecodedAudioFrame};
use crate::audio_decoder::EncodedAudioChunk;
use crate::config::AudioDecoderConfig;
use crate::error::WebCodecsError;
use crate::webcodecs_codec::{gst_audio_decoder_element, ParsedAudioCodec, ParsedCodec};

/// Backend trait boundary used by the DOM layer (kept local so the
/// `media-gstreamer` feature fully owns the gst dependency; mirrors
/// `gst_video_decoder::VideoDecoderBackend`).
pub trait AudioDecoderBackend: Send {
    /// Honest capability probe: is there a decoder element for this config's
    /// codec on this platform?
    fn is_available_for(config: &AudioDecoderConfig) -> bool
    where
        Self: Sized;
    /// Build a backend for a validated config.
    fn new(config: &AudioDecoderConfig) -> Result<Self, WebCodecsError>
    where
        Self: Sized;
    fn decode(&mut self, chunk: EncodedAudioChunk) -> Result<(), WebCodecsError>;
    /// Drain: pull every produced frame out of appsink.
    fn flush(&mut self) -> Result<Vec<DecodedAudioFrame>, WebCodecsError>;
    fn reset(&mut self);
}

/// The gst-backed implementation handle.
pub struct GstreamerAudioDecoderBackend {
    inner: Pipeline,
}

impl AudioDecoderBackend for GstreamerAudioDecoderBackend {
    fn is_available_for(config: &AudioDecoderConfig) -> bool {
        is_available_for(config)
    }

    fn new(config: &AudioDecoderConfig) -> Result<Self, WebCodecsError> {
        Ok(Self {
            inner: Pipeline::new(config)?,
        })
    }

    fn decode(&mut self, chunk: EncodedAudioChunk) -> Result<(), WebCodecsError> {
        self.inner.push_chunk(chunk)
    }

    fn flush(&mut self) -> Result<Vec<DecodedAudioFrame>, WebCodecsError> {
        self.inner.drain()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// Parse the codec string and return the verified decoder element family.
fn parsed_codec(config: &AudioDecoderConfig) -> Option<ParsedAudioCodec> {
    let Ok(ParsedCodec::Audio(audio)) = ParsedCodec::parse(&config.codec) else {
        return None;
    };
    Some(audio)
}

/// Honest probe: parsed codec + a verified element mapping + the element
/// exists after `gst::init()`.
///
/// Registered boundary (Wave B): multichannel Opus (>2 channels) is *not*
/// claimed. Opus channels beyond stereo require an `OpusHead` channel-mapping
/// table forwarded as the `channel-mapping` caps array; a local-runtime probe
/// (4-channel opusenc output, mapping family 255) did not negotiate even with
/// `channel-mapping-family`/`stream-count`/`coupled-count`/`channel-mapping`
/// supplied (`not-negotiated`), so the caps shape is unverified and support is
/// withheld rather than guessed.
pub fn is_available_for(config: &AudioDecoderConfig) -> bool {
    let Some(audio) = parsed_codec(config) else {
        return false;
    };
    let Some(element) = gst_audio_decoder_element(&audio) else {
        return false;
    };
    if matches!(audio, ParsedAudioCodec::Opus) &&
        config.number_of_channels.is_some_and(|c| c > 2.0)
    {
        return false;
    }
    if gst::init().is_err() {
        return false;
    }
    gst::ElementFactory::find(element).is_some()
}

/// Build the source (encoded-stream) caps for the configured codec.
///
/// Field rows are taken from the local runtime's pad templates
/// (`gst-inspect-1.0 opusdec` / `avdec_aac`):
/// - opusdec sink: `audio/x-opus` with `channel-mapping-family: 0` for the
///   mono/stereo mapping family, or `channel-mapping-family: [1,255]` plus
///   `channels`/`stream-count`/`coupled-count` for mapped multichannel.
/// - avdec_aac sink: `audio/mpeg, mpegversion: {2,4}, stream-format:
///   {raw,adts,adif}`; raw AAC requires the `codec_data` (AudioSpecificConfig)
///   that WebCodecs supplies as the decoder config `description`.
pub fn source_caps(config: &AudioDecoderConfig) -> Result<gst::Caps, WebCodecsError> {
    let Some(audio) = parsed_codec(config) else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("{} is not a supported audio codec string", config.codec),
        ));
    };
    match audio {
        ParsedAudioCodec::Opus => {
            let channels = config.number_of_channels.unwrap_or(0.0);
            if !(1.0..=2.0).contains(&channels) {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsUnsupportedCodec",
                    format!("opus channel count {channels} is outside the verified 1-2 range"),
                ));
            }
            let mut builder = gst::Caps::builder("audio/x-opus");
            builder = builder.field("channel-mapping-family", 0i32);
            Ok(builder.build())
        },
        ParsedAudioCodec::Aac { .. } => {
            // WebCodecs Codec Registry (AAC): a `description` is the
            // AudioSpecificConfig (raw payload); without one the chunks carry
            // ADTS headers.
            let mut builder = gst::Caps::builder("audio/mpeg");
            builder = builder.field("mpegversion", 4i32);
            match config.description.as_ref() {
                Some(description) if !description.is_empty() => {
                    builder = builder
                        .field("stream-format", "raw")
                        .field("codec_data", gst::Buffer::from_slice(description.clone()));
                },
                _ => {
                    builder = builder.field("stream-format", "adts");
                },
            }
            Ok(builder.build())
        },
        ParsedAudioCodec::Vorbis | ParsedAudioCodec::Flac | ParsedAudioCodec::Mp3 => {
            Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst audio decode caps for {}", config.codec),
            ))
        },
    }
}

/// Requested decoded output caps: interleaved little-endian `f32` at the
/// configured geometry. `AudioFormat::F32le` (not the native
/// `AUDIO_FORMAT_F32` alias) is deliberate: the DOM PCM helpers
/// (`webcodecs_pcm`) serialize with `f32::to_le_bytes`. The spelling matters
/// for negotiation as well: the local runtime's `audioconvert`/`audioresample`
/// template rows list `F32LE`/`F32BE` only — a bare `F32` caps string does not
/// link (verified with `gst-launch-1.0`).
fn output_caps(sample_rate: u32, channels: u32) -> gst::Caps {
    gstreamer_audio::AudioCapsBuilder::new()
        .format(AudioFormat::F32le)
        .layout(AudioLayout::Interleaved)
        .rate(sample_rate as i32)
        .channels(channels as i32)
        .build()
}

/// Configured geometry, narrowed into the caps `i32`/`u32` surface.
fn geometry(config: &AudioDecoderConfig) -> Result<(u32, u32), WebCodecsError> {
    let rate = config.sample_rate.unwrap_or(0.0);
    let channels = config.number_of_channels.unwrap_or(0.0);
    if !(1.0..=i32::MAX as f64).contains(&rate) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroSampleRate",
            format!("sampleRate {rate} is outside the gst caps range"),
        ));
    }
    if !(1.0..=i32::MAX as f64).contains(&channels) {
        return Err(WebCodecsError::syntax(
            "WebCodecsZeroChannels",
            format!("numberOfChannels {channels} is outside the gst caps range"),
        ));
    }
    Ok((rate as u32, channels as u32))
}

/// PTS epoch: GStreamer timestamps are unsigned while WebCodecs timestamps are
/// signed (`EncodedAudioChunk.timestamp` is a `long long`; the WPT decoder
/// oracle drives negative timestamps). Every pushed PTS carries this fixed
/// offset, which is subtracted again on the way out, so the full `i64` range
/// (down to `-PTS_EPOCH_US`) round-trips exactly instead of being folded to its
/// magnitude by `unsigned_abs`.
const PTS_EPOCH_US: u64 = 1_000_000_000_000;

/// WebCodecs microseconds → GStreamer `ClockTime` microseconds (epoch-shifted).
fn to_gst_pts(timestamp: i64) -> u64 {
    let shifted = PTS_EPOCH_US as i128 + timestamp as i128;
    shifted.clamp(0, u64::MAX as i128) as u64
}

/// Buffer PTS → WebCodecs microseconds (epoch removed). A buffer without a PTS
/// reads as 0, matching the video backend's `pts_micros` convention.
fn from_gst_pts(buffer: &gst::BufferRef) -> i64 {
    buffer
        .pts()
        .map(|t| (t.nseconds() / 1000) as i64 - PTS_EPOCH_US as i64)
        .unwrap_or(0)
}

/// One configured decoder pipeline.
struct Pipeline {
    appsrc: AppSrc,
    appsink: AppSink,
    pipeline: gst::Pipeline,
    bus: gst::Bus,
}

impl Pipeline {
    fn new(config: &AudioDecoderConfig) -> Result<Self, WebCodecsError> {
        gst::init().map_err(|e| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", format!("gst::init: {e}"))
        })?;
        if !is_available_for(config) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst audio decoder for {}", config.codec),
            ));
        }
        let Some(audio) = parsed_codec(config) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("{} is not an audio codec", config.codec),
            ));
        };
        let decoder_element = gst_audio_decoder_element(&audio).ok_or_else(|| {
            WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst audio decoder element for {}", config.codec),
            )
        })?;
        let (sample_rate, channels) = geometry(config)?;
        let src_caps = source_caps(config)?;

        let src = gst::ElementFactory::make("appsrc")
            .build()
            .map_err(bus_error("appsrc"))?;
        let dec = gst::ElementFactory::make(decoder_element)
            .build()
            .map_err(bus_error(decoder_element))?;
        let convert = gst::ElementFactory::make("audioconvert")
            .build()
            .map_err(bus_error("audioconvert"))?;
        let resample = gst::ElementFactory::make("audioresample")
            .build()
            .map_err(bus_error("audioresample"))?;
        let sink = gst::ElementFactory::make("appsink")
            .build()
            .map_err(bus_error("appsink"))?;

        // `ElementFactory::make` yields a plain `gst::Element`; the concrete
        // element interface is reached by downcast (same shape as the
        // servo-media gstreamer backend precedent).
        let appsrc = src
            .clone()
            .downcast::<AppSrc>()
            .map_err(downcast_error("appsrc"))?;
        let appsink = sink
            .clone()
            .downcast::<AppSink>()
            .map_err(downcast_error("appsink"))?;

        appsink.set_caps(Some(&output_caps(sample_rate, channels)));
        appsink.set_max_buffers(0);
        appsink.set_drop(false);
        // Non-realtime decoding: never pace delivery on the clock (the
        // servo-media gstreamer audio backend sets the same property).
        appsink.set_sync(false);

        let pipeline = gst::Pipeline::new();
        pipeline
            .add_many([&src, &dec, &convert, &resample, &sink])
            .map_err(bus_error("add_many"))?;
        gst::Element::link_many([&src, &dec, &convert, &resample, &sink])
            .map_err(bus_error("link_many"))?;

        let bus = pipeline.bus().ok_or_else(|| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", "pipeline has no bus")
        })?;

        appsrc.set_caps(Some(&src_caps));
        appsrc.set_format(gst::Format::Time);
        // The script thread drains only at flush(); never let a full internal
        // queue turn a push into a block (0 = unlimited, appsrc property docs).
        appsrc.set_max_bytes(0);
        pipeline.set_state(gst::State::Playing).map_err(|_| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", "pipeline did not start")
        })?;

        Ok(Self {
            appsrc,
            appsink,
            pipeline,
            bus,
        })
    }

    /// Feed one encoded chunk to the decoder.
    fn push_chunk(&mut self, chunk: EncodedAudioChunk) -> Result<(), WebCodecsError> {
        self.poll_bus_for_errors()?;
        let mut buffer = gst::Buffer::from_slice(chunk.data);
        {
            let buf = buffer.get_mut().expect("fresh buffer is writable");
            buf.set_pts(gst::ClockTime::from_useconds(to_gst_pts(chunk.timestamp)));
            if let Some(d) = chunk.duration {
                buf.set_duration(gst::ClockTime::from_useconds(d.unsigned_abs()));
            }
        }
        self.appsrc
            .push_buffer(buffer)
            .map_err(|_| self.take_bus_error())?;
        Ok(())
    }

    /// Spec `flush()`: send EOS, drain every produced frame from appsink until
    /// the bus reports the matching `eos`, then restart the pipeline.
    fn drain(&mut self) -> Result<Vec<DecodedAudioFrame>, WebCodecsError> {
        self.appsrc
            .end_of_stream()
            .map_err(|_| self.take_bus_error())?;
        let mut frames = Vec::new();
        loop {
            // Non-blocking pull: a produced frame returns Some immediately.
            while let Some(sample) = self
                .appsink
                .try_pull_sample(gst::ClockTime::from_mseconds(1))
            {
                frames.push(sample_to_frame(&sample)?);
            }
            match self.bus.timed_pop_filtered(
                Some(gst::ClockTime::from_mseconds(50)),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
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
                    // draining (audio blocks can be mid-transit).
                    continue;
                },
            }
        }
        // Restart for the next round (WPT decodes twice with one decoder).
        self.pipeline.set_state(gst::State::Playing).map_err(|_| self.take_bus_error())?;
        Ok(frames)
    }

    /// Spec `reset()`: stop the pipeline and drop queued state.
    fn reset(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
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

fn pts_micros(buffer: &gst::BufferRef) -> i64 {
    from_gst_pts(buffer)
}

/// Map the negotiated sample format onto the component surface. The pipeline
/// requests interleaved `F32LE`, so the F32 row is the expected one; the other
/// rows are accepted honestly should a host negotiate a different narrow
/// format (the DOM layer converts into `AudioData` without loss).
fn sample_format_for(info: &AudioInfo) -> Result<AudioSampleFormat, WebCodecsError> {
    match info.format() {
        AudioFormat::F32le => Ok(AudioSampleFormat::F32),
        AudioFormat::S32le => Ok(AudioSampleFormat::S32),
        AudioFormat::S16le => Ok(AudioSampleFormat::S16),
        AudioFormat::U8 => Ok(AudioSampleFormat::U8),
        other => Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            format!("appsink returned unsupported audio format {other:?}"),
        )),
    }
}

/// Convert one appsink sample into a plain [`DecodedAudioFrame`].
fn sample_to_frame(sample: &gst::Sample) -> Result<DecodedAudioFrame, WebCodecsError> {
    let Some(buffer) = sample.buffer() else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "empty sample",
        ));
    };
    let Some(caps) = sample.caps() else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "no caps",
        ));
    };
    let info = AudioInfo::from_caps(caps).map_err(|_| {
        WebCodecsError::unsupported("WebCodecsDecodeError", "caps are not raw audio")
    })?;
    if info.layout() != AudioLayout::Interleaved {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "negotiated audio layout is not interleaved",
        ));
    }
    let format = sample_format_for(&info)?;
    let bpf = info.bpf() as usize; // bytes per frame across all channels
    if bpf == 0 {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "audio caps report zero bytes per frame",
        ));
    }
    let map = buffer.map_readable().map_err(|_| {
        WebCodecsError::unsupported("WebCodecsDecodeError", "buffer is not readable")
    })?;
    let bytes = map.as_slice();
    let number_of_frames = (bytes.len() / bpf) as u64;
    if number_of_frames == 0 {
        return Err(WebCodecsError::unsupported(
            "WebCodecsDecodeError",
            "decoded sample carries no complete frame",
        ));
    }
    Ok(DecodedAudioFrame {
        format,
        sample_rate: f64::from(info.rate()),
        number_of_frames,
        number_of_channels: u64::from(info.channels()),
        timestamp: pts_micros(buffer),
        data: vec![bytes.to_vec()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus_config(channels: f64, description: Option<Vec<u8>>) -> AudioDecoderConfig {
        AudioDecoderConfig {
            codec: "opus".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(channels),
            description,
        }
    }

    fn aac_config(description: Option<Vec<u8>>) -> AudioDecoderConfig {
        AudioDecoderConfig {
            codec: "mp4a.40.2".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(2.0),
            description,
        }
    }

    fn element_present(element: &str) -> bool {
        if gst::init().is_err() {
            return false;
        }
        gst::ElementFactory::find(element).is_some()
    }

    #[test]
    fn opus_source_caps_is_mapping_family_zero() {
        gst::init().unwrap();
        let caps = source_caps(&opus_config(2.0, None)).unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.name(), "audio/x-opus");
        assert_eq!(s.get::<i32>("channel-mapping-family").unwrap(), 0);
    }

    #[test]
    fn opus_multichannel_is_not_claimed() {
        // Registered boundary: >2 channel opus needs the channel-mapping caps
        // array whose shape is unverified on this runtime.
        assert!(source_caps(&opus_config(4.0, Some(vec![0u8; 21]))).is_err());
        assert!(!is_available_for(&opus_config(4.0, Some(vec![0u8; 21]))));
    }

    #[test]
    fn aac_source_caps_uses_raw_plus_codec_data() {
        gst::init().unwrap();
        let description = vec![0x11, 0x90, 0x56, 0xe5, 0x00];
        let caps = source_caps(&aac_config(Some(description.clone()))).unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.name(), "audio/mpeg");
        assert_eq!(s.get::<i32>("mpegversion").unwrap(), 4);
        assert_eq!(s.get::<&str>("stream-format").unwrap(), "raw");
        assert!(s.has_field("codec_data"));
        let codec_data = s.get::<gst::Buffer>("codec_data").unwrap();
        let map = codec_data.map_readable().unwrap();
        assert_eq!(map.as_slice(), description.as_slice());
    }

    #[test]
    fn aac_without_description_is_adts() {
        gst::init().unwrap();
        let caps = source_caps(&aac_config(None)).unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.get::<&str>("stream-format").unwrap(), "adts");
        assert!(!s.has_field("codec_data"));
    }

    #[test]
    fn output_caps_spell_the_interleaved_f32le_row() {
        gst::init().unwrap();
        let caps = output_caps(48000, 2);
        let s = caps.structure(0).unwrap();
        assert_eq!(s.name(), "audio/x-raw");
        assert_eq!(s.get::<&str>("layout").unwrap(), "interleaved");
        assert_eq!(s.get::<i32>("rate").unwrap(), 48000);
        assert_eq!(s.get::<i32>("channels").unwrap(), 2);
        // The local audioresample/audioconvert templates list F32LE (not the
        // native "F32" alias), so the explicit LE row is the negotiating one.
        assert_eq!(s.get::<&str>("format").unwrap(), "F32LE");
    }

    #[test]
    fn unsupported_codecs_have_no_backend() {
        let vorbis = AudioDecoderConfig {
            codec: "vorbis".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(2.0),
            description: Some(vec![0u8; 32]),
        };
        assert!(source_caps(&vorbis).is_err());
        assert!(!is_available_for(&vorbis));
    }

    #[test]
    fn probe_matches_element_presence() {
        if !element_present("opusdec") {
            return;
        }
        assert!(is_available_for(&opus_config(2.0, None)));
        if element_present("avdec_aac") {
            assert!(is_available_for(&aac_config(None)));
        }
    }

    #[test]
    fn negative_timestamps_round_trip_through_the_pts_epoch() {
        gst::init().unwrap();
        for timestamp in [-42i64, -10_000, 0, 42, 5_000_000] {
            let mut buffer = gst::Buffer::new();
            {
                let buf = buffer.get_mut().expect("fresh buffer is writable");
                buf.set_pts(gst::ClockTime::from_useconds(to_gst_pts(timestamp)));
            }
            assert_eq!(
                from_gst_pts(&buffer),
                timestamp,
                "PTS epoch round-trip lost the sign for {timestamp}"
            );
        }
    }

    /// Real decode roundtrip: opusenc packet stream -> opusdec -> F32 frames,
    /// asserting the chunk timestamp is carried through unchanged (the
    /// WebCodecs timestamp contract). Skips on hosts without the two elements.
    #[test]
    fn opus_roundtrip_preserves_chunk_timestamp() {
        use crate::audio_data::AudioSampleFormat;
        use crate::gst_audio_encoder::{AudioEncoderBackend, EncodedAudioSamples};

        if !element_present("opusdec") || !element_present("opusenc") {
            return;
        }
        let cfg = opus_config(2.0, None);
        let mut encoder =
            match crate::gst_audio_encoder::GstreamerAudioEncoderBackend::new(
                &crate::config::AudioEncoderConfig {
                    codec: "opus".into(),
                    sample_rate: Some(48000.0),
                    number_of_channels: Some(2.0),
                    bitrate: None,
                },
            ) {
                Ok(encoder) => encoder,
                Err(_) => return,
            };
        // 10 x 100 ms of stereo silence, timestamps starting at 5 s.
        let frames_per_chunk = 4800usize;
        let mut samples = Vec::with_capacity(frames_per_chunk * 2);
        for _ in 0..frames_per_chunk * 2 {
            samples.extend_from_slice(&0.0f32.to_le_bytes());
        }
        let base = 5_000_000i64;
        for i in 0..10 {
            encoder
                .encode(EncodedAudioSamples {
                    timestamp: base + i * 100_000,
                    duration: Some(100_000),
                    sample_rate: 48000,
                    number_of_channels: 2,
                    data_f32_interleaved: samples.clone(),
                })
                .expect("opus encode");
        }
        let encoded = encoder.flush().expect("opus encoder flush");
        assert!(!encoded.is_empty(), "opusenc produced no packets");
        assert_eq!(encoded[0].timestamp, base, "first packet PTS != input PTS");

        let mut decoder = match GstreamerAudioDecoderBackend::new(&cfg) {
            Ok(decoder) => decoder,
            Err(_) => return,
        };
        for frame in &encoded {
            decoder
                .decode(EncodedAudioChunk {
                    timestamp: frame.timestamp,
                    duration: frame.duration,
                    data: frame.data.clone(),
                })
                .expect("opus decode");
        }
        let decoded = decoder.flush().expect("opus decoder flush");
        assert!(!decoded.is_empty(), "opusdec produced no frames");
        assert_eq!(
            decoded[0].timestamp, base,
            "decoded timestamp != chunk timestamp"
        );
        assert_eq!(decoded[0].format, AudioSampleFormat::F32);
        assert_eq!(decoded[0].number_of_channels, 2);
        assert_eq!(decoded[0].sample_rate, 48000.0);
    }
}
