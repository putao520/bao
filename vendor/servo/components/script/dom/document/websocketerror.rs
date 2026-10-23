/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use js::context::JSContext;
use js::gc::HandleObject;
use script_bindings::codegen::GenericBindings::WebSocketErrorBinding::{
    WebSocketErrorInit, WebSocketErrorMethods,
};
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};
use script_bindings::root::DomRoot;
use script_bindings::str::{DOMString, USVString};

use crate::dom::bindings::error::Error;
use crate::dom::types::{DOMException, GlobalScope};

/// <https://github.com/ricea/websocketstream-explainer#websocketerror>
///
/// `WebSocketError` is a `DOMException` with `closeCode` and `reason`
/// attributes. It is used to communicate abnormal-close information through
/// the `WebSocketStream` streams (writable abort / readable cancel /
/// the `closed` promise rejection).
#[dom_struct]
pub(crate) struct WebSocketError {
    /// <https://webidl.spec.whatwg.org/#idl-DOMException>
    dom_exception: DOMException,
    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketerror-closecode>
    close_code: Option<u16>,
    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketerror-reason>
    reason: DOMString,
}

/// <https://github.com/ricea/websocketstream-explainer#websocketerror-constructor>
///
/// Only 1000 and the application-private range 3000-4999 are valid close
/// codes, mirroring the `WebSocket.close()` validation.
pub(crate) fn is_valid_close_code(code: u16) -> bool {
    code == 1000 || (3000..=4999).contains(&code)
}

/// The Close frame reason may not exceed 123 UTF-8 bytes (RFC 6455 §5.5
/// reserves the remaining two bytes of the 125-byte control frame payload for
/// the status code).
pub(crate) const MAX_REASON_BYTES: usize = 123;

impl WebSocketError {
    fn new_inherited(
        message: DOMString,
        close_code: Option<u16>,
        reason: DOMString,
    ) -> WebSocketError {
        WebSocketError {
            dom_exception: DOMException::new_inherited(
                message,
                DOMString::from_static("WebSocketError"),
            ),
            close_code,
            reason,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        message: DOMString,
        close_code: Option<u16>,
        reason: DOMString,
    ) -> DomRoot<WebSocketError> {
        reflect_dom_object(
            cx,
            Box::new(WebSocketError::new_inherited(message, close_code, reason)),
            global,
        )
    }

    /// The close code carried by this error, if any.
    pub(crate) fn close_code(&self) -> Option<u16> {
        self.close_code
    }

    /// The close reason carried by this error (empty when there is none).
    pub(crate) fn reason(&self) -> DOMString {
        self.reason.clone()
    }
}

impl WebSocketErrorMethods<crate::DomTypeHolder> for WebSocketError {
    /// <https://github.com/ricea/websocketstream-explainer#websocketerror-constructor>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        message: DOMString,
        init: &WebSocketErrorInit,
    ) -> Result<DomRoot<Self>, Error> {
        let close_code = init.closeCode;
        let reason = init.reason.clone();

        // If options["closeCode"] is present and not a valid close code,
        // throw an "InvalidAccessError" DOMException.
        if let Some(code) = close_code
            && !is_valid_close_code(code)
        {
            return Err(Error::InvalidAccess(Some(
                "closeCode is not a valid close code".into(),
            )));
        }

        // If options["reason"] is present and its UTF-8 byte length is greater
        // than 123, throw a "SyntaxError" DOMException.
        if reason.as_bytes().len() > MAX_REASON_BYTES {
            return Err(Error::Syntax(Some(
                "reason is longer than 123 bytes".into(),
            )));
        }

        // If options["reason"] is the empty string and options["closeCode"] is
        // present, the close code is used as-is. Otherwise, a non-empty reason
        // implies a normal-closure (1000) close code.
        let close_code = close_code.or_else(|| (!reason.is_empty()).then_some(1000u16));

        Ok(reflect_dom_object_with_proto(
            cx,
            Box::new(WebSocketError::new_inherited(
                message,
                close_code,
                DOMString::from(reason),
            )),
            global,
            proto,
        ))
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketerror-closecode>
    fn GetCloseCode(&self) -> Option<u16> {
        self.close_code
    }

    /// <https://github.com/ricea/websocketstream-explainer#dom-websocketerror-reason>
    fn Reason(&self) -> USVString {
        USVString::from(self.reason.str().to_owned())
    }
}
