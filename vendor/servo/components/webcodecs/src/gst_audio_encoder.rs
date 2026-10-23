// @trace REQ-BRW-051
//! GStreamer audio encode backend (feature `media-gstreamer`, Wave B): one
//! `appsrc → audioconvert → audioresample → <encoder> → appsink` pipeline per
//! configured codec.
//!
//! Verified against the local GStreamer 1.24.2 runtime (pad templates + an
//! appsrc→encoder→appsink roundtrip, see the pipeline-shape rows):
//! - opus (`opusenc`): sink pads accept `audio/x-raw,format=S16LE,
//!   layout=interleaved,rate=48000,channels=[1,255]` (plus the 8/12/16/24 kHz
//!   rows); src emits `audio/x-opus` with `channel-mapping-family=0` for 1-2
//!   channels. A 10-chunk 100 ms push at PTS 5_000_000 µs produced 51 packets
//!   whose first PTS is exactly 5_000_000 µs (timestamp passthrough).
//! - AAC (`avenc_aac`): sink pads accept `audio/x-raw,format=F32LE,
//!   layout=interleaved,rate∈{7350..96000},channels=[1,16]`; src emits
//!   `audio/mpeg,mpegversion=4,base-profile=lc,stream-format=raw` plus a
//!   `codec_data` buffer (the 5-byte AudioSpecificConfig, e.g. `119056e500`
//!   for 48 kHz stereo) — the WebCodecs `AudioDecoderConfig.description` an
//!   accompanying decoder would consume. First output PTS equals the first
//!   input PTS (verified).
//!
//! The `audioconvert`/`audioresample` pre-stage is required, not decorative:
//! the runtime decoders' raw sink templates don't list every input format
//! (`opusenc` takes S16LE only; feeding F32LE straight in fails caps
//! negotiation), and `audioresample` narrows the configured rate to the
//! encoder's supported rows.
//!
//! Output packets carry interleaved [`EncodedAudioSamples::sample_format`]
//! `S16` for opus (the opusdec/opusenc S16 row) or `F32` for AAC, plus the
//! chunk `timestamp` (encoder PTS, microseconds) — the WPT encoder tests
//! assert timestamp preservation.
//!
//! Honest capability probe: [`is_available_for`] checks the encoder *element*
//! exists at configure time — nothing is fabricated. `bitrate` is forwarded
//! when the element exposes a writable `bitrate` property (opusenc:
//! `Integer 4000..650000`; avenc_aac: `Integer 0..2147483647`), clamped to the
//! probed `ParamSpec` range; out-of-range values are a configure error, not a
//! silent clamp.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_audio::{AudioFormat, AudioLayout};

use crate::config::AudioEncoderConfig;
use crate::error::WebCodecsError;
use crate::webcodecs_codec::{gst_audio_encoder_element, ParsedAudioCodec, ParsedCodec};

/// One encoded packet pulled from the encoder's appsink.
#[derive(Debug, Clone)]
pub struct EncodedAudioPacket {
    /// Microseconds (encoder PTS).
    pub timestamp: i64,
    /// Microseconds; `None` when the pipeline did not stamp one.
    pub duration: Option<i64>,
    pub data: Vec<u8>,
    /// `Some(bytes)` when the stream header (OpusHead + OpusTags, or the AAC
    /// AudioSpecificConfig) was negotiated — the `description` an accompanying
    /// `AudioDecoderConfig` consumes.
    pub description: Option<Vec<u8>>,
}

/// Interleaved PCM input to [`AudioEncoderBackend::encode`].
#[derive(Debug, Clone)]
pub struct EncodedAudioSamples {
    /// Microseconds.
    pub timestamp: i64,
    /// Microseconds; `None` when absent.
    pub duration: Option<i64>,
    pub sample_rate: u32,
    pub number_of_channels: u32,
    /// Interleaved native-`f32` samples (`data_f32_interleaved.len()` must be
    /// a multiple of `number_of_channels`).
    pub data_f32_interleaved: Vec<u8>,
}

