/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Visual caret motion for DOM selections inside editable regions
//! (REQ-BRW-050 P1, Bao fork self-build).
//!
//! Arrow keys move the caret one *visual* step through the bidi-resolved
//! line, not one logical code unit: at a boundary between runs of different
//! direction the two logical neighbours of a caret position are not visual
//! neighbours, and which visual slot the caret occupies is decided by its
//! association (<https://drafts.csswg.org/css-writing-modes-4/#caret>).
//! The EditContext tracks that association across user-agent edits
//! ("before" after insertions and backwards deletions, "after" after
//! forwards deletions) and resets it when the author moves the selection,
//! which is the contract exercised by
//! `editing/edit-context/edit-context-bidi-caret-association.tentative.html`.

use embedder_traits::{EditingDirection, EditingMotion, ModifySelection};
use js::context::JSContext;
use unicode_bidi::BidiInfo;

use crate::dom::bindings::codegen::GenericBindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::GenericBindings::SelectionBinding::SelectionMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::characterdata::CharacterData;
use crate::dom::document::Document;
use crate::dom::editcontext::EditContextCaretAssociation;
use crate::dom::html::htmlelement::HTMLElement;
use crate::dom::node::{Node, NodeTraits};
use crate::dom::text::Text;

/// A caret position resolved against the bidi ordering of its text run:
/// the UTF-16 offset plus the association used when the offset sits at a
/// boundary between runs of different direction.
struct VisualCaret {
    offset_utf16: u32,
    association: EditContextCaretAssociation,
}

impl Document {
    /// Perform one visual caret motion step over the DOM selection. Returns
    /// `true` when the motion was consumed (the selection lives inside an
    /// editable region), `false` when it should fall through to the default
    /// key handling.
    pub(crate) fn perform_caret_motion(
        &self,
        cx: &mut JSContext,
        direction: EditingDirection,
        motion: EditingMotion,
        modify_selection: ModifySelection,
    ) -> bool {
        if motion != EditingMotion::Grapheme || modify_selection != ModifySelection::No {
            // Only plain grapheme motion is bidi-resolved here; selection
            // extension and other granularities keep their previous handling.
            return false;
        }
        let Some(selection) = self.GetSelection(cx) else {
            return false;
        };
        let Some(focus_node) = selection.GetFocusNode(cx) else {
            return false;
        };
        // The motion only applies to selections inside an editable region;
        // selections in non-editable content must not move
        // (edit-context-selection-outside-host contract).
        let Some(editing_host) = focus_node.editing_host_of() else {
            return false;
        };
        let edit_context = editing_host
            .downcast::<HTMLElement>()
            .and_then(|host| host.attached_edit_context());

        // A non-collapsed selection collapses to its leftmost (leftward) or
        // rightmost (rightward) boundary instead of moving.
        if !selection.IsCollapsed(cx) {
            if let Some(active_range) = selection.active_range(cx) {
                let (node, offset) = match direction {
                    EditingDirection::Backward => {
                        (active_range.start_container(), active_range.start_offset())
                    },
                    EditingDirection::Forward => {
                        (active_range.end_container(), active_range.end_offset())
                    },
                };
                let _ = selection.Collapse(cx, Some(&node), offset);
            }
            if let Some(edit_context) = edit_context.as_deref() {
                edit_context.set_caret_association(EditContextCaretAssociation::Default);
            }
            return true;
        }

        let Some(text) = focus_node.downcast::<Text>() else {
            // Caret positions on non-text nodes are not bidi-resolved here;
            // consume the key so an editable region does not scroll instead.
            return true;
        };
        let offset = selection.FocusOffset(cx);
        let text_data = text.upcast::<CharacterData>().data();
        let (new_offset, new_association) = match visual_caret_step(
            &text_data,
            offset,
            direction,
            edit_context
                .as_deref()
                .map_or(EditContextCaretAssociation::Default, |ec| {
                    ec.caret_association()
                }),
        ) {
            Some(step) => step,
            // At the edge of the text (or an unresolvable position): consume
            // the key without moving.
            None => return true,
        };
        let node = text.upcast::<Node>();
        let _ = selection.Collapse(cx, Some(node), new_offset);
        if let Some(edit_context) = edit_context.as_deref() {
            edit_context.set_caret_association(new_association);
        }
        true
    }
}

