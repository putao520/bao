/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `ImageDecoder` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://wicg.github.io/image-output/> + the WPT image-decoder oracle).
//! The decode plane is the component image-crate backend (png/gif/jpeg/webp/
//! bmp/ico real decode; avif honestly unsupported).
//!
//! Shape mandated by the WPT oracle: the constructor NEVER throws — an
//! unsupported type (`junk/type` with a decodable body) or a failed decode
//! constructs successfully and marks the decoder failed, so `decode()` and
//! `tracks.ready` reject NotSupportedError afterwards. `decode()` decodes
//! lazily (a truncated body still constructs; its decode rejects — the WPT
//! truncated case expects an EncodingError face, our mapping lands on
//! NotSupportedError: registered divergence, follow-up candidate).
//!
//! Registered boundaries (webidl header + this slice): the data source is
//! `BufferSource` only (no ReadableStream incremental delivery);
//! `colorSpaceConversion`/`desiredWidth`/`desiredHeight` are accepted but not
//! applied; `reset()` is a no-op (all decodes complete synchronously).

use std::cell::Cell;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{DomObject, Reflector, reflect_dom_object_with_proto};
use script_bindings::root::Dom;

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
use crate::dom::bindings::codegen::Bindings::ImageDecoderBinding::{
    ImageDecodeOptions, ImageDecodeResult, ImageDecoderInit, ImageDecoderMethods,
};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{DomRoot, MutNullableDom};
use crate::dom::globalscope::GlobalScope;
use crate::dom::imagetrack::ImageTrack;
use crate::dom::imagetracklist::ImageTrackList;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::videoframe::VideoFrame;
use webcodecs::image_decoder::{decode_image, is_type_supported};
use webcodecs::video_frame::{DecodedVideoFrame, FrameColorSpace, FrameFormat};

/// Snapshot of one decoded RGBA frame (plain data; the component
/// `ImageFrame` is not traceable so the DOM layer owns its copy).
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct ImageFrameSnapshot {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Frame delay in milliseconds (0 for stills).
    duration_ms: u32,
}

/// Snapshot of a successful decode: the frames plus the track metadata.
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct DecodedImage {
    frames: Vec<ImageFrameSnapshot>,
    animated: bool,
    frame_count: u32,
    /// `None` = infinite repetition.
    repetition_count: Option<u32>,
}

/// <https://wicg.github.io/image-output/#dom-imagedecoder>
#[dom_struct]
pub(crate) struct ImageDecoder {
    reflector_: Reflector,
    /// The `type` the decoder was constructed with.
    type_: DomRefCell<String>,
    /// Whether construction failed (unsupported type or failed decode): the
    /// deferred NotSupportedError face for `decode()` and `tracks.ready`.
    failed: Cell<bool>,
    /// Whether `close()` was called (subsequent `decode()` rejects
    /// InvalidStateError).
    closed: Cell<bool>,
    /// The decoded image when construction succeeded.
    decoded: DomRefCell<Option<DecodedImage>>,
    tracks: MutNullableDom<ImageTrackList>,
    completed: MutNullableDom<Promise>,
}

impl ImageDecoder {
    fn new_inherited(type_: String, failed: bool, decoded: Option<DecodedImage>) -> ImageDecoder {
        ImageDecoder {
            reflector_: Reflector::new(),
            type_: DomRefCell::new(type_),
            failed: Cell::new(failed),
            closed: Cell::new(false),
            decoded: DomRefCell::new(decoded),
            tracks: Default::default(),
            completed: Default::default(),
        }
    }

    #[allow(unsafe_code)]
    fn thread_cx() -> JSContext {
        unsafe { JSContext::get_from_thread() }.expect("no active JS context")
    }
}

