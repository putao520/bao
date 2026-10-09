/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! W3C EditContext API (REQ-BRW-050, Bao fork self-build).
//!
//! Upstream servo has no implementation of the EditContext API; Chromium 121+
//! ships it enabled by default, so its absence is a fingerprinting vector.
//! The upstream `document::editing::EditingContext` abstraction remains the
//! engine-side editing host for DOM-backed editing — this file only adds the
//! JS-facing API and routes text-only editing actions into it when attached.

use std::cell::Cell;

use dom_struct::dom_struct;
use embedder_traits::EditingAction;
use js::context::JSContext;
use js::rust::HandleObject;
use layout_api::QueryMsg;
use script_bindings::cell::DomRefCell;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::DOMRectBinding::DOMRectMethods;
use crate::dom::bindings::codegen::Bindings::EditContextBinding::EditContextInit;
use crate::dom::bindings::codegen::Bindings::EditContextBinding::EditContextMethods;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{DomRoot, MutNullableDom};
use crate::dom::bindings::str::DOMString;
use crate::dom::datatransfer::DataTransfer;
use crate::dom::event::{Event, EventBubbles, EventCancelable, EventFlags};
use crate::dom::event::inputevent::InputEvent;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::geometry::domrect::DOMRect;
use crate::dom::html::htmlcanvaselement::HTMLCanvasElement;
use crate::dom::html::htmlelement::HTMLElement;
use crate::dom::node::{Node, NodeTraits};
use crate::dom::staticrange::StaticRange;
use crate::dom::textupdateevent::TextUpdateEvent;
use crate::dom::types::Window;

/// Plain cached geometry for the bounds surfaces of an [`EditContext`].
#[derive(Clone, Copy, Debug, Default, JSTraceable, MallocSizeOf)]
pub(crate) struct EditContextRectData {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Which character a bidi caret position is associated with, determining its
/// visual slot when the two neighbours of a logical caret offset resolve to
/// different visual positions (<https://drafts.csswg.org/css-writing-modes-4/#caret>,
/// "Before" = associated with the character before the offset, "After" = with
/// the character after it). `Default` lets the caret-motion algorithm resolve
/// the slot from the bidi levels alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, JSTraceable, MallocSizeOf)]
pub(crate) enum EditContextCaretAssociation {
    #[default]
    Default,
    Before,
    After,
}

impl EditContextRectData {
    fn from_dom_rect(rect: &DOMRect) -> Self {
        EditContextRectData {
            x: rect.X(),
            y: rect.Y(),
            width: rect.Width(),
            height: rect.Height(),
        }
    }
}

/// <https://w3c.github.io/edit-context/#dom-editcontext>
#[dom_struct]
pub(crate) struct EditContext {
    eventtarget: EventTarget,
    /// <https://w3c.github.io/edit-context/#edit-context-text>
    text: DomRefCell<DOMString>,
    /// <https://w3c.github.io/edit-context/#edit-context-selection-start>
    selection_start: Cell<u32>,
    /// <https://w3c.github.io/edit-context/#edit-context-selection-end>
    selection_end: Cell<u32>,
    /// <https://w3c.github.io/edit-context/#edit-context-character-bounds-range-start>
    character_bounds_range_start: Cell<u32>,
    /// Cached copies of the author-supplied character bounds. Values are
    /// snapshotted (not `Dom` references) so later mutation of the source
    /// `DOMRect`s does not change the cached state.
    character_bounds: DomRefCell<Vec<EditContextRectData>>,
    /// Author-supplied control bounds, <https://w3c.github.io/edit-context/#dom-editcontext-updatecontrolbounds>.
    control_bounds: DomRefCell<EditContextRectData>,
    /// Author-supplied selection bounds, <https://w3c.github.io/edit-context/#dom-editcontext-updateselectionbounds>.
    selection_bounds: DomRefCell<EditContextRectData>,
    /// The single element this EditContext is associated with via
    /// `HTMLElement.editContext`, if any.
    associated_element: MutNullableDom<HTMLElement>,
    /// Which side of a bidi boundary the caret is visually attached to,
    /// consumed by caret motion over the author's DOM selection mirror
    /// (Bao fork, REQ-BRW-050 P1). Updated by user-agent text updates and
    /// reset by author calls to `updateSelection()`.
    caret_association: Cell<EditContextCaretAssociation>,
}

impl EditContext {
    fn new_inherited(text: DOMString, selection_start: u32, selection_end: u32) -> EditContext {
        EditContext {
            eventtarget: EventTarget::new_inherited(),
            text: DomRefCell::new(text),
            selection_start: Cell::new(selection_start),
            selection_end: Cell::new(selection_end),
            character_bounds_range_start: Cell::new(0),
            character_bounds: DomRefCell::new(Vec::new()),
            control_bounds: DomRefCell::new(EditContextRectData::default()),
            selection_bounds: DomRefCell::new(EditContextRectData::default()),
            associated_element: MutNullableDom::default(),
            caret_association: Cell::new(EditContextCaretAssociation::Default),
        }
    }

