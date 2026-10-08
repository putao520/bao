/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `CharacterBoundsUpdateEvent` from the W3C EditContext API (REQ-BRW-050,
//! Bao fork self-build — see `dom::editcontext`).

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::EditContextBinding::CharacterBoundsUpdateEventInit;
use crate::dom::bindings::codegen::Bindings::EditContextBinding::CharacterBoundsUpdateEventMethods;
use crate::dom::bindings::codegen::Bindings::EventBinding::EventMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::event::{Event, EventBubbles, EventCancelable};
use crate::dom::types::Window;

/// <https://w3c.github.io/edit-context/#characterboundsupdateevent>
#[dom_struct]
pub(crate) struct CharacterBoundsUpdateEvent {
    event: Event,
    range_start: u32,
    range_end: u32,
}

impl CharacterBoundsUpdateEvent {
    fn new_inherited(range_start: u32, range_end: u32) -> CharacterBoundsUpdateEvent {
        CharacterBoundsUpdateEvent {
            event: Event::new_inherited(),
            range_start,
            range_end,
        }
    }

    fn new(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        range_start: u32,
        range_end: u32,
    ) -> DomRoot<CharacterBoundsUpdateEvent> {
        let event = Box::new(CharacterBoundsUpdateEvent::new_inherited(
            range_start, range_end,
        ));
        let event = reflect_dom_object_with_proto(cx, event, window, proto);
        {
            let event = event.upcast::<Event>();
            event.init_event(type_, bool::from(bubbles), bool::from(cancelable));
        }
        event
    }
}

impl CharacterBoundsUpdateEventMethods<crate::DomTypeHolder> for CharacterBoundsUpdateEvent {
    /// <https://w3c.github.io/edit-context/#dom-characterboundsupdateevent>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &CharacterBoundsUpdateEventInit,
    ) -> DomRoot<CharacterBoundsUpdateEvent> {
        let bubbles = EventBubbles::from(init.parent.bubbles);
        let cancelable = EventCancelable::from(init.parent.cancelable);
        CharacterBoundsUpdateEvent::new(
            cx,
            window,
            proto,
            Atom::from(type_),
            bubbles,
            cancelable,
            init.rangeStart,
            init.rangeEnd,
        )
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }

    /// <https://w3c.github.io/edit-context/#dom-characterboundsupdateevent-rangestart>
    fn RangeStart(&self) -> u32 {
        self.range_start
    }

    /// <https://w3c.github.io/edit-context/#dom-characterboundsupdateevent-rangeend>
    fn RangeEnd(&self) -> u32 {
        self.range_end
    }
}