/// One visual step through the bidi-resolved `text` from the caret at
/// UTF-16 `offset`, returning the new UTF-16 offset and association.
fn visual_caret_step(
    text: &str,
    offset: u32,
    direction: EditingDirection,
    association: EditContextCaretAssociation,
) -> Option<(u32, EditContextCaretAssociation)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    // Map the UTF-16 offset to a char boundary (never split a surrogate
    // pair: snap forward).
    let mut char_index = 0usize;
    let mut utf16_seen = 0u32;
    for (index, character) in chars.iter().enumerate() {
        if utf16_seen >= offset {
            char_index = index;
            break;
        }
        utf16_seen += character.len_utf16() as u32;
        char_index = index + 1;
    }
    if char_index > chars.len() {
        return None;
    }

    // Bidi resolution of the whole text (single paragraph, auto-detected
    // base direction, mirroring layout). `BidiInfo::levels` is indexed per
    // *byte*; the per-character levels (one entry per `char`) are what the
    // caret model works with.
    let bidi_info = BidiInfo::new(text, None);
    let paragraph = bidi_info.paragraphs.first()?;
    let levels: Vec<u8> = bidi_info
        .reordered_levels_per_char(paragraph, paragraph.range.clone())
        .iter()
        .map(|level| level.number())
        .collect();
    let visual = visual_order(&levels);
    let mut logical_to_visual = vec![0usize; chars.len()];
    for (visual_index, logical_index) in visual.iter().enumerate() {
        logical_to_visual[*logical_index] = visual_index;
    }

    // The visual slot the caret currently occupies. Slot `s` sits between
    // the chars at visual positions `s - 1` and `s`.
    let slot = match char_index {
        0 => 0,
        n if n == chars.len() => chars.len(),
        index => {
            let before_char = index - 1;
            let after_char = index;
            // Association "before": attached to the trailing edge of the
            // preceding char — on its right for LTR, on its left for RTL.
            let slot_before = if levels[before_char] % 2 == 0 {
                logical_to_visual[before_char] + 1
            } else {
                logical_to_visual[before_char]
            };
            // Association "after": attached to the leading edge of the
            // following char — on its left for LTR, on its right for RTL.
            let slot_after = if levels[after_char] % 2 == 0 {
                logical_to_visual[after_char]
            } else {
                logical_to_visual[after_char] + 1
            };
            match association {
                EditContextCaretAssociation::Before => slot_before,
                EditContextCaretAssociation::After => slot_after,
                EditContextCaretAssociation::Default => {
                    if slot_before == slot_after {
                        slot_before
                    } else {
                        // An unassociated caret at a directional boundary
                        // takes the side of the lower-level (base direction)
                        // character.
                        if levels[before_char] < levels[after_char] {
                            slot_before
                        } else {
                            slot_after
                        }
                    }
                },
            }
        },
    };

    let new_slot = match direction {
        EditingDirection::Backward => slot.checked_sub(1)?,
        EditingDirection::Forward => {
            if slot >= chars.len() {
                return None;
            }
            slot + 1
        },
    };

    // Map the new slot back to a logical position. Arriving from the left
    // (moving right) attaches to the trailing edge of the char just passed;
    // arriving from the right (moving left) attaches to the leading edge of
    // the char about to be entered.
    let (new_char_index, new_association) = match direction {
        EditingDirection::Forward => (visual[new_slot - 1] + 1, EditContextCaretAssociation::Before),
        EditingDirection::Backward => (visual[new_slot], EditContextCaretAssociation::After),
    };
    let new_offset: u32 = chars[..new_char_index]
        .iter()
        .map(|character| character.len_utf16() as u32)
        .sum();
    Some((new_offset, new_association))
}

