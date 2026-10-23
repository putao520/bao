/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! <https://github.com/ricea/websocketstream-explainer>
//!
//! `WebSocketStream` is the promise/streams-shaped WebSocket API: instead of
//! events it exposes `opened` (resolving to `{readable, writable, protocol,
//! extensions}`) and `closed` (resolving to `{closeCode, reason}` on a clean
//! close, rejecting with a `WebSocketError` otherwise). The transport is the
//! existing servo WebSocket pipeline (`FetchChannels::WebSocket`), so the DOM
//! layer only translates between `WebSocketNetworkEvent`s and the
//! `ReadableStream`/`WritableStream` machinery.

use std::cell::Cell;
use std::collections::VecDeque;
use std::ptr;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::conversions::{ToJSValConvertible, jsstr_to_string};
use js::jsapi::{JSObject, JSPROP_ENUMERATE};
use js::jsval::{ObjectValue, UndefinedValue};
use js::rust::HandleValue as SafeHandleValue;
use js::rust::wrappers2::{JS_DefineProperty, JS_NewObject};
use net_traits::blob_url_store::UrlWithBlobClaim;
use net_traits::request::{
    CacheMode, CredentialsMode, RedirectMode, Referrer, RequestBuilder, RequestMode,
    ServiceWorkersMode,
};
use net_traits::{FetchChannels, MessageData, WebSocketDomAction, WebSocketNetworkEvent};
use profile_traits::generic_callback::GenericCallback as ProfileGenericCallback;
use script_bindings::cell::DomRefCell;
use script_bindings::conversions::native_from_object;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};
use script_bindings::str::{DOMString, USVString, is_token};
use servo_base::generic_channel::{LazyCallback, lazy_callback};
use servo_url::ServoUrl;

use crate::dom::abortsignal::{AbortAlgorithm, AbortSignal};
use crate::dom::bindings::codegen::Bindings::WebSocketStreamBinding::{
    WebSocketCloseInfo, WebSocketStreamMethods, WebSocketStreamOptions,
};
use crate::dom::bindings::error::{Error, ErrorToJsval, Fallible};
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot, MutNullableDom};
use crate::dom::csp::{GlobalCspReporting, Violation};
use crate::dom::document::websocketerror::WebSocketError;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise, TracedPromise};
use crate::dom::readablestream::{ReadableStream, create_readable_stream};
use crate::dom::stream::countqueuingstrategy::extract_size_algorithm;
use crate::dom::stream::underlyingsourcecontainer::UnderlyingSourceType;
use crate::dom::stream::writablestreamdefaultcontroller::UnderlyingSinkType;
use crate::dom::writablestream::{WritableStream, create_writable_stream};
use crate::fetch::fetch::{RequestWithGlobalScope, send_fetch};
use crate::tasks::task::TaskOnce;
use crate::tasks::task_source::SendableTaskSource;

// Close codes defined in RFC 6455 §7.4.1.
mod close_code {
    pub(crate) const NORMAL: u16 = 1000;
    pub(crate) const NO_STATUS: u16 = 1005;
    pub(crate) const ABNORMAL: u16 = 1006;
}

/// The four states of a `WebSocketStream` connection. The stream objects
/// (readable/writable) are created at construction time; only the network leg
/// moves between states.
#[derive(Clone, Copy, JSTraceable, MallocSizeOf, PartialEq)]
enum WebSocketStreamState {
    Connecting,
    Open,
    Closing,
    Closed,
}

/// <https://github.com/ricea/websocketstream-explainer#websocketstream>
#[dom_struct]
pub(crate) struct WebSocketStream {
    reflector_: Reflector,
    #[no_trace]
    url: ServoUrl,
    state: Cell<WebSocketStreamState>,
    /// Whether the `opened` promise has settled (resolved or rejected).
    opened_settled: Cell<bool>,
    #[no_trace]
    callback: LazyCallback<WebSocketDomAction>,
    opened: TracedPromise,
    closed: TracedPromise,
    readable: MutNullableDom<ReadableStream>,
    writable: MutNullableDom<WritableStream>,
    /// Write promises handed to the network loop that have not yet been
    /// acknowledged by a `SendComplete` event. Non-empty at close time means
    /// the close is unclean (data was still buffered in the user agent).
    #[ignore_malloc_size_of = "promises awaiting network acknowledgement"]
    pending_write_acks: DomRefCell<VecDeque<TracedPromise>>,
    /// Promise returned by the writable sink's close algorithm. It resolves
    /// when the peer's Close frame completes the closing handshake.
    #[ignore_malloc_size_of = "close-handshake promise"]
    pending_close_ack: DomRefCell<Option<TracedPromise>>,
    /// The `AbortSignal` passed to the constructor, kept alive so the abort
    /// algorithm can be delivered while connecting.
    signal: MutNullableDom<AbortSignal>,
    /// A read credit granted by the readable half's pull algorithm before the
    /// connection opened. `LazyCallback::send` blocks until the network side
    /// has taken the receiver (which happens after `send_fetch`), so credits
    /// are held here and flushed once the connection is established.
    read_credit_pending: Cell<bool>,
}

