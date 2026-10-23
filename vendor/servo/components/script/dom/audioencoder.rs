/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `AudioEncoder` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#audioencoder-interface> + the WPT
//! audio-encoder oracle).
//!
//! Two encode legs, selected by the configured codec string:
//! - **Raw PCM** (pass-through, the encode-plane mirror of the Wave A PCM
//!   decode contract): the PCM codec-registry strings
//!   (`pcm-u8`/`pcm-s16`/`pcm-s32`/`pcm-f32`) map 1:1 onto interleaved
//!   `AudioSampleFormat` values, so encoding is a sample-format conversion of
//!   the `AudioData` resource with no compression.
//! - **Compressed audio** (Wave B): opus / mp4a (AAC) encode through the
//!   component GStreamer backend (`webcodecs-gstreamer` feature). The
//!   capability probe is the encoder *element* presence (`opusenc`,
//!   `avenc_aac`), so a platform without the element reports `supported=false`
//!   and fires the `error` callback on configure — no engine is fabricated.
//!
//! Output delivery: encoded packets reach the `output` callback at the
//! `flush()` drain point, together with an `EncodedAudioChunkMetadata` whose
//! `decoderConfig.description` is the description the encoder itself
//! negotiated (OpusHead from the `streamheader` caps / AAC
//! AudioSpecificConfig from `codec_data`) — read from the pipeline, never
//! synthesized.
//!
//! Registered boundaries:
//! - `configure()` on a well-formed-but-unbacked config fires
//!   NotSupportedError + close (the WPT "unsupported config" ordering: the
//!   error callback is invoked on a task, the state is already `closed` when
//!   the page observes it).
//! - `bitrateMode` is treated as the spec's hint (no CBR/quantizer engine
//!   control is claimed); `bitrate` is forwarded to the encoder element when
//!   it exposes the property.
//! - `encode()` requires the `AudioData` geometry to match the configured
//!   sample rate / channel count (the encoder-config contract); a mismatch is
//!   a TypeError.
//! - `flush()` settles in a microtask so `reset()` can abort an in-flight
//!   flush with AbortError (same model as the decoder).

use std::cell::Cell;
use std::ptr;
use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::JSObject;
use js::rust::HandleObject;
use js::typedarray::{HeapArrayBufferView, Uint8};
use script_bindings::callback::TracedCallback;
use script_bindings::cell::DomRefCell;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use script_bindings::root::Dom;
use script_bindings::trace::RootedTraceableBox;
use stylo_atoms::Atom;

