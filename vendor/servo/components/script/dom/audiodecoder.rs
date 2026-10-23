/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `AudioDecoder` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#audiodecoder-interface> + the WPT
//! audio-decoder oracle).
//!
//! Two decode legs, selected by the configured codec string:
//! - **Raw PCM** (Wave A contract): the PCM codec-registry strings
//!   (`pcm-u8`/`pcm-s16`/`pcm-s32`/`pcm-f32`) map 1:1 onto interleaved
//!   `AudioSampleFormat` values, so decoding is a re-interpretation of the
//!   encoded chunk bytes as samples — no compression, resampling or priming.
//! - **Compressed audio** (Wave B): opus / mp4a (AAC) decode through the
//!   component GStreamer backend (`webcodecs-gstreamer` feature). The
//!   capability probe is the decoder *element* presence, so a platform without
//!   the element reports `supported=false` and fires the `error` callback on
//!   configure — nothing is fabricated.
//!
//! Registered boundaries:
//! - Codecs without a verified gst caps mapping (vorbis/flac/mp3) and
//!   multichannel opus (>2 channels) validate as well-formed-but-unbacked: the
//!   `error` callback fires with NotSupportedError and the decoder closes (the
//!   WPT shouldError contract).
//! - Decoded output is delivered to the `output` callback at the `flush()`
//!   drain point (both legs queue until flush); a trailing partial PCM frame is
//!   a decode error (error callback + close + rejected flush).
//! - `flush()` settles in a microtask so `reset()` can abort an in-flight
//!   flush with AbortError (the spec reset-during-flush contract, modeled by
//!   the component `flush_aborted` flag).
//! - The `error`/`output` callbacks fire synchronously with the event that
//!   caused them (Chrome defers to a task; no oracle test observes the
//!   ordering — registered divergence).

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
use std::cell::Cell;
use std::ptr;
use stylo_atoms::Atom;