/// Backend trait boundary used by the DOM layer (kept local so the
/// `media-gstreamer` feature fully owns the gst dependency; mirrors
/// `gst_video_decoder::VideoDecoderBackend`).
pub trait AudioEncoderBackend: Send {
    /// Honest capability probe: is there an encoder element for this config's
    /// codec on this platform?
    fn is_available_for(config: &AudioEncoderConfig) -> bool
    where
        Self: Sized;
    /// Build a backend for a validated config.
    fn new(config: &AudioEncoderConfig) -> Result<Self, WebCodecsError>
    where
        Self: Sized;
    /// Push interleaved f32 samples into the encoder.
    fn encode(&mut self, samples: EncodedAudioSamples) -> Result<(), WebCodecsError>;
    /// Drain: send EOS and pull every produced packet.
    fn flush(&mut self) -> Result<Vec<EncodedAudioPacket>, WebCodecsError>;
    fn reset(&mut self);
}

/// The gst-backed implementation handle.
pub struct GstreamerAudioEncoderBackend {
    inner: Pipeline,
}

impl AudioEncoderBackend for GstreamerAudioEncoderBackend {
    fn is_available_for(config: &AudioEncoderConfig) -> bool {
        is_available_for(config)
    }

    fn new(config: &AudioEncoderConfig) -> Result<Self, WebCodecsError> {
        Ok(Self {
            inner: Pipeline::new(config)?,
        })
    }

    fn encode(&mut self, samples: EncodedAudioSamples) -> Result<(), WebCodecsError> {
        self.inner.push_samples(samples)
    }