    fn new(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        text: DOMString,
        selection_start: u32,
        selection_end: u32,
    ) -> DomRoot<EditContext> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(EditContext::new_inherited(text, selection_start, selection_end)),
            window,
            proto,
        )
    }

    /// The element this EditContext is attached to, if any.
    pub(crate) fn associated_element(&self) -> Option<DomRoot<HTMLElement>> {
        self.associated_element.get()
    }

    /// Associate with `element` (the element side is set by the
    /// `HTMLElement.editContext` setter, which owns both directions).
    pub(crate) fn associate_with_element(&self, element: &HTMLElement) {
        self.associated_element.set(Some(element));
    }

    /// Clear the association, if it currently points at `element`.
    pub(crate) fn dissociate_from_element(&self, element: &HTMLElement) {
        if self
            .associated_element
            .get()
            .is_some_and(|associated| &*associated == element)
        {
            self.associated_element.set(None);
        }
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updatetext>
    ///
    /// Replace the substring of `text` in the UTF-16 code unit range
    /// `[range_start, range_end)` with `new_text`. Reversed ranges behave as
    /// though the indices were swapped; out-of-bounds indices clamp to the
    /// string length and surrogate pairs are never split.
    fn replace_text_range(&self, range_start: u32, range_end: u32, new_text: &str) {
        let (start, end) = if range_start > range_end {
            (range_end, range_start)
        } else {
            (range_start, range_end)
        };
        let mut text = self.text.borrow_mut();
        let replacement = String::from(new_text);
        let mut updated = String::with_capacity(text.str().len() + replacement.len());
        let mut tail = String::new();
        let mut index = 0u32;
        for character in text.str().chars() {
            let length = character.len_utf16() as u32;
            if index + length <= start {
                // Fully before the replacement range.
                updated.push(character);
            } else if index >= end {
                // At or after the replacement range.
                tail.push(character);
            }
            // Otherwise the character is inside the range (or the range
            // boundary splits a surrogate pair): dropped.
            index += length;
        }
        updated.push_str(&replacement);
        updated.push_str(&tail);
        *text = DOMString::from(updated.as_str());
    }

    /// The length of `text` in UTF-16 code units.
    fn text_length_utf16(&self) -> u32 {
        self.text
            .borrow()
            .str()
            .chars()
            .map(|character| character.len_utf16() as u32)
            .sum()
    }

    /// Apply the state change and fire a trusted `textupdate` event at this
    /// EditContext, <https://w3c.github.io/edit-context/#update-the-text-edit-context>.
    /// `caret_association` records which side of a bidi boundary the caret is
    /// attached to following the edit ("before" for insertions and backwards
    /// deletions, "after" for forwards deletions — the Chromium caret
    /// association contract exercised by
    /// `edit-context/edit-context-bidi-caret-association.tentative.html`).
    fn apply_update_and_fire_textupdate(
        &self,
        cx: &mut JSContext,
        range_start: u32,
        range_end: u32,
        new_text: &str,
        selection_start: u32,
        selection_end: u32,
        caret_association: EditContextCaretAssociation,
    ) {
        self.replace_text_range(range_start, range_end, new_text);
        self.selection_start.set(selection_start);
        self.selection_end.set(selection_end);
        self.caret_association.set(caret_association);

        let global = self.global();
        let window = global.as_window();
        let event = TextUpdateEvent::new(
            cx,
            window,
            Atom::from("textupdate"),
            EventBubbles::DoesNotBubble,
            EventCancelable::NotCancelable,
            range_start.min(range_end),
            range_start.max(range_end),
            DOMString::from(new_text),
            selection_start,
            selection_end,
        );
        let event = event.upcast::<Event>();
        event.set_trusted(true);
        event.set_composed(true);
        event.fire(cx, self.upcast::<EventTarget>());
    }

    /// Handle a text-only [`EditingAction`] routed from the keyboard default
    /// handler because the focused element has this EditContext attached,
    /// <https://w3c.github.io/edit-context/#handle-input-for-editcontext>.
    ///
    /// Returns `true` when the action was consumed by the EditContext (even
    /// when a `beforeinput` listener canceled it), `false` when the action is
    /// not one of the raw-text inputTypes and should fall through to the
    /// regular editing pipeline.
    pub(crate) fn handle_editing_action(
        &self,
        cx: &mut JSContext,
        element: &HTMLElement,
        action: &EditingAction,
    ) -> bool {
        let data = match action {
            EditingAction::InsertText(text) => Some(text.as_str()),
            EditingAction::Backspace(_) | EditingAction::Delete => None,
            // Everything else (clipboard, selection, newline, motion, ...) is
            // not a raw-text inputType and must be handled by the author in a
            // `beforeinput` listener.
            _ => return false,
        };
        let input_type = match action {
            EditingAction::InsertText(..) => "insertText",
            EditingAction::Backspace(..) => "deleteContentBackward",
            _ => "deleteContentForward",
        };

        // The target ranges of the `beforeinput`: a single StaticRange
        // covering the EditContext's selection for insertions; none for
        // deletions and none inside `<canvas>`, where no DOM selection can
        // exist (Chromium parity).
        let target_ranges = self.target_ranges_for_insert(cx, element, action);

        if fire_beforeinput_on_element(cx, element, data, input_type, None, target_ranges) {
            // Canceled: the EditContext still consumed the key.
            return true;
        }

        let text_length = self.text_length_utf16();
        let selection_start = self.selection_start.get().min(text_length);
        let selection_end = self.selection_end.get().min(text_length);
        let (range_start, range_end) = if selection_start > selection_end {
            (selection_end, selection_start)
        } else {
            (selection_start, selection_end)
        };

        match action {
            EditingAction::InsertText(text) => {
                let caret = range_start.saturating_add(text.encode_utf16().count() as u32);
                self.apply_update_and_fire_textupdate(
                    cx,
                    range_start,
                    range_end,
                    text,
                    caret,
                    caret,
                    EditContextCaretAssociation::Before,
                );
            },
            EditingAction::Backspace(_) if range_start == range_end => {
                // Collapsed: delete the previous UTF-16 code point.
                if range_start == 0 {
                    return true;
                }
                let delete_start = previous_utf16_code_point(&self.text.borrow(), range_start);
                self.apply_update_and_fire_textupdate(
                    cx,
                    delete_start,
                    range_start,
                    "",
                    delete_start,
                    delete_start,
                    EditContextCaretAssociation::Before,
                );
            },
            EditingAction::Backspace(_) => {
                self.apply_update_and_fire_textupdate(
                    cx,
                    range_start,
                    range_end,
                    "",
                    range_start,
                    range_start,
                    EditContextCaretAssociation::Before,
                );
            },
            EditingAction::Delete if range_start == range_end => {
                // Collapsed: delete the next UTF-16 code point.
                if range_start >= text_length {
                    return true;
                }
                let delete_end = next_utf16_code_point(&self.text.borrow(), range_start);
                self.apply_update_and_fire_textupdate(
                    cx,
                    range_start,
                    delete_end,
                    "",
                    range_start,
                    range_start,
                    EditContextCaretAssociation::After,
                );
            },
            EditingAction::Delete => {
                self.apply_update_and_fire_textupdate(
                    cx,
                    range_start,
                    range_end,
                    "",
                    range_start,
                    range_start,
                    EditContextCaretAssociation::After,
                );
            },
            _ => unreachable!("Non-text actions returned early above"),
        }
        true
    }

    /// The target ranges a `beforeinput` for `action` at `element` carries:
    /// one collapsed-over-the-selection StaticRange rooted at `element` for
    /// `insertText`, none otherwise and never for `<canvas>` hosts.
    fn target_ranges_for_insert(
        &self,
        cx: &mut JSContext,
        element: &HTMLElement,
        action: &EditingAction,
    ) -> Vec<DomRoot<StaticRange>> {
        if !matches!(action, EditingAction::InsertText(..)) ||
            element.upcast::<Node>().is::<HTMLCanvasElement>()
        {
            return Vec::new();
        }
        let text_length = self.text_length_utf16();
        let selection_start = self.selection_start.get().min(text_length);
        let selection_end = self.selection_end.get().min(text_length);
        let node = element.upcast::<Node>();
        vec![StaticRange::new(
            cx,
            &element.owner_document(),
            node,
            selection_start.min(selection_end),
            node,
            selection_start.max(selection_end),
        )]
    }

    /// Which character the caret is visually attached to at a bidi boundary,
    /// for caret motion over the DOM selection inside an EditContext host.
    pub(crate) fn caret_association(&self) -> EditContextCaretAssociation {
        self.caret_association.get()
    }

    /// Set the caret association (used by caret motion when it lands on a
    /// bidi boundary).
    pub(crate) fn set_caret_association(&self, association: EditContextCaretAssociation) {
        self.caret_association.set(association);
    }

    /// Handle a trusted paste of `text` into this EditContext after the
    /// `paste` ClipboardEvent was dispatched (and not canceled): fire a
    /// cancelable `beforeinput` (`insertFromPaste`) at `element`, then update
    /// the EditContext text and fire `textupdate`. The DOM is never mutated.
    /// Returns `true` iff the `beforeinput` was canceled.
    pub(crate) fn handle_paste(&self, cx: &mut JSContext, element: &HTMLElement, text: &str) -> bool {
        // The prepopulated clipboard payload of the `beforeinput`
        // (<https://w3c.github.io/input-events/#dom-inputevent-datatransfer>).
        let data_transfer =
            DataTransfer::new_readonly_clipboard_text(cx, &element.owner_window(), text);
        if fire_beforeinput_on_element(
            cx,
            element,
            Some(text),
            "insertFromPaste",
            Some(&data_transfer),
            Vec::new(),
        ) {
            return true;
        }
        let text_length = self.text_length_utf16();
        let selection_start = self.selection_start.get().min(text_length);
        let selection_end = self.selection_end.get().min(text_length);
        let range_start = selection_start.min(selection_end);
        let range_end = selection_start.max(selection_end);
        let caret = range_start.saturating_add(text.encode_utf16().count() as u32);
        self.apply_update_and_fire_textupdate(
            cx,
            range_start,
            range_end,
            text,
            caret,
            caret,
            EditContextCaretAssociation::Before,
        );
        false
    }
}