impl ImageDecoderMethods<crate::DomTypeHolder> for ImageDecoder {
    /// <https://wicg.github.io/image-output/#dom-imagedecoder-imagedecoder>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &ImageDecoderInit,
    ) -> Fallible<DomRoot<ImageDecoder>> {
        // Copy the bytes at construction (the BufferSource is consumed
        // synchronously; a detached buffer yields zero bytes and fails the
        // decode honestly).
        let no_gc = cx.no_gc();
        let bytes = get_buffer_source_slice(&init.data, no_gc).to_vec();
        let mime = init.type_.to_string();
        // The constructor never throws (WPT junk/type oracle): unsupported
        // types and failed decodes construct into the failed state.
        let outcome = if !is_type_supported(&mime) {
            None
        } else {
            decode_image(
                &webcodecs::config::ImageDecoderConfig {
                    mime_type: mime.clone(),
                    prefer_animation: init.preferAnimation.unwrap_or(false),
                },
                &bytes,
            )
            .ok()
        };
        let decoded = outcome.map(|result| DecodedImage {
            frames: result
                .frames
                .iter()
                .map(|frame| ImageFrameSnapshot {
                    width: frame.width,
                    height: frame.height,
                    rgba: frame.rgba.clone(),
                    duration_ms: frame.duration_ms,
                })
                .collect(),
            animated: result.track.animated,
            frame_count: result.track.frame_count,
            repetition_count: result.track.repetition_count,
        });
        let failed = decoded.is_none();
        Ok(reflect_dom_object_with_proto(
            cx,
            Box::new(ImageDecoder::new_inherited(mime, failed, decoded)),
            global,
            proto,
        ))
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-type>
    fn Type(&self) -> crate::dom::bindings::str::DOMString {
        self.type_.borrow().clone().into()
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-complete>
    /// The data source is fully buffered (BufferSource-only slice), so the
    /// decoder is always complete — false only for a failed construction.
    fn Complete(&self) -> bool {
        !self.failed.get()
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-completed>
    fn Completed(&self) -> RootedPromise {
        let global = self.global();
        let global: &GlobalScope = &global;
        let mut cx = Self::thread_cx();
        let promise = self.completed.or_init(|| {
            // All data is buffered: `completed` resolves immediately (the
            // empty-data failure surface surfaces through decode() instead).
            let resolved = Promise::new_resolved(&mut cx, global, ());
            DomRoot::from_ref(&*resolved)
        });
        // Re-wrap the cached promise so every access observes the same JS
        // promise object.
        Promise::new_with_js_promise(&cx, promise.reflector().get_jsobject())
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-tracks>
    fn Tracks(&self) -> DomRoot<ImageTrackList> {
        let global = self.global();
        let global: &GlobalScope = &global;
        self.tracks.or_init(|| {
            let mut cx = Self::thread_cx();
            // The ready promise settles with the constructor outcome:
            // resolved once tracks exist, rejected NotSupportedError for an
            // unsupported type or failed decode (WPT junk/type oracle). The
            // rejection goes through `reject_error` — `Error` converts to a
            // JS value only through the DOM error path, not as a plain value.
            let ready: Dom<Promise> = if self.failed.get() {
                let promise = Promise::new(&mut cx, global);
                promise.reject_error(&mut cx, Error::NotSupported(None));
                Dom::from_ref(&*promise)
            } else {
                let promise = Promise::new_resolved(&mut cx, global, ());
                Dom::from_ref(&*promise)
            };
            let track = if self.failed.get() {
                None
            } else {
                self.decoded.borrow().as_ref().map(|decoded| {
                    ImageTrack::new_selected(
                        &mut cx,
                        global,
                        decoded.animated,
                        decoded.frame_count,
                        decoded.repetition_count,
                    )
                })
            };
            ImageTrackList::new(&mut cx, global, None, track, ready)
        })
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-decode>
    fn Decode(&self, options: &ImageDecodeOptions) -> RootedPromise {
        let global = self.global();
        let global: &GlobalScope = &global;
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        if self.closed.get() {
            promise.reject_error(&mut cx, Error::InvalidState(None));
            return promise;
        }
        if self.failed.get() {
            promise.reject_error(&mut cx, Error::NotSupported(None));
            return promise;
        }
        let decoded = self.decoded.borrow();
        let Some(decoded) = decoded.as_ref() else {
            promise.reject_error(&mut cx, Error::NotSupported(None));
            return promise;
        };
        // Out-of-range frameIndex rejects RangeError (WPT oracle).
        let Some(frame) = decoded.frames.get(options.frameIndex as usize) else {
            promise.reject_error(
                &mut cx,
                Error::Range(c"frameIndex is out of range".to_owned()),
            );
            return promise;
        };
        // The image backend normalizes to RGBA8; surface it as a VideoFrame
        // through the decoded-frame delivery path.
        let decoded_video = DecodedVideoFrame {
            format: FrameFormat::Rgba,
            coded_width: frame.width,
            coded_height: frame.height,
            display_width: frame.width,
            display_height: frame.height,
            timestamp: 0,
            duration: Some(i64::from(frame.duration_ms) * 1000),
            color_space: FrameColorSpace {
                primaries: "bt709",
                transfer: "bt709",
                matrix: "rgb",
                full_range: true,
            },
            planes: vec![frame.rgba.clone()],
            strides: vec![frame.width * 4],
        };
        match VideoFrame::from_decoded_frame(&mut cx, global, &decoded_video) {
            Ok(video_frame) => promise.resolve_native(
                &mut cx,
                &ImageDecodeResult {
                    image: DomRoot::from_ref(&*video_frame),
                    complete: true,
                },
            ),
            Err(error) => promise.reject_error(&mut cx, error),
        }
        promise
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-reset>
    /// All decodes complete synchronously in this slice, so there is no
    /// in-flight work to abort.
    fn Reset(&self) -> Fallible<()> {
        Ok(())
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-close>
    fn Close(&self) -> Fallible<()> {
        self.closed.set(true);
        Ok(())
    }

    /// <https://wicg.github.io/image-output/#dom-imagedecoder-istypesupported>
    fn IsTypeSupported(
        global: &GlobalScope,
        type_: crate::dom::bindings::str::DOMString,
    ) -> RootedPromise {
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, global);
        promise.resolve_native(&mut cx, &is_type_supported(&type_.str()));
        promise
    }
}