use crate::dom::audiodata::{AudioData, AudioDataSnapshot};
use crate::dom::bindings::buffer_source::create_buffer_source;
use crate::dom::bindings::callback::ExceptionHandling;
use crate::dom::bindings::codegen::Bindings::AudioDataBinding::AudioSampleFormat;
// `AudioEncoder` and every dictionary/callback/enum it uses are declared in
// `WebCodecs.webidl`; with several interfaces per file the generated module is
// the *file* module (`WebCodecsBinding`), which re-exports each interface's
// `Methods` trait at module level (`CGConcreteBindingRoot`) — the same shape
// as `Clipboard.webidl` → `ClipboardBinding::ClipboardItemMethods`.
use crate::dom::bindings::codegen::Bindings::WebCodecsBinding::{
    AudioDecoderConfig as AudioDecoderConfigDict, AudioEncoderConfig, AudioEncoderInit,
    AudioEncoderMethods, AudioEncoderSupport, CodecState, EncodedAudioChunkMetadata,
    EncodedAudioChunkOutputCallback, WebCodecsErrorCallback,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::bindings::trace::JSTraceable;
use crate::dom::domexception::{DOMErrorName, DOMException};
use crate::dom::encodedaudiochunk::EncodedAudioChunk;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::webcodecs_error::{
    audio_encoder_config_to_engine, codec_string, encoder_syntax_face, map_error,
};
use crate::dom::webcodecs_pcm::{
    copy_samples, is_planar, sample_format_for_pcm_codec, sample_size,
};
use crate::realms::enter_auto_realm;
use crate::runtime::job_queue::MicrotaskRunnable;
use webcodecs::config::{ConfigValidation, validate_audio_encoder_config};

#[cfg(feature = "webcodecs-gstreamer")]
use webcodecs::gst_audio_encoder::{
    AudioEncoderBackend, EncodedAudioSamples, GstreamerAudioEncoderBackend,
};

/// Configured geometry + codec string, captured at `configure()` time for
/// output wrapping (`EncodedAudioChunkMetadata.decoderConfig`) and the
/// `encode()` geometry contract.
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct EncoderConfig {
    codec: String,
    sample_rate: u32,
    number_of_channels: u32,
}

/// PCM pass-through encoder configuration (Wave A codec-registry strings).
#[derive(Clone, Copy)]
struct PcmEncodeConfig {
    format: AudioSampleFormat,
    sample_rate: u32,
}

impl PcmEncodeConfig {
    /// Re-interpret the `AudioData` samples in the configured PCM format. The
    /// configured sample rate is the authoritative frame clock for the chunk
    /// duration (`encode()` already enforced geometry equality).
    fn encode(&self, snapshot: &AudioDataSnapshot) -> OutputPacket {
        OutputPacket {
            timestamp: snapshot.timestamp,
            duration: duration_micros(snapshot.number_of_frames, self.sample_rate as f32),
            data: convert_samples(snapshot, self.format),
            description: None,
        }
    }
}

/// The active encode leg. The slot is opaque to the GC (no JS pointers).
enum EncodeBackend {
    Unconfigured,
    Pcm(PcmEncodeConfig),
    #[cfg(feature = "webcodecs-gstreamer")]
    Gst(Box<dyn AudioEncoderBackend>),
}

struct EncodeBackendSlot(EncodeBackend);

#[expect(unsafe_code)]
unsafe impl JSTraceable for EncodeBackendSlot {
    #[inline]
    unsafe fn trace(&self, _: *mut ::js::jsapi::JSTracer) {
        // No JS-managed fields inside the backend / PCM config.
    }
}

impl malloc_size_of::MallocSizeOf for EncodeBackendSlot {
    fn size_of(&self, _ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        // Opaque native handle; the pipeline's own accounting stays native.
        0
    }
}

/// One queued `encode()` request: an immutable capture of the `AudioData`
/// resource. The bytes are copied once at enqueue; every later queue read
/// shares them through the `Rc`.
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct PendingEncode {
    format: AudioSampleFormat,
    sample_rate: f32,
    number_of_frames: u32,
    number_of_channels: u32,
    timestamp: i64,
    /// Plain sample bytes; never JS-managed.
    #[no_trace]
    #[ignore_malloc_size_of = "Plain sample bytes owned by the webcodecs pipeline"]
    data: Rc<[u8]>,
}

impl PendingEncode {
    fn from_snapshot(snapshot: AudioDataSnapshot) -> Self {
        Self {
            format: snapshot.format,
            sample_rate: snapshot.sample_rate,
            number_of_frames: snapshot.number_of_frames,
            number_of_channels: snapshot.number_of_channels,
            timestamp: snapshot.timestamp,
            data: Rc::from(snapshot.data.to_vec()),
        }
    }

    fn as_snapshot(&self) -> AudioDataSnapshot {
        AudioDataSnapshot {
            format: self.format,
            sample_rate: self.sample_rate,
            number_of_frames: self.number_of_frames,
            number_of_channels: self.number_of_channels,
            timestamp: self.timestamp,
            data: self.data.clone(),
        }
    }
}

/// One encoded packet ready for the output callback (both legs).
struct OutputPacket {
    timestamp: i64,
    duration: Option<i64>,
    data: Vec<u8>,
    description: Option<Vec<u8>>,
}

/// Bitrate syntax face against the encoder element's own property range
/// (WPT "Bit rate too big"/"equal to zero" are TypeErrors). Without the gst
/// feature there is no engine to interrogate, so only the element-presence
/// capability probe applies.
fn validate_encoder_bitrate(
    config: &webcodecs::config::AudioEncoderConfig,
) -> Result<(), webcodecs::error::WebCodecsError> {
    #[cfg(feature = "webcodecs-gstreamer")]
    {
        webcodecs::gst_audio_encoder::validate_bitrate(config)
    }
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    {
        let _ = config;
        Ok(())
    }
}

/// Honest platform probe for the compressed-audio encode legs.
fn encoder_backend_available(config: &webcodecs::config::AudioEncoderConfig) -> bool {
    #[cfg(feature = "webcodecs-gstreamer")]
    {
        GstreamerAudioEncoderBackend::is_available_for(config)
    }
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    {
        let _ = config;
        false
    }
}

fn make_backend(config: &webcodecs::config::AudioEncoderConfig) -> Option<EncodeBackend> {
    #[cfg(feature = "webcodecs-gstreamer")]
    {
        GstreamerAudioEncoderBackend::new(config)
            .ok()
            .map(|backend| EncodeBackend::Gst(Box::new(backend) as Box<dyn AudioEncoderBackend>))
    }
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    {
        let _ = config;
        None
    }
}

/// Duration in microseconds for `frames` at `sample_rate` (rounded, the
/// WebCodecs `EncodedAudioChunk.duration` surface).
fn duration_micros(frames: u32, sample_rate: f32) -> Option<i64> {
    if !(sample_rate > 0.0) {
        return None;
    }
    let micros = f64::from(frames) * 1_000_000.0 / f64::from(sample_rate);
    Some(micros.round() as i64)
}

/// Convert the source samples into `target` layout/format using the shared
/// WebCodecs PCM arithmetic. Planar targets are written plane by plane (the
/// helper addresses one destination plane per call).
fn convert_samples(snapshot: &AudioDataSnapshot, target: AudioSampleFormat) -> Vec<u8> {
    let frames = snapshot.number_of_frames as usize;
    let channels = snapshot.number_of_channels as usize;
    let size = sample_size(target);
    let Some(total) = frames
        .checked_mul(channels)
        .and_then(|samples| samples.checked_mul(size))
    else {
        return Vec::new();
    };
    let mut out = vec![0u8; total];
    if frames == 0 || channels == 0 {
        return out;
    }
    if is_planar(target) {
        let plane_len = frames * size;
        for channel in 0..channels {
            let start = channel * plane_len;
            let end = start + plane_len;
            if end > out.len() {
                break;
            }
            copy_samples(
                snapshot.format,
                &snapshot.data,
                channels,
                frames,
                0,
                frames,
                target,
                &mut out[start..end],
                channel,
            );
        }
    } else {
        copy_samples(
            snapshot.format,
            &snapshot.data,
            channels,
            frames,
            0,
            frames,
            target,
            &mut out,
            0,
        );
    }
    out
}

/// <https://www.w3.org/TR/webcodecs/#audioencoder>
#[dom_struct]
pub(crate) struct AudioEncoder {
    eventtarget: EventTarget,
    state: Cell<CodecState>,
    config: DomRefCell<Option<EncoderConfig>>,
    backend: DomRefCell<EncodeBackendSlot>,
    pending: DomRefCell<Vec<PendingEncode>>,
    flush_aborted: Cell<bool>,
    /// `EncodedAudioChunkMetadata.decoderConfig` is emitted exactly once per
    /// configuration (first output), per the WPT encoder oracle.
    sent_decoder_config: Cell<bool>,
    output: DomRefCell<Option<TracedCallback<EncodedAudioChunkOutputCallback>>>,
    error_callback: DomRefCell<Option<TracedCallback<WebCodecsErrorCallback>>>,
}

impl AudioEncoder {
    fn new_inherited(
        output: TracedCallback<EncodedAudioChunkOutputCallback>,
        error_callback: TracedCallback<WebCodecsErrorCallback>,
    ) -> AudioEncoder {
        AudioEncoder {
            eventtarget: EventTarget::new_inherited(),
            state: Cell::new(CodecState::Unconfigured),
            config: DomRefCell::new(None),
            backend: DomRefCell::new(EncodeBackendSlot(EncodeBackend::Unconfigured)),
            pending: DomRefCell::new(Vec::new()),
            flush_aborted: Cell::new(false),
            sent_decoder_config: Cell::new(false),
            output: DomRefCell::new(Some(output)),
            error_callback: DomRefCell::new(Some(error_callback)),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioEncoderInit,
    ) -> DomRoot<AudioEncoder> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(AudioEncoder::new_inherited(
                init.output.root(cx).to_traced(),
                init.error.root(cx).to_traced(),
            )),
            global,
            proto,
        )
    }

    #[allow(unsafe_code)]
    fn thread_cx() -> JSContext {
        unsafe { JSContext::get_from_thread() }.expect("no active JS context")
    }

    /// Fire the `error` callback with a DOMException of `name`.
    fn fire_error(&self, name: DOMErrorName) {
        // Clone the callback out of the cell: the `Ref` temporary of a
        // let-else dies with the statement, and the slot must stay free for a
        // re-entrant `close()` on the error path.
        let Some(callback) = self.error_callback.borrow().clone() else {
            return;
        };
        let global = self.global();
        let mut cx = Self::thread_cx();
        let exception = DOMException::new(&mut cx, &global, name);
        let _ = callback.Call_(&mut cx, &*global, &exception, ExceptionHandling::Report);
    }

    fn close(&self) {
        self.state.set(CodecState::Closed);
        *self.config.borrow_mut() = None;
        *self.backend.borrow_mut() = EncodeBackendSlot(EncodeBackend::Unconfigured);
        self.pending.borrow_mut().clear();
        self.sent_decoder_config.set(false);
    }

    /// Pending encode count (inherent form of the `encodeQueueSize`
    /// attribute, for the flush microtask's dequeue decision).
    fn queued_count(&self) -> usize {
        self.pending.borrow().len()
    }

    fn take_flush_aborted(&self) -> bool {
        if self.flush_aborted.get() {
            self.flush_aborted.set(false);
            true
        } else {
            false
        }
    }

    /// Drain the queued `AudioData` captures through the active leg and fire
    /// the `output` callback for every encoded packet. The backend slot is
    /// borrowed only for the encode/flush window; the error path closes the
    /// encoder after the borrow is released (closing re-borrows the slot).
    fn drain_pending(&self) -> Fallible<()> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        // `Ref` itself is `Clone`, so deref before cloning the option.
        let Some(config) = (*self.config.borrow()).clone() else {
            return Ok(());
        };
        let mut packets: Vec<OutputPacket> = Vec::new();
        let mut failure: Option<webcodecs::error::WebCodecsError> = None;
        // Encoder-config contract: every queued frame's geometry must match the
        // configured sample rate / channel count. The mismatch is reported
        // asynchronously as an EncodingError (the WPT "Channel number
        // variation" / "Sample rate variation" tests call encode() without
        // expecting a throw, then assert the error callback + flush rejection).
        if let Some(bad) = pending.iter().find(|queued| {
            queued.sample_rate != config.sample_rate as f32
                || queued.number_of_channels != config.number_of_channels
        }) {
            failure = Some(webcodecs::error::WebCodecsError::unsupported(
                "WebCodecsEncodeError",
                format!(
                    "encode() sample geometry {}Hz/{}ch does not match the configured {}Hz/{}ch",
                    bad.sample_rate,
                    bad.number_of_channels,
                    config.sample_rate,
                    config.number_of_channels
                ),
            ));
        }
        if failure.is_none() {
            let mut guard = self.backend.borrow_mut();
            match &mut guard.0 {
                EncodeBackend::Unconfigured => {
                    // Configure never reaches Configured without a leg.
                },
                EncodeBackend::Pcm(pcm) => {
                    for queued in pending {
                        packets.push(pcm.encode(&queued.as_snapshot()));
                    }
                },
                #[cfg(feature = "webcodecs-gstreamer")]
                EncodeBackend::Gst(backend) => {
                    for queued in pending {
                        let samples =
                            convert_samples(&queued.as_snapshot(), AudioSampleFormat::F32);
                        let duration = duration_micros(queued.number_of_frames, queued.sample_rate);
                        let input = EncodedAudioSamples {
                            timestamp: queued.timestamp,
                            duration,
                            sample_rate: queued.sample_rate as u32,
                            number_of_channels: queued.number_of_channels,
                            data_f32_interleaved: samples,
                        };
                        if let Err(error) = backend.encode(input) {
                            failure = Some(error);
                            break;
                        }
                    }
                    if failure.is_none() {
                        match backend.flush() {
                            Ok(drained) => {
                                packets.extend(drained.into_iter().map(|packet| OutputPacket {
                                    timestamp: packet.timestamp,
                                    duration: packet.duration,
                                    data: packet.data,
                                    description: packet.description,
                                }));
                            },
                            Err(error) => failure = Some(error),
                        }
                    }
                },
            }
        }
        if let Some(error) = failure {
            // Any encode-path failure is the spec's EncodingError face (the
            // component reason stays in the error value for diagnostics).
            let _ = &error;
            self.fire_error(DOMErrorName::EncodingError);
            self.close();
            return Err(Error::Encoding(None));
        }
        let global = self.global();
        let mut cx = Self::thread_cx();
        for packet in &packets {
            if self.state.get() != CodecState::Configured {
                // reset() during output delivery aborts the in-flight flush and
                // suppresses further outputs (WPT "Test reset during flush":
                // exactly one output, then AbortError).
                self.flush_aborted.set(false);
                return Err(Error::Abort(None));
            }
            let with_config = !self.sent_decoder_config.get();
            self.emit_packet(&mut cx, &global, packet, &config, with_config)?;
            if with_config {
                self.sent_decoder_config.set(true);
            }
        }
        Ok(())
    }

    /// Deliver one encoded packet: `EncodedAudioChunk` + metadata carrying the
    /// encoder-negotiated codec description.
    ///
    /// `with_config` is true only for the first output after `configure()` —
    /// the WPT oracle asserts `metadata.decoderConfig` is an object exactly
    /// once per configuration and `undefined` for every later output.
    fn emit_packet(
        &self,
        cx: &mut JSContext,
        global: &GlobalScope,
        packet: &OutputPacket,
        config: &EncoderConfig,
        with_config: bool,
    ) -> Fallible<()> {
        let duration = packet.duration.and_then(|d| u64::try_from(d).ok());
        let chunk = EncodedAudioChunk::from_encoded(
            cx,
            global,
            packet.timestamp,
            duration,
            Rc::from(packet.data.as_slice()),
        );
        let decoder_config = if with_config {
            let description = match packet.description.as_deref() {
                Some(bytes) if !bytes.is_empty() => {
                    rooted!(&in(cx) let mut object = ptr::null_mut::<JSObject>());
                    create_buffer_source::<Uint8>(cx, bytes, object.handle_mut())
                        .map_err(|_| Error::JSFailed)?;
                    let view =
                        HeapArrayBufferView::from(object.get()).map_err(|_| Error::JSFailed)?;
                    Some(ArrayBufferViewOrArrayBuffer::ArrayBufferView(
                        RootedTraceableBox::new(view),
                    ))
                },
                _ => None,
            };
            Some(RootedTraceableBox::new(AudioDecoderConfigDict {
                codec: DOMString::from(config.codec.clone()),
                sampleRate: config.sample_rate,
                numberOfChannels: config.number_of_channels,
                description,
            }))
        } else {
            None
        };
        let metadata = EncodedAudioChunkMetadata {
            decoderConfig: decoder_config,
        };
        let Some(callback) = self.output.borrow().clone() else {
            return Ok(());
        };
        let _ = callback.Call_(cx, &*global, &chunk, &metadata, ExceptionHandling::Report);
        Ok(())
    }
}

