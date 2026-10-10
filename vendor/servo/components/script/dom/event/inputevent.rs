/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use embedder_traits::Cursor;
use euclid::Point2D;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::reflect_dom_object_with_proto;
use servo_base::text::Utf32CodeUnitsOrNodeOffset;
use style::Atom;
use style_traits::CSSPixel;

use crate::dom::bindings::codegen::Bindings::InputEventBinding::{self, InputEventMethods};
use crate::dom::bindings::codegen::Bindings::UIEventBinding::UIEvent_Binding::UIEventMethods;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::{Dom, DomRoot, MutNullableDom};
use crate::dom::bindings::str::DOMString;
use crate::dom::datatransfer::DataTransfer;
use crate::dom::node::Node;
use crate::dom::staticrange::StaticRange;
use crate::dom::uievent::UIEvent;
use crate::dom::window::Window;

#[dom_struct]
pub(crate) struct InputEvent {
    uievent: UIEvent,
    data: Option<DOMString>,
    is_composing: bool,
    input_type: DOMString,
    /// Target ranges of a trusted `beforeinput` dispatched by the user agent
    /// (<https://w3c.github.io/input-events/#dom-inputevent-gettargetranges>).
    /// Populated for EditContext editing hosts, where the single StaticRange
    /// covers the EditContext's selection in UTF-16 code units
    /// (Bao fork, REQ-BRW-050 P1).
    target_ranges: script_bindings::cell::DomRefCell<Vec<Dom<StaticRange>>>,
    /// Clipboard payload of a trusted `beforeinput`/`input` with a clipboard
    /// inputType (`insertFromPaste`) fired at a contenteditable host
    /// (<https://w3c.github.io/input-events/#dom-inputevent-datatransfer>),
    /// or of the `InputEventInit.dataTransfer` dictionary member carried by
    /// a script-constructed event (absent member defaults to `null`).
    data_transfer: MutNullableDom<DataTransfer>,
}

impl InputEvent {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        event_type: Atom,
        can_bubble: bool,
        cancelable: bool,
        view: Option<&Window>,
        detail: i32,
        data: Option<DOMString>,
        is_composing: bool,
        input_type: DOMString,
    ) -> DomRoot<InputEvent> {
        let event = reflect_dom_object_with_proto(
            cx,
            Box::new(InputEvent {
                uievent: UIEvent::new_inherited(),
                data,
                is_composing,
                input_type,
                target_ranges: script_bindings::cell::DomRefCell::new(Vec::new()),
                data_transfer: MutNullableDom::default(),
            }),
            window,
            proto,
        );
        event
            .uievent
            .init_event(event_type, can_bubble, cancelable, view, detail);
        event
    }

    /// Set the target ranges carried by a trusted `beforeinput`. Only the
    /// user agent sets these; script-constructed events keep an empty list.
    pub(crate) fn set_target_ranges(&self, ranges: Vec<DomRoot<StaticRange>>) {
        *self.target_ranges.borrow_mut() = ranges
            .into_iter()
            .map(|range| Dom::from_ref(&*range))
            .collect();
    }

    /// Set the clipboard payload carried by a trusted `beforeinput`/`input`
    /// with a clipboard inputType (user agent), or by the constructor's
    /// `InputEventInit.dataTransfer` dictionary member (script).
    pub(crate) fn set_data_transfer(&self, data_transfer: Option<&DataTransfer>) {
        self.data_transfer.set(data_transfer);
    }
}

impl InputEventMethods<crate::DomTypeHolder> for InputEvent {
    /// <https://w3c.github.io/uievents/#dom-inputevent-inputevent>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        event_type: DOMString,
        init: &InputEventBinding::InputEventInit,
    ) -> Fallible<DomRoot<InputEvent>> {
        let event = InputEvent::new(
            cx,
            window,
            proto,
            event_type.into(),
            init.parent.parent.bubbles,
            init.parent.parent.cancelable,
            init.parent.view.as_deref(),
            init.parent.detail,
            init.data.clone(),
            init.isComposing,
            init.inputType.clone(),
        );
        // The `InputEventInit.dataTransfer` dictionary member initializes the
        // `dataTransfer` attribute
        // (<https://w3c.github.io/input-events/#dom-inputeventinit-datatransfer>).
        event.set_data_transfer(init.dataTransfer.as_deref());
        Ok(event)
    }

    /// <https://w3c.github.io/uievents/#dom-inputevent-data>
    fn GetData(&self) -> Option<DOMString> {
        self.data.clone()
    }

    /// <https://w3c.github.io/uievents/#dom-inputevent-iscomposing>
    fn IsComposing(&self) -> bool {
        self.is_composing
    }

    /// <https://w3c.github.io/uievents/#dom-inputevent-inputtype>
    fn InputType(&self) -> DOMString {
        self.input_type.clone()
    }

    /// <https://w3c.github.io/input-events/#dom-inputevent-datatransfer>
    fn GetDataTransfer(&self) -> Option<DomRoot<DataTransfer>> {
        self.data_transfer.get()
    }

    /// <https://w3c.github.io/input-events/#dom-inputevent-gettargetranges>
    fn GetTargetRanges(&self) -> Vec<DomRoot<StaticRange>> {
        self.target_ranges
            .borrow()
            .iter()
            .map(|range| DomRoot::from_ref(&**range))
            .collect()
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.uievent.IsTrusted()
    }
}

/// A [`HitTestResult`] that is the result of doing a hit test based on a less-fine-grained
/// `PaintHitTestResult` against our current layout.
pub(crate) struct HitTestResult {
    pub node: DomRoot<Node>,
    pub dom_position_for_selection: Option<(DomRoot<Node>, Utf32CodeUnitsOrNodeOffset)>,
    pub cursor: Cursor,
    pub point_in_node: Point2D<f32, CSSPixel>,
    pub point_in_frame: Point2D<f32, CSSPixel>,
    pub point_relative_to_initial_containing_block: Point2D<f32, CSSPixel>,
}
