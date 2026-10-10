/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use html5ever::local_name;
use js::context::JSContext;
use script_bindings::codegen::GenericBindings::SelectionBinding::SelectionMethods;
use script_bindings::inheritance::Castable;
use style::values::specified::box_::DisplayOutside;

use crate::dom::Node;
use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::Bindings::RangeBinding::RangeMethods;
use crate::dom::bindings::codegen::Bindings::TextBinding::TextMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::comment::Comment;
use crate::dom::document::Document;
use crate::dom::element::Element;
use crate::dom::execcommand::basecommand::CommandName;
use crate::dom::execcommand::commands::insertlinebreak::execute_insert_line_break_command;
use crate::dom::execcommand::contenteditable::node::{
    NodeOrString, is_allowed_child, node_matches_local_name, split_the_parent, wrap_node_list,
};
use crate::dom::html::htmlbrelement::HTMLBRElement;
use crate::dom::iterators::ShadowIncluding;
use crate::dom::selection::Selection;
use crate::dom::text::Text;

/// <https://w3c.github.io/editing/docs/execCommand/#the-insertparagraph-command>
pub(crate) fn execute_insert_paragraph_command(
    cx: &mut JSContext,
    document: &Document,
    selection: &Selection,
) -> bool {
    // Step 1. Delete the selection.
    selection.delete_the_selection(
        cx,
        document,
        Default::default(),
        Default::default(),
        Default::default(),
    );
    // Step 3. Let node and offset be the active range's start node and offset.
    let (mut node, mut offset) = selection.start_boundary(cx);
    // Step 2. If the active range's start node is neither editable
    // nor an editing host, return true.
    if !node.is_editable_or_editing_host() {
        return true;
    }
    // Step 4. If node is a Text node, and offset is neither 0 nor the length of node,
    // call splitText(offset) on node.
    if offset != 0 &&
        offset != node.len() &&
        let Some(text_node) = node.downcast::<Text>() &&
        text_node.SplitText(cx, offset).is_err()
    {
        unreachable!("Must always be able to split");
    }
    // Step 5. If node is a Text node and offset is its length,
    // set offset to one plus the index of node, then set node to its parent.
    if node.is::<Text>() && offset == node.len() {
        offset = 1 + node.index();
        node = node.GetParentNode().expect("Must always have a parent");
    }
    // Step 6. If node is a Text or Comment node, set offset to the index of node,
    // then set node to its parent.
    if node.is::<Text>() || node.is::<Comment>() {
        offset = node.index();
        node = node.GetParentNode().expect("Must always have a parent");
    }
    // Step 7. Call collapse(node, offset) on the context object's selection.
    let _ = selection.Collapse(cx, Some(&node), offset);
    // Fork-autonomous branch (WPT editing/other/insertparagraph-with-white-
    // space-style, Chrome-91 anchored, REQ-BRW-002): an inline editing host
    // (display: inline/inline-block) takes a line break instead of a
    // paragraph — breaking its inline content into blocks would destroy the
    // host's own inline formatting. Only when no block element intervenes
    // between the caret and the host; a caret inside a block paragraph
    // splits the paragraph even in an inline host. This check runs before
    // the boundary lift below: a line break belongs inside the inline
    // element at the caret, not lifted out of it.
    if node
        .editing_host_of()
        .is_some_and(|host| {
            !node
                .inclusive_ancestors(ShadowIncluding::No)
                .take_while(|ancestor| ancestor != &host)
                .any(|ancestor| ancestor.is_block_node()) &&
                host.downcast::<Element>()
                    .and_then(Element::resolved_display_value)
                    .is_some_and(|display| display == DisplayOutside::Inline)
        })
    {
        return execute_insert_line_break_command(cx, document, selection);
    }
    // Fork-autonomous normalization (same test family): lift a caret that
    // sits at an inline element's boundary out to the equivalent parent
    // position, so block-level insertions below (the new paragraph) never
    // nest a block inside inline content. The walk stops at the editing host
    // (never lifts out of it) and at single-line containers (a p/div
    // paragraph is the split target, not an obstacle). Mid-element carets
    // are handled by the extraction (it splits the inline ancestors).
    while let Some(element) = node.downcast::<Element>() &&
        element.upcast::<Node>().is_inline_node() &&
        !node.is_single_line_container() &&
        !node.is_editing_host() &&
        let Some(parent) = node.GetParentNode()
    {
        if offset == 0 {
            // Also covers void/empty inline elements (length zero): the
            // caret sits before them, so they belong to the tail.
            offset = node.index();
        } else if offset == node.len() {
            offset = 1 + node.index();
        } else {
            break;
        }
        node = parent;
        let _ = selection.Collapse(cx, Some(&node), offset);
    }
    // Step 8. Let container equal node.
    let mut container = node.clone();
    // Step 9. While container is not a single-line container,
    // and container's parent is editable and in the same editing host as node,
    // set container to its parent.
    while !container.is_single_line_container() &&
        let Some(parent) = container.GetParentNode() &&
        parent.is_editable() &&
        parent.same_editing_host(&node)
    {
        container = parent;
    }
    // Step 10. If container is an editable single-line container in the same editing host as node,
    // and its local name is "p" or "div":
    if container.is_editable() &&
        container.is_single_line_container() &&
        container.same_editing_host(&node) &&
        node_matches_local_name!(container, local_name!("p") | local_name!("div"))
    {
        // Step 10.1. Let outer container equal container.
        let mut outer_container = container.clone();
        // Step 10.2. While outer container is not a dd or dt or li,
        // and outer container's parent is editable, set outer container to its parent.
        while !node_matches_local_name!(
            outer_container,
            local_name!("dd") | local_name!("dt") | local_name!("li")
        ) && let Some(parent) = outer_container.GetParentNode() &&
            parent.is_editable()
        {
            outer_container = parent;
        }
        // Step 10.3. If outer container is a dd or dt or li, set container to outer container.
        if node_matches_local_name!(
            outer_container,
            local_name!("dd") | local_name!("dt") | local_name!("li")
        ) {
            container = outer_container;
        }
    }
    // Step 11. If container is not editable or not in the same editing host as node or is not a single-line container:
    if !container.is_editable() ||
        !container.same_editing_host(&node) ||
        !container.is_single_line_container()
    {
        // Step 11.1. Let tag be the default single-line container name.
        let tag = document.default_single_line_container_name();
        // Step 11.2. Block-extend the active range, and let new range be the result.
        let new_range = selection.expect_active_range(cx).block_extend(cx, document);
        // Step 11.4. Append to node list the first node in tree order that is contained in new range and is an allowed child of "p", if any.
        let mut node_list = if let Some(eligible_node) = new_range
            .contained_children(cx.no_gc())
            .ok()
            .and_then(|contained_children| {
                contained_children
                    .contained_children
                    .into_iter()
                    .find(|node| {
                        is_allowed_child(
                            NodeOrString::from_node(node, cx.no_gc()),
                            NodeOrString::String("p".to_owned()),
                        )
                    })
            }) {
            vec![eligible_node]
        } else {
            // Step 11.3. Let node list be a list of nodes, initially empty.
            // Step 11.5. If node list is empty:
            //
            // Fork-autonomous rework (WPT insertparagraph-with-white-space-style,
            // Chrome-91 anchored, REQ-BRW-002): instead of the spec's empty-
            // paragraph drop at the caret (with its bail when the caret's node
            // cannot contain blocks), split the caret's line inside its block:
            // the content after the caret moves into the new paragraph —
            // unless nothing visible precedes the caret, in which case the
            // content stays in place and the new paragraph (with a br
            // placeholder) is inserted at the caret.
            let mut block = node.clone();
            while !block.is_block_node() &&
                let Some(parent) = block.GetParentNode()
            {
                block = parent;
            }
            // Step 11.5.1. If tag is not an allowed child of the block, return true.
            if !is_allowed_child(
                NodeOrString::String(tag.str().to_owned()),
                NodeOrString::from_node(&block, cx.no_gc()),
            ) {
                return true;
            }
            // Step 11.5.2. Set container to the result of calling createElement(tag)
            // on the context object.
            let new_paragraph = document.create_element(cx, tag.str());
            let new_paragraph_node = DomRoot::upcast::<Node>(new_paragraph);
            let has_visible_before = node
                .children()
                .take(offset as usize)
                .any(|child| child.is_visible(cx.no_gc()));
            // Fork-autonomous typing-style carry (same test family): typed
            // text after a paragraph split continues the inline style of the
            // text before the caret (Chromium behavior). The exec-command
            // override store only tracks explicitly toggled commands, so
            // capture the deepest node before the caret here and mirror its
            // DOM style into the state overrides below, where typing lands
            // in the unstyled new paragraph.
            let style_source = if offset > 0 {
                node.children().nth((offset - 1) as usize).map(|mut child| {
                    while let Some(last) = child.children().last() {
                        child = last;
                    }
                    child
                })
            } else {
                None
            };
            if has_visible_before {
                // Move the tail (caret .. end of the caret's inline run)
                // into the new paragraph; the extraction splits the inline
                // ancestors of the caret. The tail is bounded by the first
                // block node following the caret within the block —
                // following blocks are not part of this line.
                let (end_node, end_offset) = if node == block {
                    // The caret sits directly in the block: the run ends at
                    // the first block child after the offset.
                    match block
                        .children()
                        .enumerate()
                        .skip(offset as usize)
                        .find(|(_, child)| child.is_block_node())
                    {
                        Some((index, _)) => (block.clone(), index as u32),
                        None => (block.clone(), block.len()),
                    }
                } else {
                    let mut boundary: Option<(DomRoot<Node>, u32)> = None;
                    for following in
                        node.following_nodes_unrooted(cx.no_gc(), &block, ShadowIncluding::No)
                    {
                        if following.is_block_node() {
                            let following = following.as_rooted();
                            boundary = Some((
                                following.GetParentNode().expect("Must have a parent"),
                                following.index() as u32,
                            ));
                            break;
                        }
                    }
                    boundary.unwrap_or((block.clone(), block.len()))
                };
                let new_line_range = document.CreateRange(cx);
                let _ = new_line_range.SetStart(cx.no_gc(), &node, offset);
                let _ = new_line_range.SetEnd(cx.no_gc(), &end_node, end_offset);
                if let Ok(frag) = new_line_range.ExtractContents(cx) {
                    let _ = new_paragraph_node.AppendChild(cx, frag.upcast::<Node>());
                }
                // The extraction left the caret inside the truncated inline
                // ancestors; lift it back out to the block level so the
                // paragraph is inserted there, not nested inside them.
                let (mut lift_node, mut lift_offset) = selection.start_boundary(cx);
                while lift_node.is_inline_node() &&
                    let Some(parent) = lift_node.GetParentNode()
                {
                    if lift_offset != lift_node.len() {
                        break;
                    }
                    lift_offset = 1 + lift_node.index();
                    lift_node = parent;
                }
                let _ = selection.Collapse(cx, Some(&lift_node), lift_offset);
            }
            // Step 11.5.3. Call insertNode(container) on the active range.
            if selection
                .expect_active_range(cx)
                .InsertNode(cx, &new_paragraph_node)
                .is_err()
            {
                unreachable!("Must always be able to insert");
            }
            // Step 11.5.4. A paragraph with no visible children gets a br
            // placeholder as its last child.
            if new_paragraph_node
                .children()
                .all(|child| child.is_invisible(cx.no_gc()))
            {
                let br = document.create_element(cx, "br");
                if new_paragraph_node.AppendChild(cx, br.upcast()).is_err() {
                    unreachable!("Must always be able to append");
                }
            }
            // Step 11.5.5. Call collapse(container, 0) on the context object's selection.
            let _ = selection.Collapse(cx, Some(&new_paragraph_node), 0);
            // The typing-style carry captured above: mirror the inline
            // command state of the text before the caret into the override
            // store, so text typed in the new paragraph keeps that style.
            if let Some(style_source) = style_source {
                for command in [
                    CommandName::Bold,
                    CommandName::Italic,
                    CommandName::Strikethrough,
                    CommandName::Subscript,
                    CommandName::Superscript,
                    CommandName::Underline,
                ] {
                    if document.state_override(&command).is_none() &&
                        style_source
                            .effective_command_value(&command)
                            .is_some_and(|value| {
                                command
                                    .inline_command_activated_values()
                                    .iter()
                                    .any(|activated| value.str() == *activated)
                            })
                    {
                        document.set_state_override(command, Some(true));
                    }
                }
            }
            // Step 11.5.6. Return true.
            return true;
        };
        // Step 11.6. While the nextSibling of the last member of node list is not null
        // and is an allowed child of "p", append it to node list.
        while let Some(next_of_last) = node_list
            .iter()
            .last()
            .and_then(|node| node.GetNextSibling())
            .filter(|next_of_last| {
                is_allowed_child(
                    NodeOrString::from_node(next_of_last, cx.no_gc()),
                    NodeOrString::String("p".to_owned()),
                )
            })
        {
            node_list.push(next_of_last);
        }
        // Step 11.7. Wrap node list, with sibling criteria returning false
        // and new parent instructions returning the result of calling createElement(tag) on the context object.
        // Set container to the result.
        container = wrap_node_list(
            cx,
            node_list,
            |_| false,
            |cx| Some(DomRoot::upcast(document.create_element(cx, tag.str()))),
        )
        .expect("Must always be able to wrap");
    }
    // Step 12. If container's local name is "address", "listing", or "pre":
    if node_matches_local_name!(
        container,
        local_name!("address") | local_name!("listing") | local_name!("pre")
    ) {
        // Step 12.1. Let br be the result of calling createElement("br") on the context object.
        let br = document.create_element(cx, "br");
        // Step 12.2. Call insertNode(br) on the active range.
        if selection
            .expect_active_range(cx)
            .InsertNode(cx, br.upcast())
            .is_err()
        {
            unreachable!("Must always be able to insert");
        }
        // Step 12.3. Call collapse(node, offset + 1) on the context object's selection.
        let _ = selection.Collapse(cx, Some(&node), offset + 1);
        // Step 12.4. If br is the last descendant of container,
        // let br be the result of calling createElement("br") on the context object,
        // then call insertNode(br) on the active range.
        if container
            .children()
            .last()
            .is_some_and(|child| *child == *br.upcast())
        {
            let br = document.create_element(cx, "br");
            if selection
                .expect_active_range(cx)
                .InsertNode(cx, br.upcast())
                .is_err()
            {
                unreachable!("Must always be able to insert");
            }
        }
        // Step 12.5. Return true.
        return true;
    }
    // Step 13. If container's local name is "li", "dt", or "dd";
    // and either it has no children or it has a single child and that child is a br:
    if node_matches_local_name!(
        container,
        local_name!("li") | local_name!("dt") | local_name!("dd")
    ) && (container.children_count() == 0 ||
        (container.children_count() == 1 &&
            container
                .children()
                .next()
                .expect("has one child")
                .is::<HTMLBRElement>()))
    {
        // Step 13.1. Split the parent of the one-node list consisting of container.
        split_the_parent(cx, &[&container]);
        // Step 13.2. If container has no children,
        // call createElement("br") on the context object and append the result as the last child of container.
        if container.children_count() == 0 {
            let br = document.create_element(cx, "br");
            if container.AppendChild(cx, br.upcast()).is_err() {
                unreachable!("Must always be able to append");
            }
        }
        // Step 13.3. If container is a dd or dt,
        // and it is not an allowed child of any of its ancestors in the same editing host,
        // set the tag name of container to the default single-line container name and let container be the result.
        if node_matches_local_name!(container, local_name!("dd") | local_name!("dt")) &&
            container.is_no_allowed_child_in_same_editing_host(cx.no_gc())
        {
            container = container
                .downcast::<Element>()
                .expect("Must always be an element")
                .set_the_tag_name(cx, document.default_single_line_container_name().str());
        }
        // Step 13.4. Fix disallowed ancestors of container.
        container.fix_disallowed_ancestors(cx, document);
        // Step 13.5. Return true.
        return true;
    }
    // Step 14. Let new line range be a new range whose start is the same as the active range's,
    // and whose end is (container, length of container).
    let new_line_range = document.CreateRange(cx);
    let (start_container, start_offset) = selection.start_boundary(cx);
    let _ = new_line_range.SetStart(cx.no_gc(), &start_container, start_offset);
    let _ = new_line_range.SetEnd(cx.no_gc(), &container, container.len());
    // Step 15. While new line range's start offset is zero and its start node
    // is not a prohibited paragraph child,
    // set its start to (parent of start node, index of start node).
    while new_line_range.start_offset() == 0 &&
        !new_line_range
            .start_container()
            .is_prohibited_paragraph_child()
    {
        let start = new_line_range.start_container();
        let _ = new_line_range.SetStart(
            cx.no_gc(),
            &start.GetParentNode().expect("Must always have a parent"),
            start.index(),
        );
    }
    // Step 16. While new line range's start offset is the length of its start node
    // and its start node is not a prohibited paragraph child,
    // set its start to (parent of start node, 1 + index of start node).
    while new_line_range.start_offset() == new_line_range.start_container().len() &&
        !new_line_range
            .start_container()
            .is_prohibited_paragraph_child()
    {
        let start = new_line_range.start_container();
        let _ = new_line_range.SetStart(
            cx.no_gc(),
            &start.GetParentNode().expect("Must always have a parent"),
            1 + start.index(),
        );
    }
    // Step 17. Let end of line be true if new line range contains either nothing or a single br, and false otherwise.
    let end_of_line =
        new_line_range
            .contained_children(cx.no_gc())
            .is_ok_and(|contained_children| {
                let contained_children = contained_children.contained_children;
                contained_children.is_empty() ||
                    (contained_children.len() == 1 &&
                        contained_children[0].is::<HTMLBRElement>())
            });
    // Step 18. If the local name of container is "h1", "h2", "h3", "h4", "h5", or "h6",
    // and end of line is true, let new container name be the default single-line container name.
    let container_as_element = container
        .downcast::<Element>()
        .expect("Must always be an element");
    let container_name = container_as_element.local_name();
    let new_container_name = if end_of_line &&
        matches!(
            *container_name,
            local_name!("h1") |
                local_name!("h2") |
                local_name!("h3") |
                local_name!("h4") |
                local_name!("h5") |
                local_name!("h6")
        ) {
        document
            .default_single_line_container_name()
            .str()
            .to_owned()
    } else
    // Step 19. Otherwise, if the local name of container is "dt" and end of line is true, let new container name be "dd".
    if end_of_line && container_name == &local_name!("dt") {
        "dd".to_owned()
    } else
    // Step 20. Otherwise, if the local name of container is "dd" and end of line is true, let new container name be "dt".
    if end_of_line && container_name == &local_name!("dd") {
        "dt".to_owned()
    } else {
        // Step 21. Otherwise, let new container name be the local name of container.
        container_name.to_string()
    };
    // Step 22. Let new container be the result of calling createElement(new container name) on the context object.
    let new_container = document.create_element(cx, &new_container_name);
    // Step 23. Copy all attributes of container to new container.
    container_as_element.copy_all_attributes_to_other_element(cx, &new_container);
    // Step 24. If new container has an id attribute, unset it.
    new_container.remove_attribute_by_name(cx, &local_name!("id"));
    // Step 25. Insert new container into the parent of container immediately after container.
    let new_container_node = DomRoot::upcast(new_container);
    if container
        .GetParentNode()
        .expect("Must always have a parent")
        .InsertBefore(
            cx,
            &new_container_node,
            container.GetNextSibling().as_deref(),
        )
        .is_err()
    {
        unreachable!("Must always be able to insert");
    }
    // Step 26. Let contained nodes be all nodes contained in new line range.
    let contained_nodes: Vec<DomRoot<Node>> = new_line_range
        .contained_nodes(cx.no_gc())
        .map(|node| node.as_rooted())
        .collect();
    // Step 27. Let frag be the result of calling extractContents() on new line range.
    let Ok(frag) = new_line_range.ExtractContents(cx) else {
        unreachable!("Must always be able to extract");
    };
    let frag_as_node = frag.upcast::<Node>();
    // Step 28. Unset the id attribute (if any) of each Element descendant of frag
    // that is not in contained nodes.
    for descendant in frag_as_node.traverse_preorder(ShadowIncluding::No) {
        if !contained_nodes.contains(&descendant) &&
            let Some(descendant) = descendant.downcast::<Element>()
        {
            descendant.remove_attribute_by_name(cx, &local_name!("id"));
        }
    }
    // Step 29. Call appendChild(frag) on new container.
    if new_container_node.AppendChild(cx, frag_as_node).is_err() {
        unreachable!("Must always be able to append");
    }
    // Step 30. While container's lastChild is a prohibited paragraph child,
    // set container to its lastChild.
    loop {
        let Some(last_child) = container.children().last() else {
            break;
        };
        if !last_child.is_prohibited_paragraph_child() {
            break;
        }
        container = last_child;
    }
    // Step 31. While new container's lastChild is a prohibited paragraph child,
    // set new container to its lastChild.
    let mut new_container_node = new_container_node;
    loop {
        let Some(last_child) = new_container_node.children().last() else {
            break;
        };
        if !last_child.is_prohibited_paragraph_child() {
            break;
        }
        new_container_node = last_child;
    }
    // Step 32. If container has no visible children,
    // call createElement("br") on the context object,
    // and append the result as the last child of container.
    if container
        .children()
        .all(|child| child.is_invisible(cx.no_gc()))
    {
        let br = document.create_element(cx, "br");
        if container.AppendChild(cx, br.upcast()).is_err() {
            unreachable!("Must always be able to append");
        }
    }
    // Step 33. If new container has no visible children,
    // call createElement("br") on the context object,
    // and append the result as the last child of new container.
    if new_container_node
        .children()
        .all(|child| child.is_invisible(cx.no_gc()))
    {
        let br = document.create_element(cx, "br");
        if new_container_node.AppendChild(cx, br.upcast()).is_err() {
            unreachable!("Must always be able to append");
        }
    }
    // Fork-autonomous wrap (WPT insertparagraph-with-white-space-style,
    // Chrome-91 anchored, REQ-BRW-002): splitting a display:inline container
    // duplicates the inline element, which does not read as a new paragraph
    // — the half that holds only the br placeholder (or the new half when
    // both halves have content) is additionally wrapped in a new block div,
    // so the paragraph break is a real block boundary.
    if container
        .downcast::<Element>()
        .and_then(Element::resolved_display_value)
        .is_some_and(|display| display == DisplayOutside::Inline)
    {
        let wrap_placeholder_half = container
            .children()
            .all(|child| child.is_invisible(cx.no_gc()));
        let target = if wrap_placeholder_half {
            container.clone()
        } else {
            new_container_node.clone()
        };
        let wrapper = document.create_element(cx, "div");
        let wrapper_node = DomRoot::upcast::<Node>(wrapper);
        if let Some(parent) = target.GetParentNode() &&
            parent
                .InsertBefore(cx, &wrapper_node, Some(&target))
                .is_ok()
        {
            let _ = wrapper_node.AppendChild(cx, &target);
        }
    }
    // Step 34. Call collapse(new container, 0) on the context object's selection.
    let _ = selection.Collapse(cx, Some(&new_container_node), 0);
    // Step 35. Return true.
    true
}