/// Step from UTF-16 index `index` back over one full code point (a surrogate
/// pair counts as one step), returning the new index.
fn previous_utf16_code_point(text: &DOMString, index: u32) -> u32 {
    let mut seen = 0u32;
    for character in text.str().chars() {
        let next = seen + character.len_utf16() as u32;
        if next >= index {
            return seen;
        }
        seen = next;
    }
    seen
}

/// Step from UTF-16 index `index` forward over one full code point.
fn next_utf16_code_point(text: &DOMString, index: u32) -> u32 {
    let mut seen = 0u32;
    for character in text.str().chars() {
        let next = seen + character.len_utf16() as u32;
        if seen >= index {
            return next;
        }
        seen = next;
    }
    seen
}

/// Fire a cancelable `beforeinput` on `element` mirroring the text-control
/// firing path. `target_ranges` are exposed through
/// `InputEvent.getTargetRanges()` (EditContext insertions carry the
/// selection range); `data_transfer` is the clipboard payload exposed
/// through `InputEvent.dataTransfer` for clipboard inputTypes. Returns
/// `true` iff the event was canceled or the element was hidden by a listener.
pub(crate) fn fire_beforeinput_on_element(
    cx: &mut JSContext,
    element: &HTMLElement,
    data: Option<&str>,
    input_type: &str,
    data_transfer: Option<&DataTransfer>,
    target_ranges: Vec<DomRoot<StaticRange>>,
) -> bool {
    let target = element.upcast::<EventTarget>();
    let window = element.owner_window();
    let event = InputEvent::new(
        cx,
        &window,
        None,
        atom!("beforeinput"),
        true,
        true,
        Some(&window),
        0,
        data.map(DOMString::from),
        false,
        DOMString::from(input_type),
    );
    event.set_data_transfer(data_transfer);
    event.set_target_ranges(target_ranges);
    let event = event.upcast::<Event>();
    event.set_composed(true);
    event.set_trusted(true);
    event.fire(cx, target);
    let flags = event.flags();
    // We need to check if the event listener hid the element during
    // execution, so we do a layout reflow, mirroring the text-control path.
    window.layout_reflow(QueryMsg::StyleQuery);
    let node = element.upcast::<Node>();
    flags.intersects(EventFlags::Canceled) || !node.is_being_rendered_or_delegates_rendering(None)
}