/// Create a promise rejected with the given DOM error.
fn rejected_with_error(cx: &mut JSContext, global: &GlobalScope, error: Error) -> RootedPromise {
    let promise = Promise::new(cx, global);
    promise.reject_error(cx, error);
    promise
}

impl WebSocketStream {
    fn new_inherited(
        url: ServoUrl,
        callback: LazyCallback<WebSocketDomAction>,
        opened: TracedPromise,
        closed: TracedPromise,
    ) -> WebSocketStream {
        WebSocketStream {
            reflector_: Reflector::new(),
            url,
            state: Cell::new(WebSocketStreamState::Connecting),
            opened_settled: Cell::new(false),
            callback,
            opened,
            closed,
            readable: MutNullableDom::new(None),
            writable: MutNullableDom::new(None),
            pending_write_acks: DomRefCell::new(VecDeque::new()),
            pending_close_ack: DomRefCell::new(None),
            signal: MutNullableDom::new(None),
            read_credit_pending: Cell::new(false),
        }
    }

    fn protocol_validation(protocols: &[DOMString]) -> Fallible<Vec<String>> {
        // https://websockets.spec.whatwg.org/#concept-websocket-establish
        // If any of the values in protocols occur more than once or otherwise
        // fail to match the requirements for elements that comprise the value
        // of `Sec-WebSocket-Protocol` fields as defined by The WebSocket
        // protocol, then throw a "SyntaxError" DOMException.
        for (i, protocol) in protocols.iter().enumerate() {
            let protocol = protocol.str();
            if protocols[i + 1..]
                .iter()
                .any(|p| p.str().eq_ignore_ascii_case(&protocol))
            {
                return Err(Error::Syntax(Some("Duplicate protocol header".into())));
            }
            if !is_token(protocol.as_bytes()) {
                return Err(Error::Syntax(Some(
                    "Protocol header field is not a valid token".into(),
                )));
            }
        }
        Ok(protocols.iter().map(|p| p.str().to_owned()).collect())
    }

    /// Reject `opened`/`closed` (and error both streams) with a WebSocketError
    /// carrying `close_code`. Used for handshake failures and for `close()`
    /// calls made before the connection is established.
    fn fail_with_websocket_error(
        &self,
        cx: &mut JSContext,
        global: &GlobalScope,
        close_code: u16,
        reason: DOMString,
    ) {
        if self.state.get() == WebSocketStreamState::Closed {
            return;
        }
        self.state.set(WebSocketStreamState::Closed);

        rooted!(&in(cx) let mut error_value = UndefinedValue());
        let error = WebSocketError::new(cx, global, DOMString::from(""), Some(close_code), reason);
        error.to_jsval(cx, error_value.handle_mut());

        if !self.opened_settled.get() {
            self.opened_settled.set(true);
            self.opened.root(cx).reject(cx, error_value.handle());
        }
        self.closed.root(cx).reject(cx, error_value.handle());

        if let Some(readable) = self.readable.get()
            && readable.is_readable()
        {
            readable.error(cx, error_value.handle());
        }
        if let Some(writable) = self.writable.get()
            && writable.is_writable()
        {
            writable.start_erroring(cx, global, error_value.handle());
        }

        // Any write that was in flight can never complete.
        rooted!(&in(cx) let mut invalid_state = UndefinedValue());
        Error::InvalidState(None).to_jsval(cx, global, invalid_state.handle_mut());
        for write_ack in self.pending_write_acks.borrow_mut().drain(..) {
            write_ack.root(cx).reject(cx, invalid_state.handle());
        }
        if let Some(close_ack) = self.pending_close_ack.borrow_mut().take() {
            close_ack.root(cx).reject(cx, error_value.handle());
        }
    }

