/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The `DeviceBoundSession` DOM interface — `navigator.deviceBoundSession`.
//!
//! BAO PATCH (REQ-BRW-051): the JS query face of the DBSC slice. The session
//! state lives in the net crate's `DbscStore` (one per cookie partition,
//! reached through `CoreResourceMsg`); this object only round-trips the
//! queries. Wave-A scope: `hasSession` (store query) and `clearSession`
//! (session termination) — both semantics are fixed by the draft's session
//! store. Exposure is entirely pref-gated (`dom_dbsc_enabled`, default OFF)
//! plus `[SecureContext]`.
//!
//! Wave-B `setSession` audit — the face is NOT implemented, and deliberately
//! so. The Wave-B dispatch premise ("pin the `SessionRegistrationConfig`
//! dictionary from the WPT fixtures") does not hold in this tree:
//!   1. No test under `tests/wpt/tests/device-bound-session-credentials/`
//!      (nor anywhere in the WPT test tree) references `setSession`,
//!      `SessionRegistrationConfig` or `navigator.deviceBoundSession`; the
//!      only whole-tree matches are Chromium feature-flag strings in
//!      `tests/tools/wptrunner/wptrunner/browsers/chrome.py`.
//!   2. The specification (W3C DBSC draft, <https://w3c.github.io/webappsec-dbsc/>)
//!      defines no JavaScript API at all — no `navigator.deviceBoundSession`,
//!      `hasSession`, `clearSession`, `setSession` or `SessionRegistrationConfig`;
//!      registration is driven solely by the `Secure-Session-Registration`
//!      response header (§8.10).
//! Shipping a guessed dictionary would invent a fingerprintable public API
//! surface with no authority, which the project rules forbid. `setSession`
//! therefore stays unexposed until an authoritative definition exists; the
//! pre-existing `hasSession`/`clearSession` extension stays behind the
//! default-OFF pref and is not reachable from any page configuration.

use dom_struct::dom_struct;
use js::context::JSContext;
use js::realm::CurrentRealm;
use net_traits::CoreResourceMsg;
use script_bindings::reflector::reflect_dom_object;
use servo_base::generic_channel::GenericCallback;
use servo_url::ServoUrl;

use crate::dom::RootedPromise;
use crate::dom::bindings::codegen::Bindings::DeviceBoundSessionBinding::DeviceBoundSessionMethods;
use crate::dom::bindings::error::Error;
use crate::dom::bindings::refcounted::TrustedPromise;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::USVString;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::Promise;
use crate::tasks::task_source::SendableTaskSource;

#[dom_struct]
pub(crate) struct DeviceBoundSession {
    eventtarget: EventTarget,
}

impl DeviceBoundSession {
    fn new_inherited() -> DeviceBoundSession {
        DeviceBoundSession {
            eventtarget: EventTarget::new_inherited(),
        }
    }

    pub(crate) fn new(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<DeviceBoundSession> {
        reflect_dom_object(cx, Box::new(DeviceBoundSession::new_inherited()), global)
    }

    fn type_error_from_string(message: String) -> Error {
        let message = std::ffi::CString::new(message)
            .unwrap_or_else(|_| c"DeviceBoundSession operation failed".to_owned());
        Error::Type(message)
    }

    /// Parse an absolute URL argument; anything the URL parser rejects is a
    /// `TypeError` (the draft's query faces take a URL, not a
    /// base-relative convenience form).
    fn parse_absolute_url(url: &USVString) -> Result<ServoUrl, Error> {
        ServoUrl::parse(&url.0)
            .map_err(|_| Self::type_error_from_string(format!("invalid URL: {}", url.0)))
    }
}

/// Promise settler for the resource-thread round trips: the callback fires
/// on the net thread (or the IPC router) and hops to the DOM-manipulation
/// task source to settle the promise (the StorageManager shape).
struct DeviceBoundSessionBooleanHandler {
    trusted_promise: Option<TrustedPromise>,
    task_source: SendableTaskSource,
}

impl DeviceBoundSessionBooleanHandler {
    fn new(trusted_promise: TrustedPromise, task_source: SendableTaskSource) -> Self {
        Self {
            trusted_promise: Some(trusted_promise),
            task_source,
        }
    }