/// Settles the `flush()` promise: honors the reset-abort flag, then drains the
/// queued encodes through the active leg.
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct AudioEncoderFlushMicrotask {
    encoder: Dom<AudioEncoder>,
    global: Dom<GlobalScope>,
    promise: Dom<Promise>,
}

impl MicrotaskRunnable for AudioEncoderFlushMicrotask {
    fn handler(&self, cx: &mut JSContext) {
        // MicrotaskRunnable contract: handlers manage their own realm entry.
        let mut realm = enter_auto_realm(cx, &*self.global);
        let cx = &mut realm.current_realm();
        if self.encoder.take_flush_aborted() {
            self.promise.reject_error(cx, Error::Abort(None));
            return;
        }
        // `dequeue` fires when encodeQueueSize drops to zero (the drain below
        // empties the queue); the WPT oracle asserts one event per drained
        // batch with the queue already at 0. "dequeue" is not in the static
        // atom table, so this uses the dynamic-atom constructor.
        let had_queued = self.encoder.queued_count() > 0;
        match self.encoder.drain_pending() {
            Ok(()) => {
                if had_queued {
                    self.encoder
                        .upcast::<EventTarget>()
                        .fire_event(cx, Atom::from("dequeue"));
                }
                self.promise.resolve_native(cx, &())
            },
            Err(error) => self.promise.reject_error(cx, error),
        }
    }
}