    /// Reject `opened`/`closed` with an `AbortError` DOMException. Used when
    /// the constructor's `AbortSignal` fires before the connection opens.
    fn fail_with_abort_error(&self, cx: &mut JSContext, global: &GlobalScope) {
        if self.state.get() == WebSocketStreamState::Closed {
            return;
        }
        self.state.set(WebSocketStreamState::Closed);

        rooted!(&in(cx) let mut error_value = UndefinedValue());
        Error::Abort(None).to_jsval(cx, global, error_value.handle_mut());

        if !self.opened_settled.get() {
            self.opened_settled.set(true);
            self.opened.root(cx).reject(cx, error_value.handle());
        }
        self.closed.root(cx).reject(cx, error_value.handle());

        if let Some(readable) = self.readable.get()
            && readable.is_readable()
        {
            readable.error(cx, error_value.handle());
        }
        if let Some(writable) = self.writable.get()
            && writable.is_writable()
        {
            writable.start_erroring(cx, global, error_value.handle());
        }
        for write_ack in self.pending_write_acks.borrow_mut().drain(..) {
            write_ack.root(cx).reject(cx, error_value.handle());
        }
        if let Some(close_ack) = self.pending_close_ack.borrow_mut().take() {
            close_ack.root(cx).reject(cx, error_value.handle());
        }
    }

    /// Handle the constructor's AbortSignal firing. Aborting only has an
    /// effect while the handshake is still in progress.
    pub(crate) fn handle_abort_signal(&self, cx: &mut JSContext, global: &GlobalScope) {
        if self.state.get() != WebSocketStreamState::Connecting {
            return;
        }
        self.fail_with_abort_error(cx, global);
        // Tear the transport down. If the WebSocket handshake has not
        // completed yet this just marks the connection as closed by us, and
        // the network loop never starts.
        let _ = self.callback.send(WebSocketDomAction::Close(None, None));
    }

    /// Read the `closeCode`/`reason` attributes out of a `WebSocketError`
    /// passed as an abort/cancel reason. Any other value (including a plain
    /// `DOMException` with manually-added properties) is ignored.
    #[allow(unsafe_code)]
    fn close_info_from_reason(
        cx: &mut JSContext,
        reason: SafeHandleValue,
    ) -> Option<(Option<u16>, Option<String>)> {
        if !reason.get().is_object() {
            return None;
        }
        let object = reason.get().to_object();
        // Brand check: only actual `WebSocketError` instances carry close
        // information; duck-typed properties are ignored per the WPT contract.
        let error = unsafe { native_from_object::<WebSocketError>(cx, object) }.ok()?;
        let error = unsafe { &*error };
        Some((error.close_code(), Some(error.reason().str().to_owned())))
    }

    /// Enqueue a text message chunk on the readable stream.
    fn enqueue_text(&self, cx: &mut JSContext, text: String) {
        let Some(readable) = self.readable.get() else {
            return;
        };
        rooted!(&in(cx) let mut chunk = UndefinedValue());
        DOMString::from(text).to_jsval(cx, chunk.handle_mut());
        let controller = readable.get_default_controller();
        let _ = controller.enqueue(cx, chunk.handle());
    }

    /// Enqueue a binary message chunk on the readable stream.
    fn enqueue_binary(&self, cx: &mut JSContext, bytes: Vec<u8>) {
        if let Some(readable) = self.readable.get() {
            readable.enqueue_native(cx, bytes);
        }
    }

    /// The writable sink's write algorithm.
    ///
    /// Returns a promise that resolves once the network loop has written the
    /// message to the socket (`SendComplete`), mirroring the backpressure
    /// behaviour of the explainer.
    pub(crate) fn perform_write(
        &self,
        cx: &mut JSContext,
        global: &GlobalScope,
        chunk: SafeHandleValue,
    ) -> RootedPromise {
        let message = match chunk_to_message(cx, chunk) {
            Ok(message) => message,
            Err(error) => return rejected_with_error(cx, global, error),
        };
        if self.state.get() != WebSocketStreamState::Open {
            return rejected_with_error(
                cx,
                global,
                Error::InvalidState(Some("WebSocketStream is not open".into())),
            );
        }

        let promise = Promise::new(cx, global);
        self.pending_write_acks
            .borrow_mut()
            .push_back(promise.to_traced());
        if self
            .callback
            .send(WebSocketDomAction::SendMessage(message))
            .is_err()
        {
            // The transport is gone; reject the write we just queued.
            self.pending_write_acks
                .borrow_mut()
                .pop_back()
                .expect("write was just queued")
                .root(cx)
                .reject_error(
                    cx,
                    Error::InvalidState(Some("WebSocketStream is closed".into())),
                );
        }
        promise
    }