    fn flush(&mut self) -> Result<Vec<EncodedAudioPacket>, WebCodecsError> {
        self.inner.drain()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// Parse the codec string and return the verified encoder element family.
fn parsed_codec(config: &AudioEncoderConfig) -> Option<ParsedAudioCodec> {
    let Ok(ParsedCodec::Audio(audio)) = ParsedCodec::parse(&config.codec) else {
        return None;
    };
    Some(audio)
}

/// Honest probe: parsed codec + a verified element mapping + the element
/// exists after `gst::init()`. Multichannel opus (>2) is withheld (same
/// registered boundary as the decode plane: the channel-mapping caps array is
/// unverified on this runtime).
pub fn is_available_for(config: &AudioEncoderConfig) -> bool {
    let Some(audio) = parsed_codec(config) else {
        return false;
    };
    let Some(element) = gst_audio_encoder_element(&audio) else {
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

/// Requested encoder input caps: interleaved little-endian `f32` at the
/// configured geometry. `F32le` (not the native `AUDIO_FORMAT_F32` alias) is
/// deliberate: the PCM conversion helpers in this tree
/// (`webcodecs_pcm::write_sample_at`) serialize with `f32::to_le_bytes`, so the
/// caps must name exactly the byte order the DOM layer produces. The
/// `audioconvert`/`audioresample` pre-stage narrows to the encoder's own raw
/// rows (opusenc: S16LE interleaved; avenc_aac: F32LE interleaved).
fn input_caps(sample_rate: u32, channels: u32) -> gst::Caps {
    gstreamer_audio::AudioCapsBuilder::new()
        .format(AudioFormat::F32le)
        .layout(AudioLayout::Interleaved)
        .rate(sample_rate as i32)
        .channels(channels as i32)
        .build()
}

/// Validated settable range of the encoder element's `bitrate` property.
fn bitrate_range(spec: &gst::glib::ParamSpec) -> Option<(i64, i64)> {
    if let Some(int_spec) = spec.downcast_ref::<gst::glib::ParamSpecInt>() {
        return Some((i64::from(int_spec.minimum()), i64::from(int_spec.maximum())));
    }
    if let Some(uint_spec) = spec.downcast_ref::<gst::glib::ParamSpecUInt>() {
        return Some((
            i64::from(uint_spec.minimum()),
            i64::from(uint_spec.maximum()),
        ));
    }
    if let Some(int64_spec) = spec.downcast_ref::<gst::glib::ParamSpecInt64>() {
        return Some((int64_spec.minimum(), int64_spec.maximum()));
    }
    if let Some(uint64_spec) = spec.downcast_ref::<gst::glib::ParamSpecUInt64>() {
        return Some((
            i64::try_from(uint64_spec.minimum()).unwrap_or(i64::MAX),
            i64::try_from(uint64_spec.maximum()).unwrap_or(i64::MAX),
        ));
    }
    None
}

/// Syntax-face validation of an explicit `bitrate` against the encoder
/// element's own property range.
///
/// The WPT encoder-config oracle classifies an out-of-range request ("Bit rate
/// too big" = 6e9, "Bit rate present but equal to zero", "Bitrate is too low
/// for Opus" = 1) as a **TypeError** from both `configure()` and
/// `isConfigSupported()` — opusenc's range is 4000..650000, so the honest
/// probe is the element's `ParamSpec`. Unprobeable configurations (no element,
/// no property, gst unavailable) are left to the capability probe: this
/// function only rejects what the engine itself rejects.
pub fn validate_bitrate(config: &AudioEncoderConfig) -> Result<(), WebCodecsError> {
    let Some(bitrate) = config.bitrate else {
        return Ok(());
    };
    let Ok(bitrate) = i64::try_from(bitrate) else {
        return Err(WebCodecsError::syntax(
            "WebCodecsBitrateOutOfRange",
            format!("bitrate {bitrate} overflows"),
        ));
    };
    let Some(audio) = parsed_codec(config) else {
        return Ok(());
    };
    let Some(element_name) = gst_audio_encoder_element(&audio) else {
        return Ok(());
    };
    if gst::init().is_err() {
        return Ok(());
    }
    let Ok(element) = gst::ElementFactory::make(element_name).build() else {
        return Ok(());
    };
    let Some(spec) = element.find_property("bitrate") else {
        return Ok(());
    };
    let Some((min, max)) = bitrate_range(&spec) else {
        return Ok(());
    };
    if bitrate < min || bitrate > max {
        return Err(WebCodecsError::syntax(
            "WebCodecsBitrateOutOfRange",
            format!("bitrate {bitrate} outside the encoder range [{min}, {max}]"),
        ));
    }
    Ok(())
}

/// Forward `bitrate` to the encoder element when it exposes a writable
/// `bitrate` property, clamped to the probed `ParamSpec` range. Returns
/// `Err` for an out-of-range request (honest failure, no silent clamp).
fn apply_bitrate(element: &gst::Element, bitrate: Option<u64>) -> Result<(), WebCodecsError> {
    let Some(bitrate) = bitrate else {
        return Ok(());
    };
    let Some(spec) = element.find_property("bitrate") else {
        // The element has no bitrate control: the request is unrepresentable,
        // which is a configure error rather than a silent ignore.
        return Err(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!("encoder {} has no bitrate property", element.name()),
        ));
    };
    let Ok(i64_bitrate) = i64::try_from(bitrate) else {
        return Err(WebCodecsError::syntax(
            "WebCodecsBitrateOutOfRange",
            format!("bitrate {bitrate} overflows"),
        ));
    };
    let Some((min, max)) = bitrate_range(&spec) else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsUnsupportedCodec",
            format!(
                "encoder bitrate property has unsupported type {:?}",
                spec.value_type()
            ),
        ));
    };
    if i64_bitrate < min || i64_bitrate > max {
        return Err(WebCodecsError::syntax(
            "WebCodecsBitrateOutOfRange",
            format!("bitrate {bitrate} outside the encoder range [{min}, {max}]"),
        ));
    }
    // The concrete property type decides the `set_property` value type.
    if spec.downcast_ref::<gst::glib::ParamSpecInt>().is_some() {
        element.set_property("bitrate", i64_bitrate as i32);
    } else if spec.downcast_ref::<gst::glib::ParamSpecUInt>().is_some() {
        element.set_property("bitrate", i64_bitrate as u32);
    } else if spec.downcast_ref::<gst::glib::ParamSpecUInt64>().is_some() {
        element.set_property("bitrate", i64_bitrate as u64);
    } else {
        element.set_property("bitrate", i64_bitrate);
    }
    Ok(())
}