/// The bidi-resolved visual order of a text run: `visual[i]` is the logical
/// char index displayed at visual position `i` (UAX #9 rule L2).
fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let max_level = levels.iter().copied().max().unwrap_or(0);
    for level in (1..=max_level).rev() {
        let mut start = None;
        for index in 0..=levels.len() {
            let at_level = index < levels.len() && levels[index] >= level;
            if at_level && start.is_none() {
                start = Some(index);
            } else if !at_level && let Some(begin) = start {
                order[begin..index].reverse();
                start = None;
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        text: &str,
        offset: u32,
        direction: EditingDirection,
        association: EditContextCaretAssociation,
    ) -> Option<(u32, EditContextCaretAssociation)> {
        visual_caret_step(text, offset, direction, association)
    }

    /// Insertion caret association: in "aא123בגa" the caret at logical 5 is
    /// associated "before" (with the '3'); pressing left lands before the
    /// '3' (logical 4), not before the 'ב'.
    #[test]
    fn bidi_insert_association_moves_left_before_digits() {
        let got = step(
            "aא123בגa",
            5,
            EditingDirection::Backward,
            EditContextCaretAssociation::Before,
        );
        assert_eq!(got, Some((4, EditContextCaretAssociation::After)));
    }

    /// Forwards deletion sets association "after": in "aא123גa" the caret at
    /// logical 5 associated "after" (with the 'ג') presses right to before
    /// the '2' (logical 3).
    #[test]
    fn bidi_forward_delete_association_moves_right_into_digits() {
        let got = step(
            "aא123גa",
            5,
            EditingDirection::Forward,
            EditContextCaretAssociation::After,
        );
        assert_eq!(got, Some((3, EditContextCaretAssociation::Before)));
    }

    /// Backwards deletion sets association "before": in "aא12בגa" the caret
    /// at logical 4 associated "before" (with the '2') presses left to
    /// before the '2' (logical 3).
    #[test]
    fn bidi_backward_delete_association_moves_left_before_digits() {
        let got = step(
            "aא12בגa",
            4,
            EditingDirection::Backward,
            EditContextCaretAssociation::Before,
        );
        assert_eq!(got, Some((3, EditContextCaretAssociation::After)));
    }

    /// An unassociated caret at a directional boundary takes the
    /// base-direction side: in "aאבגbc" the caret at logical 4 pressing
    /// right moves to logical 5 ("c" after the caret).
    #[test]
    fn bidi_default_association_takes_base_direction_side() {
        let got = step(
            "aאבגbc",
            4,
            EditingDirection::Forward,
            EditContextCaretAssociation::Default,
        );
        assert_eq!(got, Some((5, EditContextCaretAssociation::Before)));
    }

    /// The reversed association at the same boundary moves the caret to the
    /// other side, demonstrating the association is load-bearing.
    #[test]
    fn bidi_association_is_load_bearing() {
        // Same position as above but associated "before" (with the 'א'):
        // the caret is on the left side of the digits run's visual start,
        // pressing right enters the digits after '1' ... in "aאבגbc" there
        // are no digits; use the "aא123בגa" boundary instead. The caret at
        // logical 5 associated "after" (with the 'ב') presses right onto the
        // '2' side (logical 3), i.e. the opposite slot from the insert case.
        let got = step(
            "aא123בגa",
            5,
            EditingDirection::Forward,
            EditContextCaretAssociation::After,
        );
        // From the 'ב' side of the boundary, pressing right steps over the
        // whole digits run's first char ('1' is the visual right neighbour
        // of the boundary slot): the caret lands before the '2'.
        assert_eq!(got, Some((3, EditContextCaretAssociation::Before)));
    }

    /// Boundaries clamp: no movement past either end of the text.
    #[test]
    fn bidi_boundaries_clamp() {
        assert!(step("abc", 0, EditingDirection::Backward,
                     EditContextCaretAssociation::Default).is_none());
        assert!(step("abc", 3, EditingDirection::Forward,
                     EditContextCaretAssociation::Default).is_none());
    }
}
