/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use js::context::JSContext;
use script_bindings::codegen::GenericBindings::CharacterDataBinding::CharacterDataMethods;
use script_bindings::codegen::GenericBindings::DocumentBinding::DocumentMethods;
use script_bindings::codegen::GenericBindings::NodeBinding::NodeMethods;
use script_bindings::codegen::GenericBindings::SelectionBinding::SelectionMethods;
use script_bindings::inheritance::Castable;

use crate::dom::Node;
use crate::dom::bindings::codegen::Bindings::RangeBinding::RangeMethods;
use crate::dom::bindings::str::DOMString;
use crate::dom::characterdata::CharacterData;
use crate::dom::document::Document;
use crate::dom::execcommand::basecommand::CommandName;
use crate::dom::execcommand::commands::insertparagraph::execute_insert_paragraph_command;
use crate::dom::execcommand::contenteditable::selection::SelectionDeletionStripWrappers;
use crate::dom::selection::Selection;
use crate::dom::text::Text;

/// <https://w3c.github.io/editing/docs/execCommand/#the-inserttext-command>
pub(crate) fn execute_insert_text_command(
    cx: &mut JSContext,
    document: &Document,
    selection: &Selection,
    value: DOMString,
) -> bool {
    // Step 1. Delete the selection, with strip wrappers false.
    selection.delete_the_selection(
        cx,
        document,
        Default::default(),
        SelectionDeletionStripWrappers::NoStrip,
        Default::default(),
    );

    // Step 2. If the active range's start node is neither editable nor an editing host, return true.
    let mut active_range = selection
        .active_range(cx)
        .expect("Must always have an active range.");
    if !active_range.start_container().is_editable_or_editing_host() {
        return true;
    }

    // Step 3. If value's length is greater than one:
    // NOTE: In theory, the 'spec' wants us to do insertText for every UTF-16 code unit. We do it for every UTF-8
    //       character instead, as that lets us avoid having to deal with inbetween states where we should have
    //       inserted one half of a surrogate pair, which would temporarily make the text in the node invalid UTF-8.
    //       This shouldn't cause any issues since none of the per-character handling cares about half a surrogate pair
    //       and the events that a MutationObserver sees aren't per-character in other browsers.
    if value.str().chars().nth(1).is_some() {
        // Step 3.1. For each code unit el in value, take the action for the insertText command, with value equal to el.
        for el in value.str().chars() {
            execute_insert_text_command(cx, document, selection, DOMString::from(el.to_string()));
        }

        // Step 3.2. Return true.
        return true;
    }

    // Step 4. If value is the empty string, return true.
    if value.is_empty() {
        return true;
    }

    // Step 5. If value is a newline (U+000A), take the action for the insertParagraph command and return true.
    if value == "\n" {
        // The paragraph command re-collapses the selection several times,
        // which resets the document's command overrides (boundary point
        // changed). Its wrapped execution would re-apply them, but this
        // call site invokes it directly, so snapshot and restore the store
        // here: a pending typing style (e.g. italic) must survive into the
        // text typed after the paragraph break.
        let newline_overrides = CommandName::record_current_overrides(document);
        execute_insert_paragraph_command(cx, document, selection);
        for override_state in newline_overrides {
            match override_state.value {
                crate::dom::execcommand::basecommand::BoolOrOptionalString::Bool(bool_) => {
                    document.set_state_override(override_state.command, Some(bool_));
                },
                crate::dom::execcommand::basecommand::BoolOrOptionalString::OptionalString(
                    optional_string,
                ) => {
                    document.set_value_override(override_state.command, optional_string);
                },
            }
        }
        return true;
    }

    // Step 6. Let node and offset be the active range's start node and offset.
    let mut node = active_range.start_container();
    let mut offset = active_range.start_offset();

    // Fork-autonomous placeholder replacement (WPT editing/other/
    // insertparagraph-with-white-space-style + insertlinebreak-with-white-
    // space-style, Chrome-91 anchored, REQ-BRW-002): typing on the empty
    // last line replaces the line's placeholder break — the collapsed-line-
    // break `br`, or a lone line-feed Text node in white-space-preserving
    // contexts — instead of stacking the text after it, which would leave
    // the placeholder rendering as an extra empty line.
    //
    // The removal mutates the DOM under the selection, which resets the
    // document's command overrides (a boundary point changed); snapshot the
    // store first and re-apply it after, so a pending typing style (e.g.
    // italic) survives into the inserted text.
    let placeholder_overrides = CommandName::record_current_overrides(document);
    let removed_placeholder = if node.is_collapsed_line_break(cx.no_gc()) &&
        node.downcast::<Text>().is_some()
    {
        // The caret sits inside the lone line-feed placeholder itself.
        let parent = node.GetParentNode();
        let index = node.index();
        node.remove_self(cx);
        if let Some(parent) = parent {
            node = parent;
            offset = index as u32;
            true
        } else {
            false
        }
    } else if offset > 0 &&
        let Some(placeholder) = node.children().nth((offset - 1) as usize) &&
        placeholder.is_collapsed_line_break(cx.no_gc())
    {
        placeholder.remove_self(cx);
        offset -= 1;
        true
    } else {
        false
    };
    if removed_placeholder {
        for override_state in placeholder_overrides {
            match override_state.value {
                crate::dom::execcommand::basecommand::BoolOrOptionalString::Bool(bool_) => {
                    document.set_state_override(override_state.command, Some(bool_));
                },
                crate::dom::execcommand::basecommand::BoolOrOptionalString::OptionalString(
                    optional_string,
                ) => {
                    document.set_value_override(override_state.command, optional_string);
                },
            }
        }
    }

    // Step 7. If node has a child whose index is offset − 1, and that child is a Text node, set node to that child, then set offset to node's length.
    if offset > 0 &&
        let Some(child) = node.children().nth((offset - 1) as usize) &&
        child.is::<Text>()
    {
        node = child;
        offset = node.len();
    }

    // Step 8. If node has a child whose index is offset, and that child is a Text node, set node to that child, then set offset to zero.
    if let Some(child) = node.children().nth((offset) as usize) &&
        child.is::<Text>()
    {
        node = child;
        offset = 0;
    }

    // Step 9. Record current overrides, and let overrides be the result.
    let overrides = CommandName::record_current_overrides(document);

    // Step 10. Call collapse(node, offset) on the context object's selection.
    if selection.Collapse(cx, Some(&node), offset).is_err() {
        unreachable!("Must always be able to collapse the selection");
    }

    // Step 11. Canonicalize whitespace at (node, offset).
    node.canonicalize_whitespace(cx, offset, Default::default());

    // Step 12. Let (node, offset) be the active range's start.
    active_range = selection
        .active_range(cx)
        .expect("Must always have an active range.");
    node = active_range.start_container();
    offset = active_range.start_offset();

    // Step 13. If node is a Text node:
    if let Some(node_as_text) = node.downcast::<Text>() {
        // Step 13.1. Call insertData(offset, value) on node.
        if node_as_text
            .upcast::<CharacterData>()
            .InsertData(cx, offset, value.clone())
            .is_err()
        {
            unreachable!("Must always be able to insert");
        }

        // Step 13.2. Call collapse(node, offset) on the context object's selection.
        if selection.Collapse(cx, Some(&node), offset).is_err() {
            unreachable!("Must always be able to collapse the selection.");
        }

        // Step 13.3. Call extend(node, offset + 1) on the context object's selection.
        // Note: We're doing this per UTF-8 character instead of per UTF-16 code unit.
        if selection
            .Extend(cx, &node, offset + value.len_utf16().0)
            .is_err()
        {
            unreachable!("Must always be able to extend the selection");
        }

        active_range = selection
            .active_range(cx)
            .expect("Must always have an active range.");
    }
    // Step 14. Otherwise:
    else {
        // Step 14.1. If node has only one child, which is a collapsed line break, remove its child from it.
        if node.children_count() == 1 &&
            let Some(only_child) = node.children().next() &&
            only_child.is_collapsed_line_break(cx.no_gc())
        {
            only_child.remove_self(cx);
        }

        // Step 14.2. Let text be the result of calling createTextNode(value) on the context object.
        let text = document.CreateTextNode(cx, value.clone());
        let text = text.upcast::<Node>();

        // Step 14.3. Call insertNode(text) on the active range.
        if active_range.InsertNode(cx, text).is_err() {
            unreachable!("Must always be able to insert");
        }

        // Step 14.4. Call collapse(text, 0) on the context object's selection.
        if selection.Collapse(cx, Some(text), 0).is_err() {
            unreachable!("Must always be able to collapse the selection");
        }

        // Step 14.5. Call extend(text, 1) on the context object's selection.
        // Note: We're doing this per UTF-8 character instead of per UTF-16 code unit.
        if selection.Extend(cx, text, value.len_utf16().0).is_err() {
            unreachable!("Must always be able to extend the selection");
        }

        active_range = selection
            .active_range(cx)
            .expect("Must always have an active range.");
    }

    // Step 15. Restore states and values from overrides.
    active_range.restore_states_and_values(cx, selection, document, overrides);

    // Step 16. Canonicalize whitespace at the active range's start, with fix collapsed space false.
    active_range
        .start_container()
        .canonicalize_whitespace(cx, active_range.start_offset(), false);

    // Step 17. Canonicalize whitespace at the active range's end, with fix collapsed space false.
    active_range
        .end_container()
        .canonicalize_whitespace(cx, active_range.end_offset(), false);

    // Step 18. If value is a space character, autolink the active range's start.
    if value == " " {
        // TODO: Implement autolink.
    }

    // Step 19. Call collapseToEnd() on the context object's selection.
    if selection.CollapseToEnd(cx).is_err() {
        unreachable!("Must always be able to CollapseToEnd here.");
    }

    // Step 20. Return true.
    true
}