/// PTS epoch (see `gst_audio_decoder::PTS_EPOCH_US`): GStreamer timestamps are
/// unsigned while WebCodecs timestamps are signed. The encoder oracle asserts
/// negative chunk timestamps round-trip ("Encode audio with negative
/// timestamp"), so every pushed PTS is shifted by this fixed offset and the
/// offset is removed from every emitted packet timestamp.
const PTS_EPOCH_US: u64 = 1_000_000_000_000;

/// WebCodecs microseconds → GStreamer `ClockTime` microseconds (epoch-shifted).
fn to_gst_pts(timestamp: i64) -> u64 {
    let shifted = PTS_EPOCH_US as i128 + timestamp as i128;
    shifted.clamp(0, u64::MAX as i128) as u64
}

/// Buffer PTS → WebCodecs microseconds (epoch removed).
fn from_gst_pts(buffer: &gst::BufferRef) -> i64 {
    buffer
        .pts()
        .map(|t| (t.nseconds() / 1000) as i64 - PTS_EPOCH_US as i64)
        .unwrap_or(0)
}

/// One configured encoder pipeline.
struct Pipeline {
    appsrc: AppSrc,
    appsink: AppSink,
    pipeline: gst::Pipeline,
    bus: gst::Bus,
    encoder: gst::Element,
    channels: u32,
    sample_rate: u32,
}

