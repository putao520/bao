/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use layout_api::{LayoutElement, LayoutNode};
use script::layout_dom::ServoLayoutNode;
use style::context::SharedStyleContext;
use style::counter_style::CounterStyle;
use style::properties::longhands::list_style_type::computed_value::T as ListStyleType;
use style::values::computed::Image;
use style::values::generics::counters::Content;
use stylo_atoms::atom;

use crate::context::LayoutContext;
use crate::counter;
use crate::dom_traversal::{
    NodeAndStyleInfo, PseudoElementContentItem, generate_pseudo_element_content,
};
use crate::replaced::ReplacedContents;

/// <https://drafts.csswg.org/css-lists/#content-property>
pub(crate) fn make_marker<'dom>(
    context: &LayoutContext,
    info: &NodeAndStyleInfo<'dom>,
) -> Option<(NodeAndStyleInfo<'dom>, Vec<PseudoElementContentItem>)> {
    let marker_info =
        info.with_pseudo_element(context, style::selector_parser::PseudoElement::Marker)?;
    let style = &marker_info.style;
    let list_style = style.get_list();

    // https://drafts.csswg.org/css-lists/#marker-image
    let marker_image = || match &list_style.list_style_image {
        Image::Url(url) => Some(vec![
            PseudoElementContentItem::Replaced(ReplacedContents::from_image_url(
                marker_info.node,
                context,
                url,
            )?),
            PseudoElementContentItem::Text(" ".into()),
        ]),
        // XXX: Non-None image types unimplemented.
        Image::ImageSet(..)
        | Image::Gradient(..)
        | Image::Image(..)
        | Image::CrossFade(..)
        | Image::PaintWorklet(..)
        | Image::None => None,
        Image::LightDark(..) => unreachable!("light-dark() should be disabled"),
    };

    let content = match &marker_info.style.get_counters().content {
        Content::Items(_) => generate_pseudo_element_content(&marker_info, context),
        Content::None => return None,
        Content::Normal => marker_image().or_else(|| {
            let value = list_item_counter_value(info.node, &context.style_context);
            Some(vec![PseudoElementContentItem::Text(marker_string(
                &list_style.list_style_type,
                value,
            )?)])
        })?,
    };

    Some((marker_info, content))
}

/// The value of the `list-item` counter at `node`.
///
/// <https://drafts.csswg.org/css-lists/#list-item-counter>: a list item
/// increments the `list-item` counter by one, and an ordered list resets it
/// to its start value. Because `counter-reset` and `counter-increment` are
/// not yet parsed by servo (they remain behind the `layout.unimplemented`
/// preference), the implicit HTML list semantics are computed structurally
/// here: the value of a list item is the start value of its list plus the
/// number of preceding list items in that list.
#[expect(unsafe_code)]
pub(crate) fn list_item_counter_value(
    node: ServoLayoutNode,
    style_context: &SharedStyleContext,
) -> i32 {
    let is_list_item = |node: ServoLayoutNode| {
        node.as_element().is_some_and(|element| {
            element
                .style(style_context)
                .get_box()
                .display
                .is_list_item()
        })
    };

    let Some(parent) = (unsafe { node.dangerous_dom_parent() }) else {
        return 1;
    };
    if !is_list_item(node) {
        // Not a list item: no `list-item` counter is incremented for it.
        return 0;
    }

    // Count the preceding list items in the same list.
    let mut preceding = 0;
    let mut total = 0;
    let mut reached_self = false;
    for child in parent.dom_children() {
        if !is_list_item(child) {
            continue;
        }
        total += 1;
        if child.opaque() == node.opaque() {
            reached_self = true;
        } else if !reached_self {
            preceding += 1;
        }
    }

    // `start="n"` sets the value of the first list item.
    let start = parent
        .as_element()
        .and_then(|element| {
            element
                .attribute_as_str(&web_atoms::ns!(), &web_atoms::local_name!("start"))
                .and_then(|value| value.trim().parse::<i32>().ok())
        })
        .unwrap_or(1);

    // `reversed` counts down from the start value, which defaults to the
    // number of list items in the list.
    let reversed = parent.as_element().is_some_and(|element| {
        element
            .attribute(&web_atoms::ns!(), &web_atoms::local_name!("reversed"))
            .is_some()
    });
    if reversed {
        let start = if start == 1 { total } else { start };
        start - preceding
    } else {
        start + preceding
    }
}

/// The `::marker` text for `list_style_type` at counter value `value`,
/// including the counter style's prefix and suffix.
///
/// <https://drafts.csswg.org/css-lists/#marker-string>
pub(crate) fn marker_string(list_style_type: &ListStyleType, value: i32) -> Option<String> {
    match &list_style_type.0 {
        // `list-style-type: none` generates no marker.
        CounterStyle::None => None,
        // A `<string>` is used verbatim, without a counter style suffix.
        CounterStyle::String(string) => Some(string.to_string()),
        style => counter::representation(style, value),
    }
}

/// The value of counter `name` at `node`, for the `counter()` function.
///
/// <https://drafts.csswg.org/css-lists/#counter-functions>: a counter that has
/// not been instantiated by a `counter-reset` in scope has the value zero.
/// `counter-reset`/`counter-increment` are still behind the
/// `layout.unimplemented` preference in servo, so `list-item` is the only
/// counter that can be non-zero; its value derives from the element's
/// position within its list.
pub(crate) fn counter_value(
    name: &stylo_atoms::Atom,
    node: ServoLayoutNode,
    style_context: &SharedStyleContext,
) -> i32 {
    if *name == atom!("list-item") {
        list_item_counter_value(node, style_context)
    } else {
        0
    }
}

/// The values of every `name` counter in scope at `node`, outermost first,
/// for the `counters()` function.
#[expect(unsafe_code)]
pub(crate) fn counter_chain(
    name: &stylo_atoms::Atom,
    node: ServoLayoutNode,
    style_context: &SharedStyleContext,
) -> Vec<i32> {
    if *name != atom!("list-item") {
        return Vec::new();
    }
    let mut chain = Vec::new();
    let mut current = Some(node);
    while let Some(node) = current {
        if node.as_element().is_some_and(|element| {
            element
                .style(style_context)
                .get_box()
                .display
                .is_list_item()
        }) {
            chain.push(list_item_counter_value(node, style_context));
        }
        current = unsafe { node.dangerous_dom_parent() };
    }
    chain.reverse();
    chain
}