    fn handle(&mut self, value: bool) {
        let Some(trusted_promise) = self.trusted_promise.take() else {
            return;
        };
        self.task_source
            .queue(task!(dbsc_has_session_response: move |cx| {
                let promise = trusted_promise.root(cx);
                promise.resolve_native(cx, &value);
            }));
    }
}

/// Promise settler for the termination face: resolves `undefined` on
/// success, rejects with a `TypeError` carrying the failure reason.
struct DeviceBoundSessionUnitHandler {
    trusted_promise: Option<TrustedPromise>,
    task_source: SendableTaskSource,
}

impl DeviceBoundSessionUnitHandler {
    fn new(trusted_promise: TrustedPromise, task_source: SendableTaskSource) -> Self {
        Self {
            trusted_promise: Some(trusted_promise),
            task_source,
        }
    }

    fn handle(&mut self, result: Result<(), String>) {
        let Some(trusted_promise) = self.trusted_promise.take() else {
            return;
        };
        self.task_source.queue(task!(dbsc_clear_session_response: move |cx| {
            let promise = trusted_promise.root(cx);
            match result {
                Ok(()) => promise.resolve_native(cx, &()),
                Err(message) => {
                    promise.reject_error(cx, DeviceBoundSession::type_error_from_string(message))
                },
            }
        }));
    }
}

/// The generated trait passes no `cx` to these methods; acquire the script
/// thread's context locally (same shape as `ImageDecoder::thread_cx`).
#[allow(unsafe_code)]
fn thread_cx() -> JSContext {
    unsafe { JSContext::get_from_thread() }.expect("no active JS context")
}

impl DeviceBoundSessionMethods<crate::DomTypeHolder> for DeviceBoundSession {
    /// Whether the UA holds at least one DBSC session for `url`'s site (in
    /// the calling page's cookie partition — the query rides the same
    /// partition-stamped channel every other page-attributed resource
    /// message uses).
    fn HasSession(&self, url: USVString) -> RootedPromise {
        let mut cx = thread_cx();
        let mut realm = CurrentRealm::assert(&mut cx);
        let cx = &mut realm;
        let promise = Promise::new_in_realm(cx);
        let global = self.global();

        let request_url = match Self::parse_absolute_url(&url) {
            Ok(url) => url,
            Err(error) => {
                promise.reject_error(cx, error);
                return promise;
            },
        };

        let mut handler = DeviceBoundSessionBooleanHandler::new(
            TrustedPromise::from(&promise),
            global
                .task_manager()
                .dom_manipulation_task_source()
                .to_sendable(),
        );
        let callback = GenericCallback::new(move |message| {
            handler.handle(message.unwrap_or(false));
        })
        .expect("Could not create DeviceBoundSession hasSession callback");

        if global
            .core_resource_thread()
            .send(CoreResourceMsg::DbscHasSession(
                request_url,
                callback.clone(),
            ))
            .is_err()
            && let Err(error) = callback.send(false)
        {
            log::error!("Failed to deliver DeviceBoundSession hasSession fallback: {error}");
        }

        promise
    }

    /// End the identified session of `url`'s site, or — without an
    /// identifier — every session of `url`'s origin.
    fn ClearSession(&self, url: USVString, session_identifier: Option<USVString>) -> RootedPromise {
        let mut cx = thread_cx();
        let mut realm = CurrentRealm::assert(&mut cx);
        let cx = &mut realm;
        let promise = Promise::new_in_realm(cx);
        let global = self.global();

        let request_url = match Self::parse_absolute_url(&url) {
            Ok(url) => url,
            Err(error) => {
                promise.reject_error(cx, error);
                return promise;
            },
        };

        let mut handler = DeviceBoundSessionUnitHandler::new(
            TrustedPromise::from(&promise),
            global
                .task_manager()
                .dom_manipulation_task_source()
                .to_sendable(),
        );
        let callback = GenericCallback::new(move |message| {
            handler.handle(message.unwrap_or_else(|_| {
                Err("DeviceBoundSession clearSession callback failed".to_owned())
            }));
        })
        .expect("Could not create DeviceBoundSession clearSession callback");

        if global
            .core_resource_thread()
            .send(CoreResourceMsg::DbscClearSession(
                request_url,
                session_identifier.map(|identifier| identifier.0),
                callback.clone(),
            ))
            .is_err()
            && let Err(error) =
                callback.send(Err("Failed to queue the clearSession task".to_owned()))
        {
            log::error!("Failed to deliver DeviceBoundSession clearSession fallback: {error}");
        }

        promise
    }
}
