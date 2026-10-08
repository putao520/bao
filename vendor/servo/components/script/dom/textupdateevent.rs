/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `TextUpdateEvent` from the W3C EditContext API (REQ-BRW-050, Bao fork
//! self-build — see `dom::editcontext`).

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::EditContextBinding::TextUpdateEventInit;
use crate::dom::bindings::codegen::Bindings::EditContextBinding::TextUpdateEventMethods;
use crate::dom::bindings::codegen::Bindings::EventBinding::EventMethods;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::event::{Event, EventBubbles, EventCancelable};
use crate::dom::types::Window;

/// <https://w3c.github.io/edit-context/#textupdateevent>
#[dom_struct]
pub(crate) struct TextUpdateEvent {
    event: Event,
    update_range_start: u32,
    update_range_end: u32,
    text: DOMString,
    selection_start: u32,
    selection_end: u32,
}

impl TextUpdateEvent {
    #[expect(clippy::too_many_arguments)]
    fn new_inherited(
        update_range_start: u32,
        update_range_end: u32,
        text: DOMString,
        selection_start: u32,
        selection_end: u32,
    ) -> TextUpdateEvent {
        TextUpdateEvent {
            event: Event::new_inherited(),
            update_range_start,
            update_range_end,
            text,
            selection_start,
            selection_end,
        }
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        cx: &mut JSContext,
        window: &Window,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        update_range_start: u32,
        update_range_end: u32,
        text: DOMString,
        selection_start: u32,
        selection_end: u32,
    ) -> DomRoot<TextUpdateEvent> {
        Self::new_with_proto(
            cx,
            window,
            None,
            type_,
            bubbles,
            cancelable,
            update_range_start,
            update_range_end,
            text,
            selection_start,
            selection_end,
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn new_with_proto(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        update_range_start: u32,
        update_range_end: u32,
        text: DOMString,
        selection_start: u32,
        selection_end: u32,
    ) -> DomRoot<TextUpdateEvent> {
        let event = Box::new(TextUpdateEvent::new_inherited(
            update_range_start,
            update_range_end,
            text,
            selection_start,
            selection_end,
        ));
        let event = reflect_dom_object_with_proto(cx, event, window, proto);
        {
            let event = event.upcast::<Event>();
            event.init_event(type_, bool::from(bubbles), bool::from(cancelable));
        }
        event
    }
}

impl TextUpdateEventMethods<crate::DomTypeHolder> for TextUpdateEvent {
    /// <https://w3c.github.io/edit-context/#dom-textupdateevent>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &TextUpdateEventInit,
    ) -> DomRoot<TextUpdateEvent> {
        let bubbles = EventBubbles::from(init.parent.bubbles);
        let cancelable = EventCancelable::from(init.parent.cancelable);
        TextUpdateEvent::new_with_proto(
            cx,
            window,
            proto,
            Atom::from(type_),
            bubbles,
            cancelable,
            init.updateRangeStart,
            init.updateRangeEnd,
            init.text.clone(),
            init.selectionStart,
            init.selectionEnd,
        )
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }

    /// <https://w3c.github.io/edit-context/#dom-textupdateevent-updaterangestart>
    fn UpdateRangeStart(&self) -> u32 {
        self.update_range_start
    }

    /// <https://w3c.github.io/edit-context/#dom-textupdateevent-updaterangeend>
    fn UpdateRangeEnd(&self) -> u32 {
        self.update_range_end
    }

    /// <https://w3c.github.io/edit-context/#dom-textupdateevent-text>
    fn Text(&self) -> DOMString {
        self.text.clone()
    }

    /// <https://w3c.github.io/edit-context/#dom-textupdateevent-selectionstart>
    fn SelectionStart(&self) -> u32 {
        self.selection_start
    }

    /// <https://w3c.github.io/edit-context/#dom-textupdateevent-selectionend>
    fn SelectionEnd(&self) -> u32 {
        self.selection_end
    }
}
