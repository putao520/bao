/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use js::context::JSContext;
use script_bindings::codegen::GenericBindings::DocumentBinding::DocumentMethods;
use script_bindings::codegen::GenericBindings::NodeBinding::NodeMethods;
use script_bindings::codegen::GenericBindings::RangeBinding::RangeMethods;
use script_bindings::codegen::GenericBindings::SelectionBinding::SelectionMethods;
use script_bindings::inheritance::Castable;

use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::document::Document;
use crate::dom::element::Element;
use crate::dom::Node;
use crate::dom::execcommand::contenteditable::node::{NodeOrString, is_allowed_child};
use crate::dom::execcommand::contenteditable::selection::SelectionDeletionStripWrappers;
use crate::dom::selection::Selection;
use crate::dom::text::Text;

/// <https://w3c.github.io/editing/docs/execCommand/#the-insertlinebreak-command>
pub(crate) fn execute_insert_line_break_command(
    cx: &mut JSContext,
    document: &Document,
    selection: &Selection,
) -> bool {
    // Step 1. Delete the selection, with strip wrappers false.
    selection.delete_the_selection(
        cx,
        document,
        Default::default(),
        SelectionDeletionStripWrappers::NoStrip,
        Default::default(),
    );

    // Step 2. If the active range's start node is neither editable nor an editing host, return
    //         true.
    let mut active_range = selection
        .active_range(cx)
        .expect("Must always have an active range.");
    if !active_range.start_container().is_editable_or_editing_host() {
        return true;
    }

    // Step 3. If the active range's start node is an Element, and "br" is not an allowed child of
    //         it, return true.
    if active_range.start_container().is::<Element>() &&
        !is_allowed_child(
            NodeOrString::String("br".to_owned()),
            NodeOrString::from_node(&active_range.start_container(), cx.no_gc()),
        )
    {
        return false;
    }

    // Step 4. If the active range's start node is not an Element, and "br" is not an allowed child
    //         of the active range's start node's parent, return true.
    if !active_range.start_container().is::<Element>() &&
        !is_allowed_child(
            NodeOrString::String("br".to_owned()),
            NodeOrString::from_node(
                &active_range
                    .start_container()
                    .GetParentNode()
                    .expect("Must always have a parent."),
                cx.no_gc(),
            ),
        )
    {
        return false;
    }

    // Step 5. If the active range's start node is a Text node and its start offset is zero, call
    //         collapse() on the context object's selection, with first argument equal to the
    //         active range's start node's parent and second argument equal to the active range's
    //         start node's index.
    if active_range.start_container().is::<Text>() && active_range.start_offset() == 0 {
        if selection
            .Collapse(
                cx,
                active_range.start_container().GetParentNode().as_deref(),
                active_range.start_container().index(),
            )
            .is_err()
        {
            unreachable!("Should always be able to collapse the selection.");
        }
        active_range = selection
            .active_range(cx)
            .expect("Must always have an active range");
    }

    // Step 6. If the active range's start node is a Text node and its start offset is the length
    //         of its start node, call collapse() on the context object's selection, with first
    //         argument equal to the active range's start node's parent and second argument equal
    //         to one plus the active range's start node's index.
    if active_range.start_container().is::<Text>() &&
        active_range.start_offset() == active_range.start_container().len()
    {
        if selection
            .Collapse(
                cx,
                active_range.start_container().GetParentNode().as_deref(),
                1 + active_range.start_container().index(),
            )
            .is_err()
        {
            unreachable!("Should always be able to collapse the selection.");
        }
        active_range = selection
            .active_range(cx)
            .expect("Must always have an active range");
    }

    // Step 7. Let br be the result of calling createElement("br") on the context object.
    //
    // Fork-autonomous form (WPT editing/other/insertlinebreak-with-white-space-style,
    // Chrome-91 anchored, REQ-BRW-002): where the computed white-space of the
    // caret's governing element preserves line feeds (pre/pre-wrap/pre-line),
    // the line break is a literal `\n` Text node — a `br` there would be a
    // second, redundant representation of the same rendered break.
    let use_line_feed = active_range.start_container().line_feed_is_significant();
    let break_node: DomRoot<Node> = if use_line_feed {
        let text = document.CreateTextNode(cx, DOMString::from_static("\n"));
        DomRoot::upcast(text)
    } else {
        DomRoot::upcast(document.create_element(cx, "br"))
    };

    // Step 8. Call insertNode(br) on the active range.
    if active_range.InsertNode(cx, &break_node).is_err() {
        unreachable!("The node should always be insertable.");
    }

    // Step 9. Call collapse() on the context object's selection, with br's parent as the first
    //         argument and one plus br's index as the second argument. The line-feed form
    //         collapses inside its Text node, past the inserted character.
    collapse_after_line_break(cx, selection, &break_node);
    // The collapse re-anchored the selection; re-read the active range so the
    // extra break below lands after the first one, not at its stale position.
    active_range = selection
        .active_range(cx)
        .expect("Must always have an active range");

    // Step 10. If br is a collapsed line break, call createElement("br") on the context object and
    //          let extra br be the result, then call insertNode(extra br) on the active range.
    // The extra break makes the line the caret is on render: a break that begins a
    // zero-height line box at the end of its block leaves the caret with no
    // visible line of its own. The extra break is always a `br`, also in the
    // line-feed mode (the accepted line-feed forms are `\n` followed by
    // either another `\n` or a `br`; the br form is what the editing/run
    // conformance data expects for the trailing placeholder).
    if break_node.is_collapsed_line_break(cx.no_gc()) {
        let extra_break: DomRoot<Node> = DomRoot::upcast(document.create_element(cx, "br"));
        if active_range.InsertNode(cx, &extra_break).is_err() {
            unreachable!("The node should always be insertable.");
        }
        collapse_after_line_break(cx, selection, &extra_break);
    }

    // Step 11. Return true.
    true
}

/// Collapse the selection immediately after the inserted line-break node:
/// inside its Text node past the line feed for the line-feed form, at
/// (parent, one plus node index) for the `br` form.
fn collapse_after_line_break(cx: &mut JSContext, selection: &Selection, node: &Node) {
    let result = if node.is::<Text>() {
        // Node::len() on a Text node is its data length.
        selection.Collapse(cx, Some(node), node.len())
    } else {
        selection.Collapse(cx, node.GetParentNode().as_deref(), 1 + node.index())
    };
    if result.is_err() {
        unreachable!("Should always be able to collapse the selection.");
    }
}
