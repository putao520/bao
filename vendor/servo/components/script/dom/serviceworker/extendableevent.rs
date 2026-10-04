/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::IsPromiseObject;
use js::jsapi::HandleObject as RawHandleObject;
use js::rust::{HandleObject, HandleValue};
use script_bindings::cell::DomRefCell;
use script_bindings::interfaces::StackRootPromiseHelpers;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::EventBinding::EventMethods;
use crate::dom::bindings::codegen::Bindings::ExtendableEventBinding::{
    ExtendableEventInit, ExtendableEventMethods,
};
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::event::Event;
use crate::dom::promise::{Promise, TracedPromise};
use crate::dom::serviceworkerglobalscope::ServiceWorkerGlobalScope;

// https://w3c.github.io/ServiceWorker/#extendable-event
#[dom_struct]
pub(crate) struct ExtendableEvent {
    event: Event,
    extensions_allowed: bool,

    /// BAO PATCH (REQ-BRW-004 e58 contract B, user ruling 2026-10-04): the
    /// spec's extend-event-promises set, registered through `waitUntil`.
    /// The fork has no job-completion determination face yet, so consumption
    /// is a lazy sweep of already-settled entries on the next `waitUntil`.
    extended_promises: DomRefCell<Vec<TracedPromise>>,
}

impl ExtendableEvent {
    pub(crate) fn new_inherited() -> ExtendableEvent {
        ExtendableEvent {
            event: Event::new_inherited(),
            extensions_allowed: true,
            extended_promises: Default::default(),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        worker: &ServiceWorkerGlobalScope,
        type_: Atom,
        bubbles: bool,
        cancelable: bool,
    ) -> DomRoot<ExtendableEvent> {
        Self::new_with_proto(cx, worker, None, type_, bubbles, cancelable)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        worker: &ServiceWorkerGlobalScope,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: bool,
        cancelable: bool,
    ) -> DomRoot<ExtendableEvent> {
        let ev = reflect_dom_object_with_proto(
            cx,
            Box::new(ExtendableEvent::new_inherited()),
            worker,
            proto,
        );
        {
            let event = ev.upcast::<Event>();
            event.init_event(type_, bubbles, cancelable);
        }
        ev
    }
}

impl ExtendableEventMethods<crate::DomTypeHolder> for ExtendableEvent {
    /// <https://w3c.github.io/ServiceWorker/#dom-extendableevent-extendableevent>
    fn Constructor(
        cx: &mut JSContext,
        worker: &ServiceWorkerGlobalScope,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &ExtendableEventInit,
    ) -> Fallible<DomRoot<ExtendableEvent>> {
        Ok(ExtendableEvent::new_with_proto(
            cx,
            worker,
            proto,
            Atom::from(type_),
            init.parent.bubbles,
            init.parent.cancelable,
        ))
    }

    /// <https://w3c.github.io/ServiceWorker/#wait-until-method>
    ///
    /// BAO PATCH (REQ-BRW-004 e58 contract B, user ruling 2026-10-04):
    /// record the promise (the spec's extend-event-promises set). Non-promise
    /// values resolve immediately and need no lifetime extension. The
    /// fork's codegen passes no cx for HandleValue-taking members (the
    /// typeNeedsCx stub); take the script thread's active context
    /// (serviceworker/cache.rs precedent).
    #[allow(unsafe_code)]
    fn WaitUntil(&self, val: HandleValue) -> ErrorResult {
        // Step 1
        if !self.extensions_allowed {
            return Err(Error::InvalidState(None));
        }
        // Step 2: record the promise. Consumption is a lazy sweep of
        // already-settled entries — settled promises leave the set on the
        // next `waitUntil` (see the field doc).
        let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        if !val.get().is_object() {
            // Not an object: `Promise.resolve` settles it immediately —
            // nothing to extend.
            return Ok(());
        }
        let obj_slot = val.get().to_object();
        // SAFETY: `IsPromiseObject` is a pure read over the promise object
        // (same marked-location read the promise state predicates use).
        let is_promise =
            unsafe { IsPromiseObject(RawHandleObject::from_marked_location(&obj_slot)) };
        if !is_promise {
            return Ok(());
        }
        let promise = Promise::new_with_js_promise(
            &cx,
            // SAFETY: the slot outlives this call; the promise re-roots the
            // object permanently (`AddRawValueRoot`) on construction.
            #[expect(unsafe_code)]
            unsafe {
                HandleObject::from_marked_location(&obj_slot)
            },
        );
        let mut extended = self.extended_promises.borrow_mut();
        extended.retain(|entry| !entry.root(&cx).is_fulfilled());
        extended.push(promise.to_traced());
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }
}