impl AudioEncoderMethods<crate::DomTypeHolder> for AudioEncoder {
    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-audioencoder>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioEncoderInit,
    ) -> Fallible<DomRoot<AudioEncoder>> {
        Ok(AudioEncoder::new(cx, global, proto, init))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-state>
    fn State(&self) -> CodecState {
        self.state.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-encodequeuesize>
    fn EncodeQueueSize(&self) -> u32 {
        self.pending.borrow().len() as u32
    }

    event_handler!(dequeue, GetOndequeue, SetOndequeue);

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-configure>
    fn Configure(&self, config: &AudioEncoderConfig) -> Fallible<()> {
        if self.state.get() == CodecState::Closed {
            return Err(Error::InvalidState(None));
        }
        let codec = codec_string(&config.codec);
        encoder_syntax_face(
            &codec,
            &[
                ("sampleRate", u64::from(config.sampleRate)),
                ("numberOfChannels", u64::from(config.numberOfChannels)),
            ],
        )?;
        // Raw-PCM passthrough leg (encode-plane mirror of the Wave A decoder).
        if let Some(format) = sample_format_for_pcm_codec(&codec) {
            *self.backend.borrow_mut() = EncodeBackendSlot(EncodeBackend::Pcm(PcmEncodeConfig {
                format,
                sample_rate: config.sampleRate,
            }));
            *self.config.borrow_mut() = Some(EncoderConfig {
                codec,
                sample_rate: config.sampleRate,
                number_of_channels: config.numberOfChannels,
            });
            self.state.set(CodecState::Configured);
            self.flush_aborted.set(false);
            self.sent_decoder_config.set(false);
            return Ok(());
        }
        let engine_config = audio_encoder_config_to_engine(config);
        // Syntax face first: an explicit bitrate the engine cannot represent is
        // a TypeError (thrown from configure, rejected from
        // isConfigSupported), before any capability question.
        validate_encoder_bitrate(&engine_config).map_err(|error| map_error(&error))?;
        match validate_audio_encoder_config(
            &engine_config,
            encoder_backend_available(&engine_config),
        ) {
            Err(error) => Err(map_error(&error)),
            // Well-formed-but-unbacked (no gst encoder element for this codec,
            // or a build without the gst audio backend): error + close, the
            // Wave A contract for unbacked configs (now reachable only when
            // the element is genuinely absent).
            Ok(ConfigValidation::Unsupported(_)) => {
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                Ok(())
            },
            Ok(ConfigValidation::Supported(_)) => {
                let Some(backend) = make_backend(&engine_config) else {
                    self.fire_error(DOMErrorName::NotSupportedError);
                    self.close();
                    return Ok(());
                };
                *self.backend.borrow_mut() = EncodeBackendSlot(backend);
                *self.config.borrow_mut() = Some(EncoderConfig {
                    codec,
                    sample_rate: config.sampleRate,
                    number_of_channels: config.numberOfChannels,
                });
                self.state.set(CodecState::Configured);
                self.flush_aborted.set(false);
                self.sent_decoder_config.set(false);
                Ok(())
            },
        }
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-encode>
    fn Encode(&self, data: &AudioData) -> Fallible<()> {
        if self.state.get() != CodecState::Configured {
            return Err(Error::InvalidState(None));
        }
        // Closed `AudioData` has a null `[[format]]` → TypeError (spec).
        let snapshot = data
            .encode_snapshot()
            .ok_or_else(|| Error::Type(c"data is closed".to_owned()))?;
        // Geometry mismatches are *not* a synchronous throw: the spec reports
        // them through the error callback (EncodingError) and closes the
        // encoder, so the frame is queued and validated at the drain point
        // (WPT "Channel number variation" / "Sample rate variation").
        self.pending
            .borrow_mut()
            .push(PendingEncode::from_snapshot(snapshot));
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-flush>
    fn Flush(&self) -> Fallible<RootedPromise> {
        let global = self.global();
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        if self.state.get() != CodecState::Configured {
            promise.reject_error(&mut cx, Error::InvalidState(None));
            return Ok(promise);
        }
        global.enqueue_microtask(
            &cx,
            Box::new(AudioEncoderFlushMicrotask {
                encoder: Dom::from_ref(self),
                global: Dom::from_ref(&*global),
                promise: Dom::from_ref(&*promise),
            }),
        );
        Ok(promise)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-reset>
    fn Reset(&self) -> Fallible<()> {
        if self.state.get() == CodecState::Configured {
            self.state.set(CodecState::Unconfigured);
            *self.config.borrow_mut() = None;
            *self.backend.borrow_mut() = EncodeBackendSlot(EncodeBackend::Unconfigured);
            self.pending.borrow_mut().clear();
            self.sent_decoder_config.set(false);
            // A reset while a flush is in flight aborts that flush (spec).
            self.flush_aborted.set(true);
        }
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-close>
    fn Close(&self) -> Fallible<()> {
        self.close();
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audioencoder-isconfigsupported>
    fn IsConfigSupported(
        global: &GlobalScope,
        config: &AudioEncoderConfig,
    ) -> Fallible<RootedPromise> {
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        let codec = codec_string(&config.codec);
        let outcome: Result<bool, Error> = if sample_format_for_pcm_codec(&codec).is_some() {
            if config.sampleRate == 0 || config.numberOfChannels == 0 {
                Err(Error::Type(
                    c"sampleRate and numberOfChannels must be non-zero".to_owned(),
                ))
            } else {
                Ok(true)
            }
        } else {
            let engine_config = audio_encoder_config_to_engine(config);
            match validate_encoder_bitrate(&engine_config).and_then(|()| {
                validate_audio_encoder_config(
                    &engine_config,
                    encoder_backend_available(&engine_config),
                )
            }) {
                Err(error) => Err(map_error(&error)),
                // Honest probe: well-formed configs without an encoder element
                // (or without the gst audio backend in this build) report
                // supported=false without throwing.
                Ok(ConfigValidation::Supported(_)) => Ok(true),
                Ok(ConfigValidation::Unsupported(_)) => Ok(false),
            }
        };
        match outcome {
            Err(error) => promise.reject_error(&mut cx, error),
            Ok(supported) => promise.resolve_native(
                &mut cx,
                &AudioEncoderSupport {
                    supported: Some(supported),
                    // The generated dict has no `Clone`, so the echo copy
                    // rebuilds each member (all are `Copy`/`Clone` scalars).
                    config: Some(AudioEncoderConfig {
                        codec: config.codec.clone(),
                        sampleRate: config.sampleRate,
                        numberOfChannels: config.numberOfChannels,
                        bitrate: config.bitrate,
                        bitrateMode: config.bitrateMode,
                    }),
                },
            ),
        }
        Ok(promise)
    }
}
