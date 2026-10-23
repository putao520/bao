/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `VideoDecoder` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#videodecoder-interface> + the WPT
//! video-decoder oracle). The decode backend is the component GStreamer
//! pipeline, gated by the `webcodecs-gstreamer` feature: `is_available_for`
//! is the honest capability probe (gst decoder-element presence), so a
//! platform without gst reports `supported=false` and fires the `error`
//! callback on configure — nothing is fabricated.
//!
//! Registered boundaries:
//! - Decoded frames are delivered to the `output` callback at the `flush()`
//!   drain point (the component control plane queues until flush).
//! - `flush()` settles in a microtask so `reset()` can abort an in-flight
//!   flush with AbortError (component `flush_aborted` model).
//! - The `error`/`output` callbacks fire synchronously with the event that
//!   caused them (Chrome defers to a task; registered divergence).
//! - The gst pipeline holds no JS/GC pointers, so the backend slot traces as
//!   nothing (the `unsafe_no_jsmanaged_fields` pattern, kept local to this
//!   module).

use std::cell::Cell;
use std::ptr;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::JSObject;
use js::rust::HandleObject;
use js::typedarray::{HeapArrayBufferView, Uint8};
use script_bindings::callback::TracedCallback;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;
use script_bindings::root::Dom;

use crate::dom::bindings::buffer_source::create_buffer_source;
use crate::dom::bindings::callback::ExceptionHandling;
// `VideoDecoder` and its dictionaries/callbacks live in `WebCodecs.webidl`;
// the generated module is the file module `WebCodecsBinding`.
use crate::dom::bindings::codegen::Bindings::EncodedVideoChunkBinding::EncodedVideoChunkType;
use crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoColorSpaceInit;
use crate::dom::bindings::codegen::Bindings::WebCodecsBinding::{
    CodecState, VideoDecoderConfig, VideoDecoderInit, VideoDecoderMethods, VideoDecoderSupport,
    VideoFrameOutputCallback, WebCodecsErrorCallback,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::trace::JSTraceable;
use crate::dom::domexception::{DOMErrorName, DOMException};
use crate::dom::encodedvideochunk::EncodedVideoChunk;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::videoframe::VideoFrame;
use crate::dom::webcodecs_error::{extract_description, map_error, video_config_to_engine};
use crate::realms::enter_auto_realm;
use crate::runtime::job_queue::MicrotaskRunnable;
use webcodecs::config::{ConfigValidation, validate_video_config};
use webcodecs::video_decoder::EncodedVideoChunk as EngineChunk;

#[cfg(feature = "webcodecs-gstreamer")]
use webcodecs::gst_video_decoder::{GstreamerVideoDecoderBackend, VideoDecoderBackend};
#[cfg(not(feature = "webcodecs-gstreamer"))]
use webcodecs::video_decoder::EncodedVideoChunkType as EngineChunkType;

/// Opaque decode backend slot. The gst pipeline is `Send`, holds no JS
/// pointers, and is not GC-managed: tracing it is a no-op
/// (`unsafe_no_jsmanaged_fields` shape, mirrored from dom/macros.rs locally
/// so `trace.rs` stays untouched). Size accounting is zero like the JobQueue
/// opaque handle.
#[cfg(feature = "webcodecs-gstreamer")]
type VideoBackendBox = Box<dyn VideoDecoderBackend>;
#[cfg(not(feature = "webcodecs-gstreamer"))]
type VideoBackendBox = ();

struct DecodeBackendSlot(Option<VideoBackendBox>);

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

/// The honest platform probe for video decode capability on this config.
fn video_backend_available(config: &webcodecs::config::VideoDecoderConfig) -> bool {
    #[cfg(feature = "webcodecs-gstreamer")]
    {
        GstreamerVideoDecoderBackend::is_available_for(config)
    }
    #[cfg(not(feature = "webcodecs-gstreamer"))]
    {
        let _ = config;
        false
    }
}

/// Build the decode backend slot for a validated config.
#[cfg(feature = "webcodecs-gstreamer")]
fn make_backend(config: &webcodecs::config::VideoDecoderConfig) -> Option<DecodeBackendSlot> {
    GstreamerVideoDecoderBackend::new(config)
        .ok()
        .map(|backend| DecodeBackendSlot(Some(Box::new(backend) as Box<dyn VideoDecoderBackend>)))
}

#[cfg(not(feature = "webcodecs-gstreamer"))]
fn make_backend(_config: &webcodecs::config::VideoDecoderConfig) -> Option<DecodeBackendSlot> {
    None
}

/// <https://www.w3.org/TR/webcodecs/#videodecoder>
#[dom_struct]
pub(crate) struct VideoDecoder {
    eventtarget: EventTarget,
    state: Cell<CodecState>,
    /// Configured codec metadata carried for frame delivery (rotation/flip
    /// from the config dictionary).
    rotation: Cell<f64>,
    flip: Cell<bool>,
    /// Chunks queued by `decode()`, drained by the flush microtask
    /// (spec decodeQueueSize).
    pending: DomRefCell<Vec<QueuedChunk>>,
    /// Set by `reset()` while a flush is in flight; the flush microtask maps
    /// it to an AbortError rejection.
    flush_aborted: Cell<bool>,
    backend: DomRefCell<DecodeBackendSlot>,
    output: DomRefCell<Option<TracedCallback<VideoFrameOutputCallback>>>,
    error_callback: DomRefCell<Option<TracedCallback<WebCodecsErrorCallback>>>,
}

/// One queued encoded chunk, converted to the engine form at `decode()` time
/// (the gst pipeline consumes plain data).
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct QueuedChunk {
    key: bool,
    timestamp: i64,
    duration: Option<i64>,
    data: Vec<u8>,
}

impl QueuedChunk {
    fn to_engine(&self) -> EngineChunk {
        EngineChunk {
            chunk_type: engine_chunk_type(self.key),
            timestamp: self.timestamp,
            duration: self.duration,
            data: self.data.clone(),
        }
    }
}

#[cfg(feature = "webcodecs-gstreamer")]
fn engine_chunk_type(key: bool) -> webcodecs::video_decoder::EncodedVideoChunkType {
    if key {
        webcodecs::video_decoder::EncodedVideoChunkType::Key
    } else {
        webcodecs::video_decoder::EncodedVideoChunkType::Delta
    }
}

#[cfg(not(feature = "webcodecs-gstreamer"))]
fn engine_chunk_type(key: bool) -> EngineChunkType {
    if key {
        EngineChunkType::Key
    } else {
        EngineChunkType::Delta
    }
}

impl VideoDecoder {
    fn new_inherited(
        output: TracedCallback<VideoFrameOutputCallback>,
        error_callback: TracedCallback<WebCodecsErrorCallback>,
    ) -> VideoDecoder {
        VideoDecoder {
            eventtarget: EventTarget::new_inherited(),
            state: Cell::new(CodecState::Unconfigured),
            rotation: Cell::new(0.0),
            flip: Cell::new(false),
            pending: DomRefCell::new(Vec::new()),
            flush_aborted: Cell::new(false),
            backend: DomRefCell::new(DecodeBackendSlot(None)),
            output: DomRefCell::new(Some(output)),
            error_callback: DomRefCell::new(Some(error_callback)),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoDecoderInit,
    ) -> DomRoot<VideoDecoder> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(VideoDecoder::new_inherited(
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
        *self.backend.borrow_mut() = DecodeBackendSlot(None);
        self.pending.borrow_mut().clear();
    }

    fn take_flush_aborted(&self) -> bool {
        if self.flush_aborted.get() {
            self.flush_aborted.set(false);
            true
        } else {
            false
        }
    }

    /// Drain the queued chunks through the gst pipeline and deliver every
    /// decoded frame to the `output` callback as a `VideoFrame`.
    fn drain_pending(&self) -> Fallible<()> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        #[cfg(feature = "webcodecs-gstreamer")]
        let mut backend_guard = self.backend.borrow_mut();
        #[cfg(feature = "webcodecs-gstreamer")]
        let Some(backend) = backend_guard.0.as_mut() else {
            return Ok(());
        };
        #[cfg(feature = "webcodecs-gstreamer")]
        {
            let global = self.global();
            let mut cx = Self::thread_cx();
            let decode_result: Result<Vec<_>, _> = (|| {
                for chunk in &pending {
                    backend.decode(chunk.to_engine())?;
                }
                backend.flush()
            })();
            // Release the backend guard *before* any failure path touches
            // `close()` (which re-borrows `backend`), mirroring the audio
            // decoder's guard discipline.
            drop(backend_guard);
            let frames = match decode_result {
                Ok(frames) => frames,
                Err(error) => {
                    self.fire_error(DOMErrorName::NotSupportedError);
                    self.close();
                    return Err(map_error(&error));
                },
            };
            for frame in &frames {
                let Ok(video_frame) = VideoFrame::from_decoded_frame(&mut cx, &global, frame)
                else {
                    self.fire_error(DOMErrorName::NotSupportedError);
                    self.close();
                    return Err(Error::NotSupported(None));
                };
                let Some(callback) = self.output.borrow().clone() else {
                    continue;
                };
                let _ = callback.Call_(&mut cx, &*global, &video_frame, ExceptionHandling::Report);
            }
        }
        // Without a gst backend there is nothing to drain (configure never
        // reaches Configured on such platforms); queued chunks are dropped
        // with the queue, mirroring the component's queue drain.
        #[cfg(not(feature = "webcodecs-gstreamer"))]
        {
            let _ = pending;
        }
        Ok(())
    }
}

/// Settles the `flush()` promise: honors the reset-abort flag, then drains
/// the queued chunks through the backend.
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct VideoDecoderFlushMicrotask {
    decoder: Dom<VideoDecoder>,
    global: Dom<GlobalScope>,
    promise: Dom<Promise>,
}

impl MicrotaskRunnable for VideoDecoderFlushMicrotask {
    fn handler(&self, cx: &mut JSContext) {
        // MicrotaskRunnable contract: handlers manage their own realm entry.
        let mut realm = enter_auto_realm(cx, &*self.global);
        let cx = &mut realm.current_realm();
        if self.decoder.take_flush_aborted() {
            self.promise.reject_error(cx, Error::Abort(None));
            return;
        }
        match self.decoder.drain_pending() {
            Ok(()) => self.promise.resolve_native(cx, &()),
            Err(error) => self.promise.reject_error(cx, error),
        }
    }
}

/// Honest configure failure surface: error callback with NotSupportedError
/// + close (WPT shouldError contract). Syntax failures throw from configure
/// instead and never reach here.
fn configure_unsupported(decoder: &VideoDecoder) {
    decoder.fire_error(DOMErrorName::NotSupportedError);
    decoder.close();
}

impl VideoDecoderMethods<crate::DomTypeHolder> for VideoDecoder {
    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-videodecoder>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoDecoderInit,
    ) -> Fallible<DomRoot<VideoDecoder>> {
        Ok(VideoDecoder::new(cx, global, proto, init))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-state>
    fn State(&self) -> CodecState {
        self.state.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-decodequeuesize>
    fn DecodeQueueSize(&self) -> u32 {
        self.pending.borrow().len() as u32
    }

    event_handler!(dequeue, GetOndequeue, SetOndequeue);

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-configure>
    fn Configure(&self, config: &VideoDecoderConfig) -> Fallible<()> {
        if self.state.get() == CodecState::Closed {
            return Err(Error::InvalidState(None));
        }
        let engine_config =
            video_config_to_engine(config, extract_description(config.description.as_ref()));
        match validate_video_config(&engine_config, video_backend_available(&engine_config)) {
            Err(error) => Err(map_error(&error)),
            Ok(ConfigValidation::Unsupported(_)) => {
                configure_unsupported(self);
                Ok(())
            },
            Ok(ConfigValidation::Supported(_)) => {
                let Some(slot) = make_backend(&engine_config) else {
                    configure_unsupported(self);
                    return Ok(());
                };
                *self.backend.borrow_mut() = slot;
                self.rotation.set(*config.rotation);
                self.flip.set(config.flip);
                self.state.set(CodecState::Configured);
                self.flush_aborted.set(false);
                Ok(())
            },
        }
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-decode>
    fn Decode(&self, chunk: &EncodedVideoChunk) -> Fallible<()> {
        if self.state.get() != CodecState::Configured {
            return Err(Error::InvalidState(None));
        }
        self.pending.borrow_mut().push(QueuedChunk {
            key: chunk.chunk_type() == EncodedVideoChunkType::Key,
            timestamp: chunk.chunk_timestamp(),
            duration: chunk.chunk_duration(),
            data: chunk.payload().to_vec(),
        });
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-flush>
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
            Box::new(VideoDecoderFlushMicrotask {
                decoder: Dom::from_ref(self),
                global: Dom::from_ref(&*global),
                promise: Dom::from_ref(&*promise),
            }),
        );
        Ok(promise)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-reset>
    fn Reset(&self) -> Fallible<()> {
        if self.state.get() == CodecState::Configured {
            self.state.set(CodecState::Unconfigured);
            *self.backend.borrow_mut() = DecodeBackendSlot(None);
            self.pending.borrow_mut().clear();
            // A reset while a flush is in flight aborts that flush (spec).
            self.flush_aborted.set(true);
        }
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-close>
    fn Close(&self) -> Fallible<()> {
        self.close();
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videodecoder-isconfigsupported>
    fn IsConfigSupported(
        global: &GlobalScope,
        config: &VideoDecoderConfig,
    ) -> Fallible<RootedPromise> {
        let mut cx = VideoDecoder::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        let engine_config =
            video_config_to_engine(config, extract_description(config.description.as_ref()));
        let outcome: Result<bool, Error> =
            match validate_video_config(&engine_config, video_backend_available(&engine_config)) {
                Err(error) => Err(map_error(&error)),
                Ok(ConfigValidation::Unsupported(_)) => Ok(false),
                Ok(ConfigValidation::Supported(_)) => Ok(true),
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
                    &VideoDecoderSupport {
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
/// metadata delivery uses.
fn webcodecs_support_config(
    cx: &mut JSContext,
    config: &VideoDecoderConfig,
) -> Fallible<script_bindings::trace::RootedTraceableBox<VideoDecoderConfig>> {
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
        VideoDecoderConfig {
            codec: config.codec.clone(),
            description,
            codedWidth: config.codedWidth,
            codedHeight: config.codedHeight,
            displayAspectWidth: config.displayAspectWidth,
            displayAspectHeight: config.displayAspectHeight,
            colorSpace: config
                .colorSpace
                .as_ref()
                .map(|color_space| VideoColorSpaceInit {
                    primaries: color_space.primaries.clone(),
                    transfer: color_space.transfer.clone(),
                    matrix: color_space.matrix.clone(),
                    fullRange: color_space.fullRange,
                }),
            hardwareAcceleration: config.hardwareAcceleration,
            optimizeForLatency: config.optimizeForLatency,
            rotation: config.rotation,
            flip: config.flip,
        },
    ))
}
