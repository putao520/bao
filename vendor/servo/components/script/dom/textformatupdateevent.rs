/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `TextFormatUpdateEvent` from the W3C EditContext API (REQ-BRW-050, Bao
//! fork self-build — see `dom::editcontext`).

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::EventBinding::EventMethods;
use crate::dom::bindings::codegen::Bindings::TextFormatUpdateBinding::TextFormatUpdateEventInit;
use crate::dom::bindings::codegen::Bindings::TextFormatUpdateBinding::TextFormatUpdateEventMethods;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::event::{Event, EventBubbles, EventCancelable};
use crate::dom::textformat::TextFormat;
use crate::dom::types::Window;

/// <https://w3c.github.io/edit-context/#textformatupdateevent>
#[dom_struct]
pub(crate) struct TextFormatUpdateEvent {
    event: Event,
    text_formats: DomRefCell<Vec<Dom<TextFormat>>>,
}

impl TextFormatUpdateEvent {
    fn new_inherited(text_formats: Vec<Dom<TextFormat>>) -> TextFormatUpdateEvent {
        TextFormatUpdateEvent {
            event: Event::new_inherited(),
            text_formats: DomRefCell::new(text_formats),
        }
    }

    fn new(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        text_formats: Vec<Dom<TextFormat>>,
    ) -> DomRoot<TextFormatUpdateEvent> {
        let event = Box::new(TextFormatUpdateEvent::new_inherited(text_formats));
        let event = reflect_dom_object_with_proto(cx, event, window, proto);
        {
            let event = event.upcast::<Event>();
            event.init_event(type_, bool::from(bubbles), bool::from(cancelable));
        }
        event
    }
}

impl TextFormatUpdateEventMethods<crate::DomTypeHolder> for TextFormatUpdateEvent {
    /// <https://w3c.github.io/edit-context/#dom-textformatupdateevent>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &TextFormatUpdateEventInit,
    ) -> DomRoot<TextFormatUpdateEvent> {
        let bubbles = EventBubbles::from(init.parent.bubbles);
        let cancelable = EventCancelable::from(init.parent.cancelable);
        TextFormatUpdateEvent::new(
            cx,
            window,
            proto,
            Atom::from(type_),
            bubbles,
            cancelable,
            init.textFormats
                .iter()
                .map(|format| Dom::from_ref(&**format))
                .collect(),
        )
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }

    /// <https://w3c.github.io/edit-context/#dom-textformatupdateevent-gettextformats>
    fn GetTextFormats(&self, _cx: &mut JSContext) -> Vec<DomRoot<TextFormat>> {
        self.text_formats
            .borrow()
            .iter()
            .map(|format| DomRoot::from_ref(&**format))
            .collect()
    }
}