impl EditContextMethods<crate::DomTypeHolder> for EditContext {
    /// <https://w3c.github.io/edit-context/#dom-editcontext>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        options: &EditContextInit,
    ) -> DomRoot<EditContext> {
        EditContext::new(
            cx,
            window,
            proto,
            options.text.clone(),
            options.selectionStart,
            options.selectionEnd,
        )
    }

    // https://w3c.github.io/edit-context/#handler-editcontext-ontextupdate
    event_handler!(textupdate, GetOntextupdate, SetOntextupdate);

    // https://w3c.github.io/edit-context/#handler-editcontext-ontextformatupdate
    event_handler!(textformatupdate, GetOntextformatupdate, SetOntextformatupdate);

    // https://w3c.github.io/edit-context/#handler-editcontext-oncharacterboundsupdate
    event_handler!(
        characterboundsupdate,
        GetOncharacterboundsupdate,
        SetOncharacterboundsupdate
    );

    // https://w3c.github.io/edit-context/#handler-editcontext-oncompositionstart
    event_handler!(compositionstart, GetOncompositionstart, SetOncompositionstart);

    // https://w3c.github.io/edit-context/#handler-editcontext-oncompositionend
    event_handler!(compositionend, GetOncompositionend, SetOncompositionend);

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updatetext>
    fn UpdateText(&self, range_start: u32, range_end: u32, text: DOMString) {
        self.replace_text_range(range_start, range_end, &text.str());
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updateselection>
    fn UpdateSelection(&self, start: u32, end: u32) {
        self.selection_start.set(start);
        self.selection_end.set(end);
        // An author-driven selection change invalidates the caret association
        // recorded by the last user-agent edit: when the textupdate handler
        // reverts an edit and resets the selection, the caret association must
        // not change (bidi caret association contract).
        self.caret_association.set(EditContextCaretAssociation::Default);
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updatecontrolbounds>
    fn UpdateControlBounds(&self, control_bounds: &DOMRect) {
        *self.control_bounds.borrow_mut() = EditContextRectData::from_dom_rect(control_bounds);
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updateselectionbounds>
    fn UpdateSelectionBounds(&self, selection_bounds: &DOMRect) {
        *self.selection_bounds.borrow_mut() = EditContextRectData::from_dom_rect(selection_bounds);
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-updatecharacterbounds>
    fn UpdateCharacterBounds(&self, range_start: u32, character_bounds: Vec<DomRoot<DOMRect>>) {
        self.character_bounds_range_start.set(range_start);
        *self.character_bounds.borrow_mut() = character_bounds
            .iter()
            .map(|rect| EditContextRectData::from_dom_rect(rect))
            .collect();
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-attachedelements>
    fn AttachedElements(&self, _cx: &mut JSContext) -> Vec<DomRoot<HTMLElement>> {
        self.associated_element.get().into_iter().collect()
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-text>
    fn Text(&self) -> DOMString {
        self.text.borrow().clone()
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-selectionstart>
    fn SelectionStart(&self) -> u32 {
        self.selection_start.get()
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-selectionend>
    fn SelectionEnd(&self) -> u32 {
        self.selection_end.get()
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-characterboundsrangestart>
    fn CharacterBoundsRangeStart(&self) -> u32 {
        self.character_bounds_range_start.get()
    }

    /// <https://w3c.github.io/edit-context/#dom-editcontext-characterbounds>
    fn CharacterBounds(&self, cx: &mut JSContext) -> Vec<DomRoot<DOMRect>> {
        let global = self.global();
        let window = global.as_window();
        self.character_bounds
            .borrow()
            .iter()
            .map(|rect| {
                DOMRect::new(
                    cx,
                    window.upcast::<GlobalScope>(),
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                )
            })
            .collect()
    }
}