    /// The writable sink's close algorithm: start the closing handshake.
    pub(crate) fn perform_close(&self, cx: &mut JSContext, global: &GlobalScope) -> RootedPromise {
        if self.state.get() == WebSocketStreamState::Closed {
            return rejected_with_error(
                cx,
                global,
                Error::InvalidState(Some("WebSocketStream is closed".into())),
            );
        }
        let promise = Promise::new(cx, global);
        if self.state.get() == WebSocketStreamState::Open {
            self.state.set(WebSocketStreamState::Closing);
            *self.pending_close_ack.borrow_mut() = Some(promise.to_traced());
            let _ = self.callback.send(WebSocketDomAction::Close(None, None));
        } else {
            // Closing while still connecting: the connection never opens, so
            // the close "handshake" is vacuously complete.
            promise.resolve_native(cx, &());
        }
        promise
    }

    /// The writable sink's abort algorithm and the readable stream's cancel
    /// algorithm: interpret a `WebSocketError` reason (if any) and send a
    /// Close frame.
    pub(crate) fn perform_abort_or_cancel(
        &self,
        cx: &mut JSContext,
        reason: SafeHandleValue,
    ) -> RootedPromise {
        if self.state.get() == WebSocketStreamState::Closed {
            return Promise::new(cx, &self.global());
        }
        let (code, reason_text) = match Self::close_info_from_reason(cx, reason) {
            Some((code, reason_text)) => (code, reason_text),
            None => (None, None),
        };
        if self.state.get() == WebSocketStreamState::Open {
            self.state.set(WebSocketStreamState::Closing);
        }
        // Per the explainer, aborting with the default reason sends an empty
        // Close frame; an empty reason with `.close()` semantics would also
        // map to 1005, which is what `WebSocketDomAction::Close(None, _)`
        // produces on the wire.
        let _ = self
            .callback
            .send(WebSocketDomAction::Close(code, reason_text));
        Promise::new_resolved(cx, &self.global(), ())
    }

    /// The readable stream's pull algorithm: allow the network loop to read
    /// the next message. This is what applies backpressure to received data.
    pub(crate) fn request_read(&self) {
        if self.state.get() == WebSocketStreamState::Connecting {
            // The network side has not taken the action receiver yet (that
            // happens after `send_fetch` runs); sending now would block the
            // script thread.
            self.read_credit_pending.set(true);
            return;
        }
        if self.state.get() == WebSocketStreamState::Open {
            let _ = self.callback.send(WebSocketDomAction::RequestRead);
        }
    }
}