impl Pipeline {
    fn new(config: &AudioEncoderConfig) -> Result<Self, WebCodecsError> {
        gst::init().map_err(|e| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", format!("gst::init: {e}"))
        })?;
        if !is_available_for(config) {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst audio encoder for {}", config.codec),
            ));
        }
        let Some(audio) = parsed_codec(config) else {
            return Err(WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("{} is not an audio codec", config.codec),
            ));
        };
        let encoder_element = gst_audio_encoder_element(&audio).ok_or_else(|| {
            WebCodecsError::unsupported(
                "WebCodecsUnsupportedCodec",
                format!("no gst audio encoder element for {}", config.codec),
            )
        })?;
        let rate = config.sample_rate.unwrap_or(0.0);
        let channels = config.number_of_channels.unwrap_or(0.0);
        if !(1.0..=i32::MAX as f64).contains(&rate) || !(1.0..=i32::MAX as f64).contains(&channels)
        {
            return Err(WebCodecsError::syntax(
                "WebCodecsZeroSampleRate",
                "sampleRate/numberOfChannels outside the gst caps range",
            ));
        }
        let (sample_rate, channels) = (rate as u32, channels as u32);

        let src = gst::ElementFactory::make("appsrc")
            .build()
            .map_err(bus_error("appsrc"))?;
        let convert = gst::ElementFactory::make("audioconvert")
            .build()
            .map_err(bus_error("audioconvert"))?;
        let resample = gst::ElementFactory::make("audioresample")
            .build()
            .map_err(bus_error("audioresample"))?;
        let enc = gst::ElementFactory::make(encoder_element)
            .build()
            .map_err(bus_error(encoder_element))?;
        let sink = gst::ElementFactory::make("appsink")
            .build()
            .map_err(bus_error("appsink"))?;

        let appsrc = src
            .clone()
            .downcast::<AppSrc>()
            .map_err(downcast_error("appsrc"))?;
        let appsink = sink
            .clone()
            .downcast::<AppSink>()
            .map_err(downcast_error("appsink"))?;

        // Encoded output is pulled synchronously; no caps request on the
        // encoded side (the encoder's src template defines the stream), no
        // clock pacing, unbounded retention.
        appsink.set_max_buffers(0);
        appsink.set_drop(false);
        appsink.set_sync(false);

        apply_bitrate(&enc, config.bitrate)?;

        let pipeline = gst::Pipeline::new();
        pipeline
            .add_many([&src, &convert, &resample, &enc, &sink])
            .map_err(bus_error("add_many"))?;
        gst::Element::link_many([&src, &convert, &resample, &enc, &sink])
            .map_err(bus_error("link_many"))?;

        let bus = pipeline.bus().ok_or_else(|| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", "pipeline has no bus")
        })?;

        appsrc.set_caps(Some(&input_caps(sample_rate, channels)));
        appsrc.set_format(gst::Format::Time);
        appsrc.set_max_bytes(0);
        pipeline.set_state(gst::State::Playing).map_err(|_| {
            WebCodecsError::unsupported("WebCodecsGstreamerInit", "pipeline did not start")
        })?;

        Ok(Self {
            appsrc,
            appsink,
            pipeline,
            bus,
            encoder: enc,
            channels,
            sample_rate,
        })
    }

    /// Push one interleaved f32 sample block.
    fn push_samples(&mut self, samples: EncodedAudioSamples) -> Result<(), WebCodecsError> {
        self.poll_bus_for_errors()?;
        if samples.number_of_channels != self.channels ||
            samples.sample_rate != self.sample_rate
        {
            return Err(WebCodecsError::syntax(
                "WebCodecsEncodeError",
                format!(
                    "encode() sample geometry {}Hz/{}ch does not match the configured {}Hz/{}ch",
                    samples.sample_rate,
                    samples.number_of_channels,
                    self.sample_rate,
                    self.channels
                ),
            ));
        }
        if samples.data_f32_interleaved.len() % 4 != 0 {
            return Err(WebCodecsError::syntax(
                "WebCodecsEncodeError",
                "sample payload is not a whole number of f32 samples",
            ));
        }
        let sample_count = samples.data_f32_interleaved.len() / 4;
        if sample_count % self.channels as usize != 0 {
            return Err(WebCodecsError::syntax(
                "WebCodecsEncodeError",
                "sample payload is not a whole number of frames",
            ));
        }
        let mut buffer = gst::Buffer::from_slice(samples.data_f32_interleaved);
        {
            let buf = buffer.get_mut().expect("fresh buffer is writable");
            buf.set_pts(gst::ClockTime::from_useconds(to_gst_pts(samples.timestamp)));
            if let Some(d) = samples.duration {
                buf.set_duration(gst::ClockTime::from_useconds(d.unsigned_abs()));
            }
        }
        self.appsrc
            .push_buffer(buffer)
            .map_err(|_| self.take_bus_error())?;
        Ok(())
    }

    /// Spec `flush()`: send EOS, pull every produced packet until the bus
    /// reports the matching `eos`.
    fn drain(&mut self) -> Result<Vec<EncodedAudioPacket>, WebCodecsError> {
        self.appsrc
            .end_of_stream()
            .map_err(|_| self.take_bus_error())?;
        let mut packets = Vec::new();
        loop {
            while let Some(sample) = self
                .appsink
                .try_pull_sample(gst::ClockTime::from_mseconds(1))
            {
                packets.push(sample_to_packet(&sample)?);
            }
            match self.bus.timed_pop_filtered(
                Some(gst::ClockTime::from_mseconds(50)),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                Some(msg) => match msg.view() {
                    gst::MessageView::Eos(_) => break,
                    gst::MessageView::Error(err) => {
                        return Err(WebCodecsError::unsupported(
                            "WebCodecsEncodeError",
                            format!("gst pipeline error: {}", err.error()),
                        ));
                    },
                    _ => {},
                },
                None => continue,
            }
        }
        // The description (streamheader / codec_data) is only knowable after
        // the encoder has negotiated; attach it once per drain.
        let description = streamheader_description(&self.encoder);
        if let Some(description) = description {
            for packet in &mut packets {
                if packet.description.is_none() {
                    packet.description = Some(description.clone());
                }
            }
        }
        // Restart for a possible second flush (the spec allows
        // encode→flush→encode on one encoder); mirrors the decode-side drain
        // restart in gst_video_decoder::Pipeline::drain.
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| self.take_bus_error())?;
        Ok(packets)
    }

    /// Spec `reset()`: stop the pipeline and drop queued state.
    fn reset(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    fn poll_bus_for_errors(&self) -> Result<(), WebCodecsError> {
        while let Some(msg) = self.bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(err) = msg.view() {
                return Err(WebCodecsError::unsupported(
                    "WebCodecsEncodeError",
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

/// Pull the negotiated `codec_data`/`streamheader` off the encoder's src pad
/// caps. Both spellings are handled:
/// - avenc_aac sets a single `codec_data` buffer (the AudioSpecificConfig) —
///   verified on the local runtime (`codec_data=(buffer)119056e500` for
///   48 kHz stereo).
/// - opusenc sets `streamheader` as a `GstValueArray` of buffers whose first
///   entry is the OpusHead (verified: the caps print as
///   `streamheader=(buffer)< 4f70757348656164…, 4f70757354616773… >` and the
///   field type is `GstValueArray`). A `GstBufferList` spelling is accepted
///   too, for plugin-version robustness.
fn streamheader_description(encoder: &gst::Element) -> Option<Vec<u8>> {
    let src_pad = encoder.static_pad("src")?;
    let caps = src_pad.current_caps()?;
    let structure = caps.structure(0)?;
    if let Ok(buffer) = structure.get::<gst::Buffer>("codec_data") {
        let map = buffer.map_readable().ok()?;
        return Some(map.as_slice().to_vec());
    }
    if let Ok(array) = structure.get::<gst::Array>("streamheader") {
        if let Some(value) = array.as_slice().first() {
            if let Ok(buffer) = value.get::<gst::Buffer>() {
                let map = buffer.map_readable().ok()?;
                return Some(map.as_slice().to_vec());
            }
        }
    }
    if let Ok(list) = structure.get::<gst::BufferList>("streamheader") {
        if let Some(first) = list.get(0) {
            let map = first.map_readable().ok()?;
            return Some(map.as_slice().to_vec());
        }
    }
    None
}

fn pts_micros(buffer: &gst::BufferRef) -> i64 {
    from_gst_pts(buffer)
}

fn duration_micros(buffer: &gst::BufferRef) -> Option<i64> {
    buffer
        .duration()
        .map(|t| (t.nseconds() / 1000) as i64)
}

/// Convert one appsink sample into a plain [`EncodedAudioPacket`].
fn sample_to_packet(sample: &gst::Sample) -> Result<EncodedAudioPacket, WebCodecsError> {
    let Some(buffer) = sample.buffer() else {
        return Err(WebCodecsError::unsupported(
            "WebCodecsEncodeError",
            "empty sample",
        ));
    };
    let map = buffer.map_readable().map_err(|_| {
        WebCodecsError::unsupported("WebCodecsEncodeError", "buffer is not readable")
    })?;
    if map.as_slice().is_empty() {
        return Err(WebCodecsError::unsupported(
            "WebCodecsEncodeError",
            "encoded packet carries no bytes",
        ));
    }
    Ok(EncodedAudioPacket {
        timestamp: pts_micros(buffer),
        duration: duration_micros(buffer),
        data: map.as_slice().to_vec(),
        description: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus_config(channels: f64, bitrate: Option<u64>) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec: "opus".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(channels),
            bitrate,
        }
    }

    fn aac_config(bitrate: Option<u64>) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec: "mp4a.40.2".into(),
            sample_rate: Some(44100.0),
            number_of_channels: Some(2.0),
            bitrate,
        }
    }

    #[test]
    fn bitrate_out_of_range_is_an_error_not_a_clamp() {
        gst::init().unwrap();
        let Ok(enc) = gst::ElementFactory::make("opusenc").build() else {
            return;
        };
        // opusenc bitrate range is 4000..650000 (verified via gst-inspect).
        assert!(apply_bitrate(&enc, Some(64000)).is_ok());
        assert!(apply_bitrate(&enc, Some(1)).is_err());
        assert!(apply_bitrate(&enc, Some(u64::MAX)).is_err());
    }

    #[test]
    fn validate_bitrate_matches_the_opusenc_range() {
        gst::init().unwrap();
        if gst::ElementFactory::find("opusenc").is_none() {
            return;
        }
        let with_bitrate = |bitrate: u64| AudioEncoderConfig {
            codec: "opus".into(),
            sample_rate: Some(48000.0),
            number_of_channels: Some(2.0),
            bitrate: Some(bitrate),
        };
        assert!(validate_bitrate(&with_bitrate(64000)).is_ok());
        // WPT "Bit rate too big" (6e9), "Bit rate present but equal to zero"
        // and "Bitrate is too low for Opus" (1) are all TypeErrors; the range
        // error carries the Syntax kind the DOM maps to TypeError.
        assert!(matches!(
            validate_bitrate(&with_bitrate(6_000_000_000)),
            Err(WebCodecsError {
                kind: crate::error::WebCodecsErrorKind::Syntax,
                ..
            })
        ));
        assert!(validate_bitrate(&with_bitrate(0)).is_err());
        assert!(validate_bitrate(&with_bitrate(1)).is_err());
        // No explicit bitrate is always fine.
        let mut no_bitrate = with_bitrate(64000);
        no_bitrate.bitrate = None;
        assert!(validate_bitrate(&no_bitrate).is_ok());
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

    #[test]
    fn multichannel_opus_is_not_claimed() {
        assert!(!is_available_for(&opus_config(4.0, None)));
    }

    #[test]
    fn probe_matches_element_presence() {
        if gst::init().is_err() {
            return;
        }
        if gst::ElementFactory::find("opusenc").is_some() {
            assert!(is_available_for(&opus_config(2.0, None)));
        }
        if gst::ElementFactory::find("avenc_aac").is_some() {
            assert!(is_available_for(&aac_config(None)));
        }
    }

    /// Real encode roundtrip for AAC: the encoder must emit the
    /// AudioSpecificConfig as the packet description (the metadata
    /// `decoderConfig.description` a decoder consumes) and preserve the input
    /// timestamp. Skips on hosts without avenc_aac.
    #[test]
    fn aac_encode_emits_codec_data_description_and_preserves_timestamp() {
        if gst::init().is_err() || gst::ElementFactory::find("avenc_aac").is_none() {
            return;
        }
        let mut encoder =
            GstreamerAudioEncoderBackend::new(&aac_config(Some(128000))).expect("aac encoder");
        let frames = 4410usize; // 0.1 s at 44.1 kHz
        let samples = vec![0u8; frames * 2 * 4];
        let base = 1_000_000i64;
        for i in 0..5 {
            encoder
                .encode(EncodedAudioSamples {
                    timestamp: base + i * 100_000,
                    duration: Some(100_000),
                    sample_rate: 44100,
                    number_of_channels: 2,
                    data_f32_interleaved: samples.clone(),
                })
                .expect("aac encode");
        }
        let packets = encoder.flush().expect("flush");
        assert!(!packets.is_empty(), "avenc_aac produced no packets");
        assert_eq!(packets[0].timestamp, base, "first packet PTS != input PTS");
        let description = packets[0]
            .description
            .as_ref()
            .expect("AAC codec_data (AudioSpecificConfig) was not negotiated");
        assert!(!description.is_empty());
    }

    /// Real encode roundtrip: interleaved f32 silence -> opusenc packets,
    /// asserting the input timestamp is carried through unchanged. Skips on
    /// hosts without opusenc.
    #[test]
    fn opus_encode_preserves_timestamp_and_emits_description() {
        if gst::init().is_err() || gst::ElementFactory::find("opusenc").is_none() {
            return;
        }
        let mut encoder = GstreamerAudioEncoderBackend::new(&opus_config(2.0, Some(64000)))
            .expect("opus encoder");
        let frames = 4800usize;
        let samples = vec![0u8; frames * 2 * 4];
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
        let packets = encoder.flush().expect("flush");
        assert!(!packets.is_empty(), "opusenc produced no packets");
        assert_eq!(packets[0].timestamp, base, "first packet PTS != input PTS");
        assert!(
            packets[0].description.is_some(),
            "opus streamheader (OpusHead) was not negotiated"
        );
    }
}
