/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `VideoEncoder` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#videoencoder-interface>). Same honest
//! no-engine surface as `AudioEncoder` (task ruling): `configure()` validates
//! the dictionary syntax face and then fires the `error` callback with
//! NotSupportedError + close; `isConfigSupported()` rejects invalid configs
//! and resolves `{supported: false}` otherwise; `encode()`/`flush()` on a
//! non-configured encoder throw/reject InvalidStateError.

use std::cell::Cell;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::callback::TracedCallback;
use script_bindings::cell::DomRefCell;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;

use crate::dom::bindings::callback::ExceptionHandling;
// `VideoEncoder` and its dictionaries live in `WebCodecs.webidl`; the
// generated module is the file module `WebCodecsBinding`.
use crate::dom::bindings::codegen::Bindings::WebCodecsBinding::{
    CodecState, VideoEncoderConfig, VideoEncoderEncodeOptions, VideoEncoderInit,
    VideoEncoderMethods, VideoEncoderSupport, WebCodecsErrorCallback,
};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::domexception::{DOMErrorName, DOMException};
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::videoframe::VideoFrame;
use crate::dom::webcodecs_error::{codec_string, encoder_syntax_face};

/// <https://www.w3.org/TR/webcodecs/#videoencoder>
#[dom_struct]
pub(crate) struct VideoEncoder {
    eventtarget: EventTarget,
    state: Cell<CodecState>,
    error_callback: DomRefCell<Option<TracedCallback<WebCodecsErrorCallback>>>,
}

impl VideoEncoder {
    fn new_inherited(error_callback: TracedCallback<WebCodecsErrorCallback>) -> VideoEncoder {
        VideoEncoder {
            eventtarget: EventTarget::new_inherited(),
            state: Cell::new(CodecState::Unconfigured),
            error_callback: DomRefCell::new(Some(error_callback)),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoEncoderInit,
    ) -> DomRoot<VideoEncoder> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(VideoEncoder::new_inherited(init.error.root(cx).to_traced())),
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
}

impl VideoEncoderMethods<crate::DomTypeHolder> for VideoEncoder {
    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-videoencoder>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoEncoderInit,
    ) -> Fallible<DomRoot<VideoEncoder>> {
        Ok(VideoEncoder::new(cx, global, proto, init))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-state>
    fn State(&self) -> CodecState {
        self.state.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-encodequeuesize>
    fn EncodeQueueSize(&self) -> u32 {
        0
    }

    event_handler!(dequeue, GetOndequeue, SetOndequeue);

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-configure>
    fn Configure(
        &self,
        config: &crate::dom::bindings::codegen::Bindings::WebCodecsBinding::VideoEncoderConfig,
    ) -> Fallible<()> {
        if self.state.get() == CodecState::Closed {
            return Err(Error::InvalidState(None));
        }
        encoder_syntax_face(
            &codec_string(&config.codec),
            &[
                ("width", u64::from(config.width)),
                ("height", u64::from(config.height)),
            ],
        )?;
        // No encoder engine in Wave A: fire NotSupportedError + close (the
        // honest face the task ruling mandates).
        self.fire_error(DOMErrorName::NotSupportedError);
        self.state.set(CodecState::Closed);
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-encode>
    fn Encode(&self, _frame: &VideoFrame, _options: &VideoEncoderEncodeOptions) -> Fallible<()> {
        if self.state.get() != CodecState::Configured {
            return Err(Error::InvalidState(None));
        }
        // Unreachable in Wave A (configure always closes); fail closed.
        Err(Error::InvalidState(None))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-flush>
    fn Flush(&self) -> Fallible<RootedPromise> {
        let global = self.global();
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        // The encoder never reaches Configured in Wave A, so flush always
        // rejects with InvalidStateError.
        promise.reject_error(&mut cx, Error::InvalidState(None));
        Ok(promise)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-reset>
    fn Reset(&self) -> Fallible<()> {
        if self.state.get() == CodecState::Configured {
            self.state.set(CodecState::Unconfigured);
        }
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-close>
    fn Close(&self) -> Fallible<()> {
        self.state.set(CodecState::Closed);
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoencoder-isconfigsupported>
    fn IsConfigSupported(
        global: &GlobalScope,
        config: &crate::dom::bindings::codegen::Bindings::WebCodecsBinding::VideoEncoderConfig,
    ) -> Fallible<RootedPromise> {
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        match encoder_syntax_face(
            &codec_string(&config.codec),
            &[
                ("width", u64::from(config.width)),
                ("height", u64::from(config.height)),
            ],
        ) {
            Err(error) => promise.reject_error(&mut cx, error),
            Ok(()) => promise.resolve_native(
                &mut cx,
                &VideoEncoderSupport {
                    // Honest: no encoder backend exists in Wave A.
                    supported: Some(false),
                    // The generated dict has no `Clone`, so the echo copy
                    // rebuilds each member (all are `Copy`/`Clone` scalars).
                    config: Some(VideoEncoderConfig {
                        codec: config.codec.clone(),
                        width: config.width,
                        height: config.height,
                        displayWidth: config.displayWidth,
                        displayHeight: config.displayHeight,
                        bitrate: config.bitrate,
                        framerate: config.framerate,
                        hardwareAcceleration: config.hardwareAcceleration,
                        alpha: config.alpha,
                        scalabilityMode: config.scalabilityMode.clone(),
                        bitrateMode: config.bitrateMode,
                        latencyMode: config.latencyMode,
                        contentHint: config.contentHint.clone(),
                    }),
                },
            ),
        }
        Ok(promise)
    }
}
