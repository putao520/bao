/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use html5ever::local_name;
use js::context::{JSContext, NoGC};
use script_bindings::codegen::GenericBindings::SelectionBinding::SelectionMethods;
use script_bindings::inheritance::Castable;
use style::values::specified::box_::DisplayOutside;

use crate::dom::Node;
use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::Bindings::RangeBinding::RangeMethods;
use crate::dom::bindings::codegen::Bindings::TextBinding::TextMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::comment::Comment;
use crate::dom::document::Document;
use crate::dom::element::Element;
use crate::dom::execcommand::basecommand::CommandName;
use crate::dom::execcommand::commands::insertlinebreak::execute_insert_line_break_command;
use crate::dom::execcommand::contenteditable::node::{
    NodeOrString, has_explicit_white_space_style, is_allowed_child, node_matches_local_name,
    split_the_parent, wrap_node_list,
};
use crate::dom::html::htmlanchorelement::HTMLAnchorElement;
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
    // Chrome-compat anchor (WPT editing/other/
    // insertparagraph-in-child-of-head.tentative.html, REQ-BRW-002): inside
    // a *visible* raw-text element — title, style, script, which cannot host
    // element children — the paragraph command inserts literal line feeds
    // instead of splitting the block: one at the caret, plus a second when
    // the caret's line would otherwise render empty. An invisible raw-text
    // element (an unstyled script renders display:none) keeps the block
    // splitting path (editing/run/insertparagraph.html
    // `<script>foo[]bar</script>baz` splits into two scripts).
    if let Some(active_range) = selection.active_range(cx) &&
        let Some(raw_text_host) = active_range.start_container().inclusive_ancestors(
            ShadowIncluding::No,
        ).find(|ancestor| {
            node_matches_local_name!(
                ancestor,
                local_name!("title") | local_name!("style") | local_name!("script")
            )
        }) &&
        !raw_text_host.is_invisible(cx.no_gc())
    {
        return insert_paragraph_line_feeds(cx, document, selection);
    }

    // The white-space-style domain gate (see has_explicit_white_space_style):
    // the e174 tentative-data semantics own every explicitly styled context,
    // so the gate is taken on the caret's surroundings before any mutation.
    let ws_styled = selection
        .active_range(cx)
        .is_some_and(|range| has_explicit_white_space_style(&range.start_container()));
    // Step 1. Delete the selection. Chrome-83 anchor (WPT editing/run/
    // insertparagraph.html `a&nbsp;[] b` family, REQ-BRW-002): with a
    // collapsed caret the deletion is empty — its only effect would be the
    // whitespace canonicalization at the caret, which rewrites runs that
    // Chrome leaves untouched across the paragraph split (the split-boundary
    // rebalance below owns those). A real selection still deletes, and the
    // white-space-style domain keeps the delete entirely.
    if ws_styled ||
        !selection
            .active_range(cx)
            .is_some_and(|range| range.collapsed())
    {
        selection.delete_the_selection(
            cx,
            document,
            Default::default(),
            Default::default(),
            Default::default(),
        );
    }
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
    // The white-space-style domain gate (see has_explicit_white_space_style):
    // the e174 tentative-data semantics own every explicitly styled context.
    let ws_styled = has_explicit_white_space_style(&node);
    // The caret's inline ancestors: a placeholder line carries clones of
    // them so the typing context survives (`<p><b>foo[]</b></p>` →
    // `<p><b><br></b></p>`). Collected before the boundary lift promotes the
    // caret out of the inline chain. Invisible shells (display:none, e.g. a
    // script) are not typing context, and an anchor cuts the chain entirely —
    // a new line after a link must not inherit the link or anything inside it
    // (`<p><a><b>[]foo</b></a></p>` → `<p><br></p>`).
    let inline_chain: Vec<DomRoot<Node>> = {
        let mut chain: Vec<DomRoot<Node>> = Vec::new();
        let mut cursor = node.clone();
        loop {
            // An anchor cuts the chain entirely — a new line after a link must
            // not inherit the link or anything inside it
            // (`<p><a><b>[]foo</b></a></p>` → `<p><br></p>`) — and so does any
            // replaced or embedded element: it is content, not a typing
            // context (non-splittable meter/progress/object/... keep their
            // own line intact).
            if cursor.is::<HTMLAnchorElement>() ||
                cursor
                    .downcast::<Element>()
                    .is_some_and(|element| {
                        matches!(
                            *element.local_name(),
                            local_name!("img") |
                                local_name!("br") |
                                local_name!("embed") |
                                local_name!("object") |
                                local_name!("iframe") |
                                local_name!("video") |
                                local_name!("audio") |
                                local_name!("canvas") |
                                local_name!("meter") |
                                local_name!("progress") |
                                local_name!("input") |
                                local_name!("select") |
                                local_name!("textarea") |
                                local_name!("hr")
                        )
                    })
            {
                chain.clear();
                break;
            }
            if cursor.is_inline_node() && !cursor.is_invisible(cx.no_gc()) {
                chain.push(cursor.clone());
            }
            let Some(parent) = cursor.GetParentNode() else {
                break;
            };
            if parent.is_block_node() ||
                parent.is_single_line_container() ||
                parent.is_editing_host()
            {
                break;
            }
            cursor = parent;
        }
        chain.reverse();
        chain
    };
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
            // Fork-autonomous rework (WPT editing/run/insertparagraph.html,
            // Chrome-83 anchored, REQ-BRW-002): instead of the spec's empty-
            // paragraph drop at the caret, Chromium's createBlock: the
            // caret's line is split into two halves, each wrapped in its own
            // default single-line container. The head half runs from the
            // nearest preceding br or block start to the caret; the tail half
            // runs from the caret to the block's end (line-terminating brs
            // included, following blocks excluded). An invisible-only tail —
            // collapsed trailing whitespace, an emptied inline shell — is not
            // a line of its own and stays with the head half, and a container
            // whose only content is a moved br takes it as the placeholder.
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
            // Chrome-83 flex/grid atomic items (editing/run/insertparagraph.html
            // display:flex family): the caret's line lives in the flex item —
            // an inline child of the flex or grid container — and the split is
            // a sibling split of that item with the container untouched
            // (`<div style=display:flex><span>a[]bc</span></div>` →
            // `<span>a</span><span>bc</span>`), not a paragraph pair inside the
            // container. The head clone takes the content before the caret;
            // the item keeps the tail; a side that would render nothing takes
            // a br placeholder.
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
            // The two half containers, created up front (the head element is
            // appended to only once the head half has been extracted).
            let head = document.create_element(cx, tag.str());
            let head_node = DomRoot::upcast::<Node>(head);
            // Tail half first: from the caret to the block's end, bounded
            // exclusively by the first following block child — following
            // blocks are not part of this line, line-terminating brs are.
            // The tail is taken before the head container is inserted, so the
            // walk never runs into it.
            let (tail_node, tail_offset) = (node.clone(), offset);
            let (end_node, end_offset) = if tail_node == block {
                // The caret sits directly in the block: the line ends at
                // the first block child after the offset.
                match block
                    .children()
                    .enumerate()
                    .skip(tail_offset as usize)
                    .find(|(_, child)| child.is_block_node())
                {
                    Some((index, _)) => (block.clone(), index as u32),
                    None => (block.clone(), block.len()),
                }
            } else {
                let mut boundary: Option<(DomRoot<Node>, u32)> = None;
                for following in
                    tail_node.following_nodes_unrooted(cx.no_gc(), &block, ShadowIncluding::No)
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
            // Step 11.5.2. Set container to the result of calling createElement(tag)
            // on the context object.
            let new_paragraph = document.create_element(cx, tag.str());
            let new_paragraph_node = DomRoot::upcast::<Node>(new_paragraph);
            let mut tail_moved = false;
            let tail_range = document.CreateRange(cx);
            let _ = tail_range.SetStart(cx.no_gc(), &tail_node, tail_offset);
            let _ = tail_range.SetEnd(cx.no_gc(), &end_node, end_offset);
            if !tail_range.collapsed() &&
                let Ok(frag) = tail_range.ExtractContents(cx)
            {
                let frag_node = frag.upcast::<Node>();
                // An invisible-only tail renders no line: trailing collapsed
                // whitespace or an emptied inline shell rides with the head
                // half (`abc[] ` keeps its plain trailing space on the head
                // line), while a tail with content moves and its leading
                // whitespace run takes the Chrome-83 non-breaking form at the
                // new line start via the rebalance below (`a[] b` → `&nbsp;b`).
                tail_moved = if ws_styled {
                    frag_node.children().count() > 0
                } else {
                    frag_node.children().any(|child| {
                        (child.is_visible(cx.no_gc()) ||
                            child.is::<HTMLBRElement>() ||
                            is_embedded_line_content(&child)) &&
                            has_non_whitespace_text(&child)
                    })
                };
                let host: &Node = if tail_moved {
                    &new_paragraph_node
                } else {
                    &head_node
                };
                let _ = host.AppendChild(cx, frag_node);
            }
            // Head half: from the line start to the caret. The extraction
            // splits the inline ancestors shared with the caret.
            let head_range = document.CreateRange(cx);
            let (head_start_node, head_start_offset) =
                line_start_boundary(&node, offset, &block);
            let _ = head_range.SetStart(cx.no_gc(), &head_start_node, head_start_offset);
            let _ = head_range.SetEnd(cx.no_gc(), &node, offset);
            if !head_range.collapsed() &&
                let Ok(frag) = head_range.ExtractContents(cx)
            {
                let _ = head_node.AppendChild(cx, frag.upcast::<Node>());
            }
            // The extraction leaves the caret inside truncated inline
            // ancestors: drop the shells it emptied, then lift to the block
            // level so the head container lands there instead of nesting
            // inside the split inline chain (`<b>foo[]bar</b>` → two divs
            // carrying <b> halves, not divs inside the original <b>).
            strip_emptied_inline_shells(cx, selection);
            let (mut junction_node, mut junction_offset) = selection.start_boundary(cx);
            while junction_node.is_inline_node() &&
                !junction_node.is_editing_host() &&
                let Some(parent) = junction_node.GetParentNode()
            {
                if junction_offset == 0 {
                    junction_offset = junction_node.index();
                } else if junction_offset == junction_node.len() {
                    junction_offset = 1 + junction_node.index();
                } else {
                    break;
                }
                junction_node = parent;
            }
            insert_node_at(cx, &junction_node, junction_offset, &head_node);
            if block_needs_placeholder(cx.no_gc(), &head_node) {
                append_placeholder(cx, document, &head_node, &[], ws_styled);
            }
            // Step 11.5.3. The tail container follows the head container.
            let tail_parent = head_node
                .GetParentNode()
                .expect("Head container must be in the tree");
            if tail_parent
                .InsertBefore(
                    cx,
                    &new_paragraph_node,
                    head_node.GetNextSibling().as_deref(),
                )
                .is_err()
            {
                unreachable!("Must always be able to insert");
            }
            // Step 11.5.4. A container whose content renders no line gets a
            // br placeholder; a moved br already is one.
            if block_needs_placeholder(cx.no_gc(), &new_paragraph_node) {
                append_placeholder(cx, document, &new_paragraph_node, &[], ws_styled);
            }
            // Chrome-83 split-boundary whitespace forms (same test family):
            // when content moved into the tail, the head's trailing collapsible
            // run becomes non-breaking at the new line end (`a []b` →
            // `a&nbsp;`) and the tail's leading run takes the Chrome-83
            // non-breaking-first form at the new line start (`a[] b` →
            // `&nbsp;b`). A whitespace-only tail that stayed behind leaves the
            // head untouched.
            if tail_moved && !ws_styled {
                if let Some((text, text_offset)) = deepest_last_text(&head_node) {
                    text.canonicalize_whitespace(cx, text_offset, false);
                }
                if let Some((text, text_offset)) = deepest_first_text(&new_paragraph_node) {
                    text.rebalance_whitespace_at(cx, text_offset);
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
    // "Contains" spans all contained nodes, not the common ancestor's element
    // children: a range that starts inside the caret's own text node contains
    // that text suffix, and the line is not over (Chrome-83:
    // `<dl><dt><p>[]foo</dl>` stays dt, only `<dl><dt><p>foo[]</dl>` flips to
    // dd).
    let end_of_line = if ws_styled {
        new_line_range
            .contained_children(cx.no_gc())
            .is_ok_and(|contained_children| {
                let contained_children = contained_children.contained_children;
                contained_children.is_empty() ||
                    (contained_children.len() == 1 &&
                        contained_children[0].is::<HTMLBRElement>())
            })
    } else {
        let contained: Vec<DomRoot<Node>> = new_line_range
            .contained_nodes(cx.no_gc())
            .map(|node| node.as_rooted())
            .collect();
        contained.is_empty() ||
            (contained.len() == 1 && contained[0].is::<HTMLBRElement>())
    };
    // Chrome-83 (editing/run/insertparagraph.html li/dt/dd inner-block
    // family): splitting inside a block nested in a list item keeps the list
    // structure — the new container is a sibling item of the same kind
    // carrying a clone of the inner block chain (`<ul><li><div>foo[]</ul>` →
    // `<li><div><br></div></li>`, `<dl><dt><p>[]foo</dl>` → `<dt><p>foo</p>
    // </dt>`), instead of the spec's bare item or the dt/dd name flip. The
    // extraction below targets the deepest clone.
    let mut inner_chain: Vec<DomRoot<Node>> = Vec::new();
    if !ws_styled &&
        node_matches_local_name!(
            container,
            local_name!("li") | local_name!("dt") | local_name!("dd")
        ) {
        let mut cursor = node.clone();
        loop {
            if *cursor == *container {
                break;
            }
            let Some(parent) = cursor.GetParentNode() else {
                break;
            };
            if cursor.is_block_node() {
                inner_chain.push(cursor.clone());
            }
            cursor = parent;
        }
        inner_chain.reverse();
    }
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
    // Chrome-83 li/dt/dd inner-block family: a tail that is empty or bare
    // inline content belongs inside a clone of the inner block chain
    // (`<ul><li><div>foo[]</ul>` → `<li><div><br></div></li>`), while a
    // fragment that already carries the inner block (the caret sat at the
    // block's start and the whole block moved) lands in the new container
    // unwrapped (`<dl><dt><p>[]foo</dl>` → `<dt><p>foo</p></dt>`).
    // Name-based, not resolved-display-based: the fragment's children are
    // detached, and a detached element resolves no block display.
    let frag_has_block = frag_as_node
        .children()
        .any(|child| child.is_prohibited_paragraph_child());
    let extraction_target: DomRoot<Node> = if inner_chain.is_empty() || frag_has_block {
        new_container_node.clone()
    } else {
        let mut host = new_container_node.clone();
        for link in inner_chain.iter() {
            let clone_node = link
                .CloneNode(cx, false)
                .expect("Must always be able to clone");
            if host.AppendChild(cx, &clone_node).is_err() {
                unreachable!("Must always be able to append");
            }
            host = clone_node;
        }
        host
    };
    // Chrome-83 tail rule (same anchor as the Step-11 branch): a tail that
    // renders no line — collapsed trailing whitespace, an emptied inline
    // shell — is not a line of its own and stays with the container
    // (`<div>abc[] </div>` keeps its trailing space on the container's line,
    // the new container takes a br placeholder). A tail with content moves,
    // and both sides of the split then take the Chrome-83 whitespace forms:
    // the container's trailing run becomes non-breaking at the new line end
    // (`<div>a []b</div>` → `a&nbsp;`), the new container's leading run takes
    // the non-breaking-first form at the new line start (`a[] b` → `&nbsp;b`).
    // A fragment member that is a paragraph-level element counts as moved even
    // when it renders nothing: the extraction of a caret at an inner block's
    // end clones that block empty (`<dl><dt><p>foo[]</dl>` → `<p></p>`), and
    // the empty clone belongs to the new container — the placeholder descent
    // then gives it the br (`<dd><p><br></p></dd>`). Detached members resolve
    // no display, so visibility alone cannot see them.
    let tail_moved = if ws_styled {
        frag_as_node.children().count() > 0
    } else {
        frag_as_node.children().any(|child| {
            (child.is_visible(cx.no_gc()) ||
                child.is::<HTMLBRElement>() ||
                is_embedded_line_content(&child) ||
                child.is_prohibited_paragraph_child()) && has_non_whitespace_text(&child)
        })
    };
    let tail_host: &Node = if tail_moved {
        &extraction_target
    } else {
        &container
    };
    // Step 29. Call appendChild(frag) on new container.
    if tail_host.AppendChild(cx, frag_as_node).is_err() {
        unreachable!("Must always be able to append");
    }
    if tail_moved && !ws_styled {
        if let Some((text, text_offset)) = deepest_last_text(&container) {
            text.canonicalize_whitespace(cx, text_offset, false);
        }
        if let Some((text, text_offset)) = deepest_first_text(&extraction_target) {
            text.rebalance_whitespace_at(cx, text_offset);
        }
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
    // A container whose only content is a moved br already renders a line —
    // doubling it would show two (Chrome-83: `foo[]<br>` → `<div><br></div>`).
    if block_needs_placeholder(cx.no_gc(), &container) {
        append_placeholder(cx, document, &container, &inline_chain, ws_styled);
    }
    // Step 33. If new container has no visible children,
    // call createElement("br") on the context object,
    // and append the result as the last child of new container.
    if block_needs_placeholder(cx.no_gc(), &new_container_node) {
        append_placeholder(cx, document, &new_container_node, &inline_chain, ws_styled);
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


/// A placeholder for a line that would render no content: the target's
/// whitespace-only text is dropped (it renders nothing and Chrome-83 lines
/// that carry only a caret keep only the br), and the caret's inline
/// ancestors are cloned around the br so the placeholder line keeps the
/// typing context (`<p><b>foo[]</b></p>` → `<p><b><br></b></p>`).
fn append_placeholder(
    cx: &mut JSContext,
    document: &Document,
    target: &Node,
    chain: &[DomRoot<Node>],
    ws_styled: bool,
) {
    if ws_styled {
        let br = document.create_element(cx, "br");
        if target.AppendChild(cx, br.upcast()).is_err() {
            unreachable!("Must always be able to append");
        }
        return;
    }
    let whitespace_only: Vec<DomRoot<Node>> = target
        .children()
        .filter(|child| {
            child.is::<Text>() && !has_non_whitespace_text(&child)
        })
        .collect();
    for child in whitespace_only {
        child.remove_self(cx);
    }
    let mut host = DomRoot::from_ref(target);
    for link in chain {
        let clone = link
            .CloneNode(cx, false)
            .expect("Must always be able to clone");
        if host.AppendChild(cx, &clone).is_err() {
            unreachable!("Must always be able to append");
        }
        host = clone;
    }
    let br = document.create_element(cx, "br");
    if host.AppendChild(cx, br.upcast()).is_err() {
        unreachable!("Must always be able to append");
    }
}

/// Whether any element between the caret and its editing host carries an
/// explicit `white-space` declaration in its `style` attribute. That is the
/// insertparagraph-with-white-space-style domain (Chrome-91 tentative data,
/// e174 anchor), where the split keeps the e174 semantics; everywhere else
/// the Chrome-83 split semantics apply (editing/run/insertparagraph.html).

/// Embedded or invisible line content that nonetheless belongs to a line as
/// its own element: the tentative void-element family splits before an
/// `embed`/`input`/`wbr` exactly as before an image.
fn is_embedded_line_content(node: &Node) -> bool {
    node.downcast::<Element>().is_some_and(|element| {
        matches!(
            *element.local_name(),
            local_name!("embed") | local_name!("input") | local_name!("wbr")
        )
    })
}

/// A Text node whose data is all whitespace renders no content of its own —
/// as a tail it is the line's trailing collapsed run, not a line of text.
fn has_non_whitespace_text(node: &Node) -> bool {
    !node
        .downcast::<Text>()
        .is_some_and(|text| text.data().chars().all(char::is_whitespace))
}

/// A container needs a br placeholder when its content would render no line:
/// every child invisible and none of them a br. A moved br already gives the
/// empty line its height, so it must not be doubled (Chrome-83: `foo[]<br>`
/// yields `<div><br></div>`, not `<div><br><br></div>`).
fn block_needs_placeholder(no_gc: &NoGC, node: &Node) -> bool {
    // The br clause is explicit and does not lean on is_visible: a br in a
    // container always renders a line here, regardless of the shared
    // extraneous-line-break approximation (Chrome-83: `foo[]<br>` →
    // `<div><br></div>`, never `<div><br><br></div>`).
    !node.children().any(|child| {
        child.is::<HTMLBRElement>() ||
            is_embedded_line_content(&child) ||
            child.is_visible(no_gc)
    })
}

/// The node just before the boundary point (node, offset) in tree order,
/// descending into descendants; None at the block start.
fn node_before(node: &Node, offset: u32, block: &Node) -> Option<DomRoot<Node>> {
    if offset > 0 {
        if let Some(child) = node.children().nth(offset as usize - 1) {
            let mut deepest = child;
            while let Some(last) = deepest.children().last() {
                deepest = last;
            }
            return Some(deepest);
        }
    }
    if *node == *block {
        return None;
    }
    // Climb to the previous position within the block.
    let mut current = DomRoot::from_ref(node);
    loop {
        if let Some(previous) = current.GetPreviousSibling() {
            let mut deepest = previous;
            while let Some(last) = deepest.children().last() {
                deepest = last;
            }
            return Some(deepest);
        }
        let Some(parent) = current.GetParentNode() else {
            return None;
        };
        if *parent == *block {
            return None;
        }
        current = parent;
    }
}

/// The boundary point where the caret's line starts: the position just after
/// the nearest preceding br or block node, walked in tree order within the
/// block. Invisible inline leftovers do not bound a line and are walked past
/// (they stay outside the head container).
fn line_start_boundary(node: &Node, offset: u32, block: &Node) -> (DomRoot<Node>, u32) {
    let mut current = node_before(node, offset, block);
    while let Some(previous) = current {
        if *previous == *block {
            return (DomRoot::from_ref(block), 0);
        }
        if previous.is::<HTMLBRElement>() || previous.is_block_node() {
            let parent = previous
                .GetParentNode()
                .expect("Must have a parent");
            return lift_out_of_inline(&parent, previous.index() + 1);
        }
        current = previous.preceding_nodes(block).next();
    }
    (DomRoot::from_ref(block), 0)
}

/// A position at an inline element's edge moves out to the equivalent
/// parent-level position, so block containers are never nested inside inline
/// shells. Mid-element positions are left alone (the extraction splits them).
fn lift_out_of_inline(node: &Node, offset: u32) -> (DomRoot<Node>, u32) {
    let mut node = DomRoot::from_ref(node);
    let mut offset = offset;
    while node.is_inline_node() &&
        !node.is_editing_host() &&
        let Some(parent) = node.GetParentNode()
    {
        if offset == 0 {
            offset = node.index();
        } else if offset == node.len() {
            offset = 1 + node.index();
        } else {
            break;
        }
        node = parent;
    }
    (node, offset)
}

fn insert_node_at(cx: &mut JSContext, node: &Node, offset: u32, new_node: &Node) {
    let next = node.children().nth(offset as usize);
    if node.InsertBefore(cx, new_node, next.as_deref()).is_err() {
        unreachable!("Must always be able to insert");
    }
}

/// Inline ancestors at the selection's start boundary that the head
/// extraction emptied are removed, so no empty inline leftover survives the
/// split (`<span>foo[]</span>` must not leave `<span></span>` behind).
fn strip_emptied_inline_shells(cx: &mut JSContext, selection: &Selection) {
    loop {
        let (node, _) = selection.start_boundary(cx);
        let Some(parent) = node.GetParentNode() else {
            return;
        };
        let empty_shell = !node.is::<Text>() &&
            node.is_inline_node() &&
            node.children().all(|child| child.is_invisible(cx.no_gc())) &&
            !node.children().any(|child| child.is::<HTMLBRElement>());
        if !empty_shell {
            return;
        }
        let index = node.index();
        node.remove_self(cx);
        let _ = selection.Collapse(cx, Some(&parent), index);
    }
}

/// The deepest last text descendant and its end offset, for boundary
/// canonicalization; None when the container ends with anything else.
fn deepest_last_text(node: &Node) -> Option<(DomRoot<Node>, u32)> {
    let mut deepest = DomRoot::from_ref(node);
    loop {
        match deepest.children().last() {
            Some(last) => deepest = last,
            None => break,
        }
    }
    deepest
        .downcast::<Text>()
        .map(|text| (DomRoot::from_ref(text.upcast()), text.upcast::<Node>().len()))
}

/// The deepest first text descendant and a zero offset, for boundary
/// rebalancing; None when the container starts with anything else.
fn deepest_first_text(node: &Node) -> Option<(DomRoot<Node>, u32)> {
    let mut deepest = DomRoot::from_ref(node);
    loop {
        match deepest.children().next() {
            Some(first) => deepest = first,
            None => break,
        }
    }
    deepest
        .downcast::<Text>()
        .map(|text| (DomRoot::from_ref(text.upcast()), 0))
}

/// Chrome-compat line-feed insertion for visible raw-text elements (see the
/// anchor comment on `execute_insert_paragraph_command`): insert a literal
/// `\n` Text at the caret, and a second one when the caret's line would
/// otherwise render empty (an empty host needs two feeds to show a line).
fn insert_paragraph_line_feeds(
    cx: &mut JSContext,
    document: &Document,
    selection: &Selection,
) -> bool {
    let line_feed = document.CreateTextNode(cx, DOMString::from_static("\n"));
    let line_feed_node = DomRoot::upcast::<Node>(line_feed);
    let mut active_range = selection
        .active_range(cx)
        .expect("Must always have an active range");
    if active_range.InsertNode(cx, &line_feed_node).is_err() {
        unreachable!("Must always be able to insert");
    }
    // Collapse inside the new text node, past the feed.
    let _ = selection.Collapse(cx, Some(&line_feed_node), line_feed_node.len());
    if line_feed_node.is_collapsed_line_break(cx.no_gc()) {
        let extra_feed = document.CreateTextNode(cx, DOMString::from_static("\n"));
        let extra_feed_node = DomRoot::upcast::<Node>(extra_feed);
        active_range = selection
            .active_range(cx)
            .expect("Must always have an active range");
        if active_range.InsertNode(cx, &extra_feed_node).is_err() {
            unreachable!("Must always be able to insert");
        }
        let _ = selection.Collapse(cx, Some(&extra_feed_node), extra_feed_node.len());
    }
    true
}