use crate::dom::audiodata::AudioData;
use crate::dom::bindings::buffer_source::create_buffer_source;
use crate::dom::bindings::callback::ExceptionHandling;
use crate::dom::bindings::codegen::Bindings::AudioDataBinding::AudioSampleFormat;
// `AudioDecoder` and its dictionaries/callbacks/enums live in
// `WebCodecs.webidl`, so the generated module is the file module
// `WebCodecsBinding` (it re-exports each interface's `Methods` trait).
use crate::dom::bindings::codegen::Bindings::WebCodecsBinding::{
    AudioDataOutputCallback, AudioDecoderConfig, AudioDecoderInit, AudioDecoderMethods,
    AudioDecoderSupport, CodecState, WebCodecsErrorCallback,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::trace::JSTraceable;
use crate::dom::domexception::{DOMErrorName, DOMException};
use crate::dom::encodedaudiochunk::EncodedAudioChunk;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::webcodecs_error::{
    audio_config_to_engine, codec_string, extract_description, map_error,
};
use crate::dom::webcodecs_pcm::sample_format_for_pcm_codec;
use crate::realms::enter_auto_realm;
use crate::runtime::job_queue::MicrotaskRunnable;
use webcodecs::audio_data::{AudioSampleFormat as ComponentAudioSampleFormat, DecodedAudioFrame};
use webcodecs::config::{ConfigValidation, validate_audio_config};

#[cfg(feature = "webcodecs-gstreamer")]
use webcodecs::audio_decoder::EncodedAudioChunk as EngineAudioChunk;
#[cfg(feature = "webcodecs-gstreamer")]
use webcodecs::gst_audio_decoder::{AudioDecoderBackend, GstreamerAudioDecoderBackend};

/// Backend handle type for the compressed-audio leg. Kept as an alias so the
/// slot keeps its `Option<…>` shape in both feature configurations (a
/// `#[cfg]`-stripped tuple field would make `DecodeBackendSlot(None)`
/// ill-formed in no-gst builds — this shape compiles on every platform, per
/// CLAUDE.md Windows 交叉编译准入律).
#[cfg(feature = "webcodecs-gstreamer")]
type AudioBackendBox = Box<dyn AudioDecoderBackend>;
#[cfg(not(feature = "webcodecs-gstreamer"))]
type AudioBackendBox = ();

/// Opaque decode backend slot for the compressed-audio (gst) leg. The pipeline
/// is `Send`, holds no JS pointers, and is not GC-managed: tracing is a no-op
/// (`unsafe_no_jsmanaged_fields` shape, mirrored from `videodecoder.rs`).
struct DecodeBackendSlot(Option<AudioBackendBox>);

#[expect(unsafe_code)]
unsafe impl JSTraceable for DecodeBackendSlot {
    #[inline]
    unsafe fn trace(&self, _: *mut ::js::jsapi::JSTracer) {
        // No JS-managed fields inside the backend.
    }
}

impl malloc_size_of::MallocSizeOf for DecodeBackendSlot {
    fn size_of(&self, _ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        // Opaque native handle; the pipeline's own accounting stays native.
        0
    }
}

/// Honest platform probe for compressed-audio decode capability on this
/// config (gst decoder-element presence; always false without the feature —
/// the Wave A contract for such builds).
fn audio_backend_available(config: &webcodecs::config::AudioDecoderConfig) -> bool {
    #[cfg(feature = "webcodecs-gstreamer")]
    {
        GstreamerAudioDecoderBackend::is_available_for(config)
    }
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    {
        let _ = config;
        false
    }
}

/// Build the decode backend slot for a validated config.
#[cfg(feature = "webcodecs-gstreamer")]
fn make_backend(config: &webcodecs::config::AudioDecoderConfig) -> Option<DecodeBackendSlot> {
    GstreamerAudioDecoderBackend::new(config)
        .ok()
        .map(|backend| DecodeBackendSlot(Some(Box::new(backend) as Box<dyn AudioDecoderBackend>)))
}

#[cfg(not(feature = "webcodecs-gstreamer"))]
fn make_backend(_config: &webcodecs::config::AudioDecoderConfig) -> Option<DecodeBackendSlot> {
    None
}

/// The configured PCM passthrough description (immutable while configured).
#[derive(Clone, Copy, JSTraceable, MallocSizeOf)]
struct PcmConfig {
    format: AudioSampleFormat,
    sample_rate: u32,
    number_of_channels: u32,
}

/// One queued encoded chunk, copied out of the `EncodedAudioChunk` DOM object
/// at `decode()` time. `duration` is carried so the engine chunk keeps the
/// spec's duration pass-through (`EncodedAudioChunk.duration`).
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct PendingChunk {
    timestamp: i64,
    duration: Option<i64>,
    data: Vec<u8>,
}

/// <https://www.w3.org/TR/webcodecs/#audiodecoder>
#[dom_struct]
pub(crate) struct AudioDecoder {
    eventtarget: EventTarget,
    state: Cell<CodecState>,
    /// The PCM passthrough configuration while `Configured` on the raw-PCM
    /// codec-registry strings (Wave A contract; `pcm` and `backend` are
    /// mutually exclusive).
    pcm: DomRefCell<Option<PcmConfig>>,
    /// The compressed-audio decode backend (Wave B: GStreamer opus/AAC).
    backend: DomRefCell<DecodeBackendSlot>,
    /// Chunks queued by `decode()`, drained by the flush microtask
    /// (spec decodeQueueSize).
    pending: DomRefCell<Vec<PendingChunk>>,
    /// Set by `reset()` while a flush is in flight; the flush microtask maps
    /// it to an AbortError rejection.
    flush_aborted: Cell<bool>,
    output: DomRefCell<Option<TracedCallback<AudioDataOutputCallback>>>,
    error_callback: DomRefCell<Option<TracedCallback<WebCodecsErrorCallback>>>,
}

impl AudioDecoder {
    fn new_inherited(
        output: TracedCallback<AudioDataOutputCallback>,
        error_callback: TracedCallback<WebCodecsErrorCallback>,
    ) -> AudioDecoder {
        AudioDecoder {
            eventtarget: EventTarget::new_inherited(),
            state: Cell::new(CodecState::Unconfigured),
            pcm: DomRefCell::new(None),
            backend: DomRefCell::new(DecodeBackendSlot(None)),
            pending: DomRefCell::new(Vec::new()),
            flush_aborted: Cell::new(false),
            output: DomRefCell::new(Some(output)),
            error_callback: DomRefCell::new(Some(error_callback)),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioDecoderInit,
    ) -> DomRoot<AudioDecoder> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(AudioDecoder::new_inherited(
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

    /// Fire the `error` callback with a DOMException of `name` (the WPT
    /// shouldError contract asserts `instanceof DOMException` + the name).
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
        *self.pcm.borrow_mut() = None;
        *self.backend.borrow_mut() = DecodeBackendSlot(None);
        self.pending.borrow_mut().clear();
    }

    /// Pending chunk count (inherent form of the `decodeQueueSize`
    /// attribute, for the flush microtask's dequeue decision).
    fn queued_count(&self) -> usize {
        self.pending.borrow().len()
    }

    /// Consume the reset-aborts-flush flag (spec: a reset during an in-flight
    /// flush rejects that flush with AbortError).
    fn take_flush_aborted(&self) -> bool {
        if self.flush_aborted.get() {
            self.flush_aborted.set(false);
            true
        } else {
            false
        }
    }

    /// Drain the pending queue: the raw-PCM pass-through when a PCM codec is
    /// configured, otherwise the compressed-audio backend (Wave B).
    fn drain_pending(&self) -> Fallible<()> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        // Copy the config out and release the cell before draining: the drain
        // error paths run `close()`, which re-borrows `pcm`.
        let pcm = self.pcm.borrow().as_ref().copied();
        if let Some(config) = pcm {
            return self.drain_pcm(pending, config);
        }
        self.drain_engine(pending)
    }

    /// Drain the compressed-audio backend and deliver every decoded frame to
    /// the `output` callback as an `AudioData`. The backend is borrowed for the
    /// duration of the decode+flush only; the slot is released before the
    /// error path runs `close()` (which re-borrows the slot — holding both
    /// would be a `BorrowMutError`).
    #[cfg(feature = "webcodecs-gstreamer")]
    fn drain_engine(&self, pending: Vec<PendingChunk>) -> Fallible<()> {
        let drained = {
            let mut guard = self.backend.borrow_mut();
            let Some(backend) = guard.0.as_mut() else {
                // Configure never reaches Configured without a backend.
                return Ok(());
            };
            let mut failure = None;
            for chunk in pending {
                if let Err(error) = backend.decode(EngineAudioChunk {
                    timestamp: chunk.timestamp,
                    duration: chunk.duration,
                    data: chunk.data,
                }) {
                    failure = Some(error);
                    break;
                }
            }
            match failure {
                Some(error) => Err(error),
                None => backend.flush(),
            }
        };
        let frames = match drained {
            Ok(frames) => frames,
            Err(error) => {
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                return Err(map_error(&error));
            },
        };
        let global = self.global();
        let mut cx = Self::thread_cx();
        for frame in &frames {
            let Some(audio_data) = AudioData::from_decoded_frame(&mut cx, &global, frame) else {
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                return Err(Error::NotSupported(None));
            };
            let Some(callback) = self.output.borrow().clone() else {
                continue;
            };
            let _ = callback.Call_(&mut cx, &*global, &audio_data, ExceptionHandling::Report);
        }
        Ok(())
    }

    /// Without the gst feature there is no compressed-audio backend: configure
    /// never reaches Configured, so the queue drains to nothing (Wave A
    /// contract preserved).
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    fn drain_engine(&self, _pending: Vec<PendingChunk>) -> Fallible<()> {
        Ok(())
    }

    /// Drain the pending queue through the PCM passthrough, delivering one
    /// `AudioData` per chunk to the `output` callback (webcodecs_pcm: chunks
    /// are re-interpreted as samples of the configured format). Errors close
    /// the decoder and reject the flush.
    fn drain_pcm(&self, pending: Vec<PendingChunk>, config: PcmConfig) -> Fallible<()> {
        let frame_bytes = crate::dom::webcodecs_pcm::sample_size(config.format)
            * config.number_of_channels as usize;
        let global = self.global();
        let mut cx = Self::thread_cx();
        for chunk in pending {
            if chunk.data.len() % frame_bytes != 0 {
                // A trailing partial PCM frame is a decode error: the PCM
                // codec registry has no concealment path.
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                return Err(Error::NotSupported(None));
            }
            if chunk.data.is_empty() {
                continue;
            }
            let frame = DecodedAudioFrame {
                format: component_format(config.format),
                sample_rate: f64::from(config.sample_rate),
                number_of_frames: (chunk.data.len() / frame_bytes) as u64,
                number_of_channels: u64::from(config.number_of_channels),
                timestamp: chunk.timestamp,
                data: vec![chunk.data],
            };
            let Some(audio_data) = AudioData::from_decoded_frame(&mut cx, &global, &frame) else {
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                return Err(Error::NotSupported(None));
            };
            let Some(callback) = self.output.borrow().clone() else {
                continue;
            };
            let _ = callback.Call_(&mut cx, &*global, &audio_data, ExceptionHandling::Report);
        }
        Ok(())
    }
}

/// DOM `AudioSampleFormat` → component mirror (the engine-facing frame type
/// is the component's; `AudioData::from_decoded_frame` maps it back).
fn component_format(format: AudioSampleFormat) -> ComponentAudioSampleFormat {
    match format {
        AudioSampleFormat::U8 => ComponentAudioSampleFormat::U8,
        AudioSampleFormat::S16 => ComponentAudioSampleFormat::S16,
        AudioSampleFormat::S32 => ComponentAudioSampleFormat::S32,
        AudioSampleFormat::F32 => ComponentAudioSampleFormat::F32,
        AudioSampleFormat::U8_planar => ComponentAudioSampleFormat::U8Planar,
        AudioSampleFormat::S16_planar => ComponentAudioSampleFormat::S16Planar,
        AudioSampleFormat::S32_planar => ComponentAudioSampleFormat::S32Planar,
        AudioSampleFormat::F32_planar => ComponentAudioSampleFormat::F32Planar,
    }
}

/// Settles the `flush()` promise: honors the reset-abort flag, then drains
/// the pending queue through the PCM passthrough.
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct AudioDecoderFlushMicrotask {
    decoder: Dom<AudioDecoder>,
    global: Dom<GlobalScope>,
    promise: Dom<Promise>,
}

impl MicrotaskRunnable for AudioDecoderFlushMicrotask {
    fn handler(&self, cx: &mut JSContext) {
        // MicrotaskRunnable contract: handlers manage their own realm entry.
        let mut realm = enter_auto_realm(cx, &*self.global);
        let cx = &mut realm.current_realm();
        if self.decoder.take_flush_aborted() {
            self.promise.reject_error(cx, Error::Abort(None));
            return;
        }
        // `dequeue` fires when decodeQueueSize drops to zero (the drain below
        // empties the queue); the WPT oracle asserts one event per drained
        // batch with the queue already at 0. "dequeue" is not in the static
        // atom table, so this uses the dynamic-atom constructor (same shape as
        // ServiceWorkerContainer's "controllerchange").
        let had_queued = self.decoder.queued_count() > 0;
        match self.decoder.drain_pending() {
            Ok(()) => {
                if had_queued {
                    self.decoder
                        .upcast::<EventTarget>()
                        .fire_event(cx, Atom::from("dequeue"));
                }
                self.promise.resolve_native(cx, &())
            },
            Err(error) => self.promise.reject_error(cx, error),
        }
    }
}

impl AudioDecoderMethods<crate::DomTypeHolder> for AudioDecoder {
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-audiodecoder>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioDecoderInit,
    ) -> Fallible<DomRoot<AudioDecoder>> {
        Ok(AudioDecoder::new(cx, global, proto, init))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-state>
    fn State(&self) -> CodecState {
        self.state.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-decodequeuesize>
    fn DecodeQueueSize(&self) -> u32 {
        self.pending.borrow().len() as u32
    }

    // `ondequeue`: the dequeue event fires when decodeQueueSize transitions
    // down to zero; Wave A drains synchronously inside flush, so the
    // transition is observable through the attribute immediately (the handler
    // surface itself is fully wired via event_handler!).
    event_handler!(dequeue, GetOndequeue, SetOndequeue);

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-configure>
    fn Configure(&self, config: &AudioDecoderConfig) -> Fallible<()> {
        if self.state.get() == CodecState::Closed {
            return Err(Error::InvalidState(None));
        }
        let codec = crate::dom::webcodecs_error::codec_string(&config.codec);
        // The PCM codec-registry strings stay on the Wave A pass-through path
        // (webcodecs_pcm contract). The component codec registry does not carry
        // the PCM strings, so that face validates DOM-side; every other codec
        // validates through the component against the honest gst probe.
        if let Some(format) = sample_format_for_pcm_codec(&codec) {
            if config.sampleRate == 0 {
                return Err(Error::Type(c"sampleRate must be non-zero".to_owned()));
            }
            if config.numberOfChannels == 0 {
                return Err(Error::Type(c"numberOfChannels must be non-zero".to_owned()));
            }
            // Reconfiguring from the compressed leg drops that pipeline.
            *self.backend.borrow_mut() = DecodeBackendSlot(None);
            *self.pcm.borrow_mut() = Some(PcmConfig {
                format,
                sample_rate: config.sampleRate,
                number_of_channels: config.numberOfChannels,
            });
            self.state.set(CodecState::Configured);
            self.flush_aborted.set(false);
            return Ok(());
        }
        let engine_config =
            audio_config_to_engine(config, extract_description(config.description.as_ref()));
        match validate_audio_config(&engine_config, audio_backend_available(&engine_config)) {
            Err(error) => Err(map_error(&error)),
            // Well-formed-but-unbacked (no gst element for this codec, or the
            // build has no gst audio backend): fail closed with the error
            // callback + closed, per the WPT shouldError contract.
            Ok(ConfigValidation::Unsupported(_)) => {
                self.fire_error(DOMErrorName::NotSupportedError);
                self.close();
                Ok(())
            },
            Ok(ConfigValidation::Supported(_)) => {
                let Some(slot) = make_backend(&engine_config) else {
                    self.fire_error(DOMErrorName::NotSupportedError);
                    self.close();
                    return Ok(());
                };
                *self.pcm.borrow_mut() = None;
                *self.backend.borrow_mut() = slot;
                self.state.set(CodecState::Configured);
                self.flush_aborted.set(false);
                Ok(())
            },
        }
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-decode>
    fn Decode(&self, chunk: &EncodedAudioChunk) -> Fallible<()> {
        if self.state.get() != CodecState::Configured {
            // Spec: decode() on an unconfigured/closed decoder throws
            // InvalidStateError (component WebCodecsUnconfigured/Closed codes).
            return Err(Error::InvalidState(None));
        }
        self.pending.borrow_mut().push(PendingChunk {
            timestamp: chunk.chunk_timestamp(),
            duration: chunk.chunk_duration(),
            data: chunk.payload().to_vec(),
        });
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-flush>
    fn Flush(&self) -> Fallible<RootedPromise> {
        let global = self.global();
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        if self.state.get() != CodecState::Configured {
            promise.reject_error(&mut cx, Error::InvalidState(None));
            return Ok(promise);
        }
        // Settle asynchronously (microtask) so reset() can abort an
        // in-flight flush with AbortError.
        global.enqueue_microtask(
            &cx,
            Box::new(AudioDecoderFlushMicrotask {
                decoder: Dom::from_ref(self),
                global: Dom::from_ref(&*global),
                promise: Dom::from_ref(&*promise),
            }),
        );
        Ok(promise)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-reset>
    fn Reset(&self) -> Fallible<()> {
        if self.state.get() == CodecState::Configured {
            self.state.set(CodecState::Unconfigured);
            *self.pcm.borrow_mut() = None;
            *self.backend.borrow_mut() = DecodeBackendSlot(None);
            self.pending.borrow_mut().clear();
            // A reset while a flush is in flight aborts that flush (spec) —
            // the flush microtask maps this flag to AbortError. It is
            // cleared by the next successful configure.
            self.flush_aborted.set(true);
        }
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-close>
    fn Close(&self) -> Fallible<()> {
        self.close();
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodecoder-isconfigsupported>
    fn IsConfigSupported(
        global: &GlobalScope,
        config: &AudioDecoderConfig,
    ) -> Fallible<RootedPromise> {
        let mut cx = AudioDecoder::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        let codec = crate::dom::webcodecs_error::codec_string(&config.codec);
        let outcome: Result<bool, Error> = if sample_format_for_pcm_codec(&codec).is_some() {
            if config.sampleRate == 0 || config.numberOfChannels == 0 {
                Err(Error::Type(
                    c"sampleRate and numberOfChannels must be non-zero".to_owned(),
                ))
            } else {
                Ok(true)
            }
        } else {
            let engine_config =
                audio_config_to_engine(config, extract_description(config.description.as_ref()));
            match validate_audio_config(&engine_config, audio_backend_available(&engine_config)) {
                Err(error) => Err(map_error(&error)),
                // Honest probe: well-formed configs without a gst decoder
                // element (or a build without the gst audio backend) report
                // supported=false without throwing.
                Ok(ConfigValidation::Supported(_)) => Ok(true),
                Ok(ConfigValidation::Unsupported(_)) => Ok(false),
            }
        };
        match outcome {
            Err(error) => promise.reject_error(&mut cx, error),
            Ok(supported) => {
                // Built before the resolve call: both steps need `&mut cx`,
                // and a nested borrow inside the argument list would be a
                // second mutable borrow of `cx`.
                let support_config = webcodecs_support_config(&mut cx, config)?;
                promise.resolve_native(
                    &mut cx,
                    &AudioDecoderSupport {
                        supported: Some(supported),
                        config: Some(support_config),
                    },
                );
            },
        }
        Ok(promise)
    }
}

/// Build the `config` member of the `*DecoderSupport` result. The generated
/// dict is stored `RootedTraceableBox`-wrapped (it carries a buffer-source
/// member) and has no `Clone` (buffer-source unions are not cloneable), so the
/// copy rebuilds each member; the `description` bytes are re-materialized as a
/// fresh `Uint8Array` view — the same construction mechanics the encoder's
/// `decoderConfig.description` delivery uses.
fn webcodecs_support_config(
    cx: &mut JSContext,
    config: &AudioDecoderConfig,
) -> Fallible<script_bindings::trace::RootedTraceableBox<AudioDecoderConfig>> {
    let description = match extract_description(config.description.as_ref()) {
        Some(bytes) if !bytes.is_empty() => {
            rooted!(&in(cx) let mut object = ptr::null_mut::<JSObject>());
            create_buffer_source::<Uint8>(cx, &bytes, object.handle_mut())
                .map_err(|_| Error::JSFailed)?;
            let view = HeapArrayBufferView::from(object.get()).map_err(|_| Error::JSFailed)?;
            Some(ArrayBufferViewOrArrayBuffer::ArrayBufferView(
                script_bindings::trace::RootedTraceableBox::new(view),
            ))
        },
        _ => None,
    };
    Ok(script_bindings::trace::RootedTraceableBox::new(
        AudioDecoderConfig {
            codec: config.codec.clone(),
            sampleRate: config.sampleRate,
            numberOfChannels: config.numberOfChannels,
            description,
        },
    ))
}