impl WebSocketStreamMethods<crate::DomTypeHolder> for WebSocketStream {
    /// <https://github.com/ricea/websocketstream-explainer#websocketstream-constructor>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<js::rust::HandleObject>,
        url: USVString,
        options: &WebSocketStreamOptions,
    ) -> Fallible<DomRoot<WebSocketStream>> {
        // Step 1. Let baseURL be this's relevant settings object's API base URL.
        let base_url = global.api_base_url();
        // Step 2-3. Parse the URL; a parse failure throws a SyntaxError.
        let mut url_record = ServoUrl::parse_with_base(Some(&base_url), &url)
            .or(Err(Error::Syntax(Some("Failed to parse url".into()))))?;

        // Step 4-6. Map http→ws and https→wss; anything else is forbidden.
        match url_record.scheme() {
            "http" => {
                url_record
                    .as_mut_url()
                    .set_scheme("ws")
                    .expect("Can't set scheme from http to ws");
            },
            "https" => {
                url_record
                    .as_mut_url()
                    .set_scheme("wss")
                    .expect("Can't set scheme from https to wss");
            },
            "ws" | "wss" => {},
            _ => {
                return Err(Error::Syntax(Some("Forbidden URL scheme".into())));
            },
        }

        // Step 7. A URL fragment is forbidden.
        if url_record.fragment().is_some() {
            return Err(Error::Syntax(Some("Forbidden URL fragment".into())));
        }

        // Step 8-9. Validate the protocols (sequence<DOMString>, duplicates and
        // non-token values are SyntaxErrors).
        let protocols = Self::protocol_validation(&options.protocols)?;

        // Create the interface for communication with the resource thread.
        let (dom_action_sender, resource_action_receiver) = lazy_callback();

        let opened_promise = Promise::new(cx, global);
        let closed_promise = Promise::new(cx, global);
        let opened = opened_promise.to_traced();
        let closed = closed_promise.to_traced();

        let stream = reflect_dom_object_with_proto(
            cx,
            Box::new(WebSocketStream::new_inherited(
                url_record.clone(),
                dom_action_sender,
                opened,
                closed,
            )),
            global,
            proto,
        );

        // Create the two streams. Both are backed by native underlying
        // source/sink variants that route through this DOM object.
        let readable = create_readable_stream(
            cx,
            global,
            UnderlyingSourceType::WebSocket(&stream),
            None,
            None,
        );
        stream.readable.set(Some(&readable));
        // Compute the strategy argument first: both calls need `&mut cx`, and
        // a nested call inside the argument list would double-borrow it.
        let size_algorithm = extract_size_algorithm(cx, &Default::default());
        let writable = create_writable_stream(
            cx,
            global,
            1.0,
            size_algorithm,
            UnderlyingSinkType::WebSocket(Dom::from_ref(&*stream)),
        )?;
        stream.writable.set(Some(&writable));

        // If an AbortSignal was passed and is already aborted, reject the
        // promises without ever starting the connection.
        if let Some(signal) = options.signal.as_ref() {
            stream.signal.set(Some(&**signal));
            if signal.aborted() {
                stream.fail_with_abort_error(cx, global);
                return Ok(stream);
            }
            signal.add(&AbortAlgorithm::WebSocketStream(Trusted::new(&*stream)));
        }

        // Establish the WebSocket connection through the existing pipeline.
        let address = Trusted::new(&*stream);
        let task_source: SendableTaskSource =
            global.task_manager().websocket_task_source().to_sendable();
        let resource_event_sender =
            ProfileGenericCallback::new(move |message| match message.unwrap() {
                WebSocketNetworkEvent::ConnectionEstablished { protocol_in_use } => {
                    task_source.queue(ConnectionEstablishedTask {
                        address: address.clone(),
                        protocol_in_use,
                    });
                },
                WebSocketNetworkEvent::MessageReceived(message) => {
                    task_source.queue(MessageReceivedTask {
                        address: address.clone(),
                        message,
                    });
                },
                WebSocketNetworkEvent::SendComplete => {
                    task_source.queue(SendCompleteTask {
                        address: address.clone(),
                    });
                },
                WebSocketNetworkEvent::Close(code, reason) => {
                    task_source.queue(CloseTask {
                        address: address.clone(),
                        code,
                        reason,
                    });
                },
                WebSocketNetworkEvent::Fail => {
                    task_source.queue(FailTask {
                        address: address.clone(),
                    });
                },
                WebSocketNetworkEvent::ReportCSPViolations(violations) => {
                    task_source.queue(ReportCSPViolationTask {
                        address: address.clone(),
                        violations,
                    });
                },
            })
            .expect("Couldn't create WebSocketStream callback.");

        let request = RequestBuilder::new(
            global.webview_id(),
            UrlWithBlobClaim::from_url_without_having_claimed_blob(url_record.clone()),
            Referrer::NoReferrer,
        )
        .with_global_scope(global)
        .mode(RequestMode::WebSocket {
            protocols,
            original_url: url_record,
        })
        .service_workers_mode(ServiceWorkersMode::None)
        .credentials_mode(CredentialsMode::Include)
        .cache_mode(CacheMode::NoCache)
        .redirect_mode(RedirectMode::Error);

        let channels = FetchChannels::WebSocket {
            event_sender: resource_event_sender,
            action_receiver: resource_action_receiver,
            flow_control: true,
        };
        let _ = send_fetch(&global.core_resource_thread(), request, channels);

        Ok(stream)
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketstream-url>
    fn Url(&self) -> USVString {
        USVString::from(self.url.as_str().to_owned())
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketstream-opened>
    #[allow(unsafe_code)]
    fn Opened(&self) -> RootedPromise {
        let cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        self.opened.root(&cx)
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketstream-closed>
    #[allow(unsafe_code)]
    fn Closed(&self) -> RootedPromise {
        let cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        self.closed.root(&cx)
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketstream-close>
    fn Close(&self, close_info: &WebSocketCloseInfo) -> Fallible<()> {
        let close_code = close_info.closeCode;
        let reason = close_info.reason.clone();

        // If closeInfo["closeCode"] is present and is not a valid close code,
        // throw an "InvalidAccessError" DOMException.
        if let Some(code) = close_code
            && !super::websocketerror::is_valid_close_code(code)
        {
            return Err(Error::InvalidAccess(Some(
                "closeCode is not a valid close code".into(),
            )));
        }
        // If closeInfo["reason"] is present and is longer than 123 bytes,
        // throw a "SyntaxError" DOMException.
        if reason.as_bytes().len() > super::websocketerror::MAX_REASON_BYTES {
            return Err(Error::Syntax(Some(
                "reason is longer than 123 bytes".into(),
            )));
        }

        let global = self.global();
        match self.state.get() {
            WebSocketStreamState::Connecting => {
                // A close before the connection opens rejects both promises
                // with a WebSocketError (abnormal closure).
                #[allow(unsafe_code)]
                let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
                self.fail_with_websocket_error(
                    &mut cx,
                    &global,
                    close_code::ABNORMAL,
                    DOMString::from(""),
                );
                let _ = self.callback.send(WebSocketDomAction::Close(None, None));
            },
            WebSocketStreamState::Open => {
                self.state.set(WebSocketStreamState::Closing);
                // An unspecified close code with a non-empty reason implies
                // 1000; otherwise an empty Close frame (1005 on the wire) is
                // sent.
                let code =
                    close_code.or_else(|| (!reason.is_empty()).then_some(close_code::NORMAL));
                let reason_text = (!reason.is_empty()).then(|| reason.0.clone());
                let _ = self
                    .callback
                    .send(WebSocketDomAction::Close(code, reason_text));
            },
            // Already closing or closed: nothing to do.
            WebSocketStreamState::Closing | WebSocketStreamState::Closed => {},
        }
        Ok(())
    }
}

/// Convert a chunk passed to `writer.write()` into a WebSocket message.
///
/// Strings are sent as text frames. Buffer sources are sent as binary frames,
/// except for resizable and shared array buffers (and views over them), which
/// are rejected with a TypeError. Any other value is a TypeError.
#[allow(unsafe_code)]
fn chunk_to_message(cx: &mut JSContext, chunk: SafeHandleValue) -> Result<MessageData, Error> {
    if chunk.get().is_string() {
        let js_string =
            ptr::NonNull::new(chunk.get().to_string()).expect("string value must have a JSString");
        let text = unsafe { jsstr_to_string(cx, js_string) };
        return Ok(MessageData::Text(text));
    }

    if !chunk.get().is_object() {
        return Err(Error::Type(
            c"chunk must be a string or a BufferSource".into(),
        ));
    }

    let object: *mut JSObject = chunk.get().to_object();
    unsafe {
        // Resizable ArrayBuffers and views over them are not supported: the
        // data could move or change size while it is being sent.
        if js::jsapi::IsResizableArrayBufferMaybeShared(object)
            || js::jsapi::IsResizableArrayBufferView(object)
        {
            return Err(Error::Type(
                c"resizable ArrayBuffers are not supported".into(),
            ));
        }
        // Shared memory (SharedArrayBuffer, or a view over one) is rejected for
        // the same reason: another agent could mutate it during the send.
        if js::jsapi::IsSharedArrayBufferObject(object) {
            return Err(Error::Type(c"shared ArrayBuffers are not supported".into()));
        }
        if js::jsapi::JS_IsArrayBufferViewObject(object) {
            let mut is_shared = false;
            // The raw object needs a rooted handle for the wrapper (the
            // `rust::Handle::from_raw` form expects a C++-side raw handle,
            // which a bare `*mut JSObject` is not).
            rooted!(&in(cx) let rooted_object = object);
            js::rust::wrappers2::JS_GetArrayBufferViewBuffer(
                cx,
                rooted_object.handle(),
                &mut is_shared,
            );
            if is_shared {
                return Err(Error::Type(
                    c"views over shared ArrayBuffers are not supported".into(),
                ));
            }
        }
    }

    let bytes = if unsafe { js::jsapi::IsArrayBufferObject(object) } {
        match js::typedarray::TypedArray::<js::typedarray::ArrayBufferU8, *mut JSObject>::from(
            object,
        ) {
            Ok(buffer) => buffer.to_vec(),
            Err(_) => {
                return Err(Error::Type(c"chunk must be a BufferSource".into()));
            },
        }
    } else if unsafe { js::jsapi::JS_IsArrayBufferViewObject(object) } {
        match js::typedarray::TypedArray::<js::typedarray::ArrayBufferViewU8, *mut JSObject>::from(
            object,
        ) {
            Ok(view) => view.to_vec(),
            Err(_) => {
                return Err(Error::Type(c"chunk must be a BufferSource".into()));
            },
        }
    } else {
        return Err(Error::Type(
            c"chunk must be a string or a BufferSource".into(),
        ));
    };

    Ok(MessageData::Binary(bytes.unwrap_or_default()))
}

struct ConnectionEstablishedTask {
    address: Trusted<WebSocketStream>,
    protocol_in_use: Option<String>,
}

impl TaskOnce for ConnectionEstablishedTask {
    #[allow(unsafe_code)]
    fn run_once(self, cx: &mut JSContext) {
        let stream = self.address.root();
        if stream.state.get() != WebSocketStreamState::Connecting {
            return;
        }
        stream.state.set(WebSocketStreamState::Open);
        stream.opened_settled.set(true);

        // Build `{readable, writable, protocol, extensions}`. The object is
        // rooted as a `*mut JSObject` because `JS_DefineProperty` takes an
        // object handle; a rooted `Value` handle does not satisfy it.
        rooted!(&in(cx) let object = unsafe { JS_NewObject(cx, ptr::null()) });
        rooted!(&in(cx) let mut readable_value = UndefinedValue());
        if let Some(readable) = stream.readable.get() {
            readable.to_jsval(cx, readable_value.handle_mut());
        }
        rooted!(&in(cx) let mut writable_value = UndefinedValue());
        if let Some(writable) = stream.writable.get() {
            writable.to_jsval(cx, writable_value.handle_mut());
        }
        rooted!(&in(cx) let mut protocol_value = UndefinedValue());
        DOMString::from(self.protocol_in_use.unwrap_or_default())
            .to_jsval(cx, protocol_value.handle_mut());
        rooted!(&in(cx) let mut extensions_value = UndefinedValue());
        DOMString::from("").to_jsval(cx, extensions_value.handle_mut());

        unsafe {
            JS_DefineProperty(
                cx,
                object.handle(),
                c"readable".as_ptr(),
                readable_value.handle(),
                JSPROP_ENUMERATE as _,
            );
            JS_DefineProperty(
                cx,
                object.handle(),
                c"writable".as_ptr(),
                writable_value.handle(),
                JSPROP_ENUMERATE as _,
            );
            JS_DefineProperty(
                cx,
                object.handle(),
                c"protocol".as_ptr(),
                protocol_value.handle(),
                JSPROP_ENUMERATE as _,
            );
            JS_DefineProperty(
                cx,
                object.handle(),
                c"extensions".as_ptr(),
                extensions_value.handle(),
                JSPROP_ENUMERATE as _,
            );
        }

        rooted!(&in(cx) let object_ref = ObjectValue(object.get()));
        stream.opened.root(cx).resolve(cx, object_ref.handle());

        // Deliver any read credit that was granted while connecting (the
        // initial pull of the readable stream).
        if stream.read_credit_pending.take() {
            let _ = stream.callback.send(WebSocketDomAction::RequestRead);
        }
    }
}

struct MessageReceivedTask {
    address: Trusted<WebSocketStream>,
    message: MessageData,
}

impl TaskOnce for MessageReceivedTask {
    fn run_once(self, cx: &mut JSContext) {
        let stream = self.address.root();
        if stream.state.get() != WebSocketStreamState::Open {
            return;
        }
        match self.message {
            MessageData::Text(text) => stream.enqueue_text(cx, text),
            MessageData::Binary(bytes) => stream.enqueue_binary(cx, bytes),
        }
    }
}

struct SendCompleteTask {
    address: Trusted<WebSocketStream>,
}

impl TaskOnce for SendCompleteTask {
    fn run_once(self, cx: &mut JSContext) {
        let stream = self.address.root();
        let write_ack = stream.pending_write_acks.borrow_mut().pop_front();
        if let Some(write_ack) = write_ack {
            write_ack.root(cx).resolve_native(cx, &());
        }
    }
}

struct CloseTask {
    address: Trusted<WebSocketStream>,
    code: Option<u16>,
    reason: String,
}

impl TaskOnce for CloseTask {
    #[allow(unsafe_code)]
    fn run_once(self, cx: &mut JSContext) {
        let stream = self.address.root();
        if stream.state.get() == WebSocketStreamState::Closed {
            return;
        }
        let global = stream.global();
        let was_connecting = stream.state.get() == WebSocketStreamState::Connecting;
        let close_code = self.code.unwrap_or(close_code::NO_STATUS);
        let pending_writes = !stream.pending_write_acks.borrow().is_empty();

        // A close is unclean if writes were still buffered when the close
        // frame arrived; it is also unclean if the connection closed before
        // the handshake completed. `fail_with_websocket_error` settles both
        // promises, errors the streams and rejects any pending writes.
        if pending_writes || was_connecting {
            stream.fail_with_websocket_error(
                cx,
                &global,
                close_code,
                DOMString::from(self.reason.clone()),
            );
            return;
        }
        stream.state.set(WebSocketStreamState::Closed);

        // The close handshake completed: resolve the writable sink's close
        // algorithm promise (if any).
        if let Some(close_ack) = stream.pending_close_ack.borrow_mut().take() {
            close_ack.root(cx).resolve_native(cx, &());
        }

        // Clean close: `closed` resolves with the values from the received
        // Close frame (WebSocket without a status code reports 1005). The
        // object is rooted as a `*mut JSObject` (see ConnectionEstablishedTask).
        rooted!(&in(cx) let object = unsafe { JS_NewObject(cx, ptr::null()) });
        rooted!(&in(cx) let mut code_value = UndefinedValue());
        close_code.to_jsval(cx, code_value.handle_mut());
        rooted!(&in(cx) let mut reason_value = UndefinedValue());
        DOMString::from(self.reason).to_jsval(cx, reason_value.handle_mut());
        unsafe {
            JS_DefineProperty(
                cx,
                object.handle(),
                c"closeCode".as_ptr(),
                code_value.handle(),
                JSPROP_ENUMERATE as _,
            );
            JS_DefineProperty(
                cx,
                object.handle(),
                c"reason".as_ptr(),
                reason_value.handle(),
                JSPROP_ENUMERATE as _,
            );
        }

        // `opened` has necessarily settled by now: `ConnectionEstablished`
        // is queued before any `Close` event, and a close that arrives while
        // still connecting is handled by the unclean branch above.
        rooted!(&in(cx) let object_ref = ObjectValue(object.get()));
        stream.closed.root(cx).resolve(cx, object_ref.handle());

        // Reads are finished; writes can no longer be made.
        if let Some(readable) = stream.readable.get()
            && readable.is_readable()
        {
            readable.controller_close_native(cx);
        }
        if let Some(writable) = stream.writable.get()
            && writable.is_writable()
        {
            rooted!(&in(cx) let mut invalid_state = UndefinedValue());
            Error::InvalidState(Some("WebSocket connection closed".into())).to_jsval(
                cx,
                &global,
                invalid_state.handle_mut(),
            );
            writable.start_erroring(cx, &global, invalid_state.handle());
        }
    }
}

struct FailTask {
    address: Trusted<WebSocketStream>,
}

impl TaskOnce for FailTask {
    fn run_once(self, cx: &mut JSContext) {
        let stream = self.address.root();
        if stream.state.get() == WebSocketStreamState::Closed {
            return;
        }
        let global = stream.global();
        let pending_writes = !stream.pending_write_acks.borrow().is_empty();
        stream.state.set(WebSocketStreamState::Closed);

        rooted!(&in(cx) let mut error_value = UndefinedValue());
        let error = WebSocketError::new(
            cx,
            &global,
            DOMString::from(""),
            Some(close_code::ABNORMAL),
            DOMString::from(""),
        );
        error.to_jsval(cx, error_value.handle_mut());

        if !stream.opened_settled.get() {
            stream.opened_settled.set(true);
            stream.opened.root(cx).reject(cx, error_value.handle());
        }
        stream.closed.root(cx).reject(cx, error_value.handle());

        if let Some(readable) = stream.readable.get()
            && readable.is_readable()
        {
            readable.error(cx, error_value.handle());
        }
        if let Some(writable) = stream.writable.get() {
            if pending_writes {
                // With buffered writes the stream's stored error is the
                // InvalidStateError the pending write promises reject with.
                rooted!(&in(cx) let mut invalid_state = UndefinedValue());
                Error::InvalidState(Some("WebSocket connection closed".into())).to_jsval(
                    cx,
                    &global,
                    invalid_state.handle_mut(),
                );
                if writable.is_writable() {
                    writable.start_erroring(cx, &global, invalid_state.handle());
                }
            } else if writable.is_writable() {
                writable.start_erroring(cx, &global, error_value.handle());
            }
        }

        rooted!(&in(cx) let mut invalid_state = UndefinedValue());
        Error::InvalidState(Some("WebSocket connection closed".into())).to_jsval(
            cx,
            &global,
            invalid_state.handle_mut(),
        );
        for write_ack in stream.pending_write_acks.borrow_mut().drain(..) {
            write_ack.root(cx).reject(cx, invalid_state.handle());
        }
        if let Some(close_ack) = stream.pending_close_ack.borrow_mut().take() {
            close_ack.root(cx).reject(cx, error_value.handle());
        }
    }
}

struct ReportCSPViolationTask {
    address: Trusted<WebSocketStream>,
    violations: Vec<Violation>,
}

impl TaskOnce for ReportCSPViolationTask {
    fn run_once(self, cx: &mut JSContext) {
        let global = self.address.root().global();
        global.report_csp_violations(cx, self.violations, None, None);
    }
}
