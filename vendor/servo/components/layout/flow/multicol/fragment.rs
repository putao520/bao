/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / C2): CSS Multi-column layout — fragment-level
// fragmentation of the column flow.
//
// Servo's block layout is a single-pass engine: a block container is laid out once
// and produces one fragment tree, with no notion of a fragmentation context. Column
// fragmentation is therefore expressed as a transformation of that fragment tree:
//
//  1. the column flow is laid out at the used column inline size with an indefinite
//     block size, which produces exactly the same line breaks and box sizes as a
//     fragmented layout would (inline layout only depends on the inline size, and
//     block fragmentation never changes the block size of an unbreakable piece);
//  2. [`collect_break_offsets`] derives the block offsets at which the flow may be
//     broken from the resulting fragment tree — before and after every sibling
//     block-level box, around monolithic content, and between the line boxes of
//     every inline formatting context;
//  3. `columns::plan_filled` / `columns::plan_balanced` choose the actual column
//     slices from those offsets;
//  4. [`take_fragment_range`] cuts one column's worth of fragments out of the flow,
//     recursively slicing the boxes that straddle the column boundary.
//
// Cuts only ever happen at collected break opportunities, so monolithic fragments
// (replaced elements, scroll containers, floats, table wrappers, single line boxes,
// …) are never sliced: when one does not fit, it overflows the column, as required
// by <https://drafts.csswg.org/css-multicol-1/#fragmentation>.
//
// Rect mutation note: fragments are positioned with interior mutability
// (`BaseFragment::translate_rect`) and fragment payloads are shared through `Arc`
// (both with the parent box's fragment cache and between the slices produced here).
// Because every shared fragment ends up in exactly one position of the final
// fragment tree, translating a shared fragment is the same operation that
// `PlacementState::place_fragment` performs for preserved fragments during
// incremental layout.

use std::sync::Arc;

use app_units::Au;
use style::Zero;
use style::logical_geometry::WritingMode;

use crate::formatting_contexts::Baselines;
use crate::fragment_tree::{BaseFragmentInfo, BoxFragment, Fragment, PositioningFragment};
use crate::geom::{LogicalVec2, PhysicalRect};
use crate::style_ext::{ComputedValuesExt, Display, DisplayInside};

/// The block-axis interval of a physical rectangle, in the block coordinates of the
/// column flow.
///
/// Fragments are positioned in physical coordinates, so the block axis has to be
/// recovered from the writing mode. Multi-column layout is currently restricted to
/// horizontal writing modes (see `MultiColContainer::is_multicol_style`), where the
/// block axis runs top-to-bottom; the vertical cases are handled for completeness
/// and mirror `PlacementState::place_fragment`.
pub(crate) fn block_interval(rect: &PhysicalRect<Au>, writing_mode: WritingMode) -> (Au, Au) {
    if writing_mode.is_horizontal() {
        (rect.origin.y, rect.origin.y + rect.size.height)
    } else if writing_mode.is_vertical_rl() {
        // `vertical-rl` stacks blocks from right to left, so the block offset is the
        // negated physical x coordinate.
        (-rect.origin.x, -rect.origin.x + rect.size.width)
    } else {
        (rect.origin.x, rect.origin.x + rect.size.width)
    }
}

/// The block extent of a fragment in its parent's coordinate space, if it has one.
/// Fragments standing in for absolutely positioned boxes are bare placeholders and
/// have no geometry of their own.
fn fragment_block_range(fragment: &Fragment, writing_mode: WritingMode) -> Option<(Au, Au)> {
    let base = fragment.base()?;
    Some(block_interval(&base.rect(), writing_mode))
}

/// Whether breaks are allowed *inside* this box fragment.
///
/// A box whose children are not laid out in the block flow (flex and grid
/// containers, table wrappers), a replaced box, and a scroll container are
/// monolithic. Note that the box's own borders and padding are sliced regardless;
/// what this predicate governs is only whether its content can be split.
fn is_splittable_box(box_fragment: &BoxFragment) -> bool {
    if box_fragment.is_replaced() || box_fragment.is_table_wrapper() {
        return false;
    }

    let style = box_fragment.style();
    if style.establishes_scroll_container(box_fragment.base.flags) {
        return false;
    }

    // A nested multi-column container is monolithic: its column boxes are laid out
    // side by side and therefore overlap in the block axis, so breaking between them
    // is meaningless.
    if style.get_column().is_multicol() {
        return false;
    }

    match Display::from(style.get_box().display) {
        Display::None | Display::Contents => false,
        Display::GeneratingBox(display) => matches!(
            display.display_inside(),
            DisplayInside::Flow { .. } | DisplayInside::FlowRoot { .. }
        ),
    }
}

/// The children of a fragment if it is a fragmentation container, i.e. if breaks
/// are allowed between its children.
fn splittable_children(fragment: &Fragment) -> Option<&[Fragment]> {
    match fragment {
        Fragment::Box(box_fragment) if is_splittable_box(box_fragment) => {
            Some(&box_fragment.children)
        },
        // Anonymous non-painting containers (the column boxes of a nested
        // multi-column container, a button's aligned content, …) are transparent for
        // fragmentation purposes. Line boxes are not: a line box is atomic.
        Fragment::Positioning(positioning_fragment) if !positioning_fragment.is_line_box() => {
            Some(&positioning_fragment.children)
        },
        _ => None,
    }
}

/// Collect the block offsets at which the column flow may be broken.
///
/// `origin` is the block offset of the fragments' coordinate space within the flow
/// (the block offset of the container that holds `fragments`). `out` is appended
/// to; the caller is responsible for sorting and deduplicating the result.
///
/// Every fragment contributes a break opportunity before and after itself, plus —
/// when it is a fragmentation container — the opportunities of its children. The
/// result is in flow block coordinates.
pub(crate) fn collect_break_offsets(
    fragments: &[Fragment],
    origin: Au,
    writing_mode: WritingMode,
    out: &mut Vec<Au>,
) {
    for fragment in fragments {
        let Some((block_start, block_end)) = fragment_block_range(fragment, writing_mode) else {
            continue;
        };
        let block_start = origin + block_start;
        let block_end = origin + block_end;
        out.push(block_start);
        if let Some(children) = splittable_children(fragment) {
            collect_break_offsets(children, block_start, writing_mode, out);
        }
        out.push(block_end);
    }
}

/// The block end of the last top-level fragment in the flow, or zero if the flow is
/// empty.
///
/// This is the block extent that actually has to be covered by columns. It can be
/// smaller than the flow's content block size, which also accounts for trailing
/// (possibly collapsed-through) margins: those must not create a trailing, empty
/// column.
pub(crate) fn flow_block_end(fragments: &[Fragment], writing_mode: WritingMode) -> Au {
    fragments
        .iter()
        .filter_map(|fragment| fragment_block_range(fragment, writing_mode))
        .map(|(_, block_end)| block_end)
        .max()
        .unwrap_or_else(Au::zero)
}

/// Remove the fragments that belong to the block range `[start, end)` from
/// `fragments` and return them.
///
/// The returned fragments keep the flow block coordinates of the input. Keeping the
/// coordinates means that the column container can be placed with a single offset
/// instead of translating every fragment (see `MultiColContainer::place_column`).
/// Fragments that end before `start` have already been consumed by an earlier column
/// and are dropped.
pub(crate) fn take_fragment_range(
    fragments: &mut Vec<Fragment>,
    start: Au,
    end: Au,
    writing_mode: WritingMode,
) -> Vec<Fragment> {
    let mut taken = Vec::new();
    let mut kept = Vec::with_capacity(fragments.len());
    for fragment in std::mem::take(fragments) {
        let (body, tail) = clip_fragment(fragment, start, end, writing_mode);
        if let Some(body) = body {
            taken.push(body);
        }
        if let Some(tail) = tail {
            kept.push(tail);
        }
    }
    *fragments = kept;
    taken
}

/// Split one fragment into the portion that belongs to the block range
/// `[start, end)` and the portion that belongs to the following columns.
///
/// A `None` body means that the fragment does not contribute to this column; a
/// `None` tail means that nothing of it is left for the later columns.
fn clip_fragment(
    fragment: Fragment,
    start: Au,
    end: Au,
    writing_mode: WritingMode,
) -> (Option<Fragment>, Option<Fragment>) {
    let Some((block_start, block_end)) = fragment_block_range(&fragment, writing_mode) else {
        // Absolutely positioned boxes only leave a placeholder in the flow; it has
        // no geometry of its own and travels with the column being built.
        return (Some(fragment), None);
    };

    if block_end <= start {
        // Consumed by an earlier column.
        return (None, None);
    }

    if block_start >= end {
        // Belongs to a later column.
        return (None, Some(fragment));
    }

    if block_start >= start && block_end <= end {
        // Entirely inside this column.
        return (Some(fragment), None);
    }

    // The fragment straddles a column boundary. Only fragmentation containers can
    // get here, because cuts only ever happen at collected break opportunities.
    match fragment {
        Fragment::Box(box_fragment) if is_splittable_box(&box_fragment) => clip_box_fragment(
            &box_fragment,
            block_start,
            block_end,
            start,
            end,
            writing_mode,
        ),
        Fragment::Positioning(positioning_fragment) if !positioning_fragment.is_line_box() => {
            clip_positioning_fragment(
                &positioning_fragment,
                block_start,
                block_end,
                start,
                end,
                writing_mode,
            )
        },
        // Monolithic content that somehow straddles a cut (only possible with
        // negative margins): keep it whole on the side that holds its block start.
        other => (Some(other), None),
    }
}

/// Slice a box fragment that straddles a column boundary, recursively slicing its
/// content.
///
/// The caller guarantees `block_start < end` and `block_end > start`, and that the
/// box is splittable. The body fragment's content rect starts at
/// `max(block_start, start)`; its children are rebased so that the first one starts
/// at zero in the body's content coordinate space. The tail is rebased the same way
/// starting at `end`.
fn clip_box_fragment(
    box_fragment: &Arc<BoxFragment>,
    block_start: Au,
    block_end: Au,
    start: Au,
    end: Au,
    writing_mode: WritingMode,
) -> (Option<Fragment>, Option<Fragment>) {
    let body_start = block_start.max(start);
    let body_end = block_end.min(end);

    let mut body_children = Vec::new();
    let mut tail_children = Vec::new();
    for child in &box_fragment.children {
        let (child_body, child_tail) = clip_fragment(
            shallow_clone(child),
            start - block_start,
            end - block_start,
            writing_mode,
        );
        if let Some(child_body) = child_body {
            body_children.push(child_body);
        }
        if let Some(child_tail) = child_tail {
            tail_children.push(child_tail);
        }
    }

    // Rebase the children: the body's content now starts at `body_start` (in the
    // parent's coordinate space) instead of at `block_start`.
    translate_block(body_children.iter(), block_start - body_start, writing_mode);

    let body = rebuild_box_fragment(
        box_fragment,
        body_children,
        block_start,
        body_start,
        body_end,
        writing_mode,
    );

    let tail = if block_end > end {
        translate_block(tail_children.iter(), block_start - end, writing_mode);
        Some(rebuild_box_fragment(
            box_fragment,
            tail_children,
            block_start,
            end,
            block_end,
            writing_mode,
        ))
    } else {
        None
    };

    (Some(Fragment::Box(body)), tail.map(Fragment::Box))
}

/// Slice an anonymous non-painting container (`Fragment::Positioning`) that
/// straddles a column boundary.
fn clip_positioning_fragment(
    fragment: &Arc<PositioningFragment>,
    block_start: Au,
    block_end: Au,
    start: Au,
    end: Au,
    writing_mode: WritingMode,
) -> (Option<Fragment>, Option<Fragment>) {
    let body_start = block_start.max(start);
    let body_end = block_end.min(end);

    let mut body_children = Vec::new();
    let mut tail_children = Vec::new();
    for child in &fragment.children {
        let (child_body, child_tail) = clip_fragment(
            shallow_clone(child),
            start - block_start,
            end - block_start,
            writing_mode,
        );
        if let Some(child_body) = child_body {
            body_children.push(child_body);
        }
        if let Some(child_tail) = child_tail {
            tail_children.push(child_tail);
        }
    }

    translate_block(body_children.iter(), block_start - body_start, writing_mode);

    let style = fragment.style.borrow().clone();
    let body = PositioningFragment::new_anonymous(
        style.clone(),
        clipped_rect(&fragment.base.rect(), body_start, body_end, writing_mode),
        body_children,
        false,
    );

    let tail = if block_end > end {
        translate_block(tail_children.iter(), block_start - end, writing_mode);
        Some(PositioningFragment::new_anonymous(
            style,
            clipped_rect(&fragment.base.rect(), end, block_end, writing_mode),
            tail_children,
            false,
        ))
    } else {
        None
    };

    (
        Some(Fragment::Positioning(body)),
        tail.map(Fragment::Positioning),
    )
}

/// Rebuild a box fragment with a new set of children and a content rect clipped to
/// `[clip_start, clip_end)`.
///
/// `block_start` is the block start of the original content rect in the parent's
/// coordinate space; `clip_start == block_start` means that nothing was removed at
/// the block start of the box, so the original baselines still apply.
fn rebuild_box_fragment(
    original: &Arc<BoxFragment>,
    children: Vec<Fragment>,
    block_start: Au,
    clip_start: Au,
    clip_end: Au,
    writing_mode: WritingMode,
) -> Arc<BoxFragment> {
    let baselines = if clip_start == block_start {
        original.baselines(writing_mode)
    } else {
        Baselines::default()
    };

    let mut fragment = BoxFragment::new(
        BaseFragmentInfo {
            tag: original.base.tag,
            flags: original.base.flags,
        },
        original.style.borrow().clone(),
        children,
        clipped_rect(&original.base.rect(), clip_start, clip_end, writing_mode),
        original.padding,
        original.border,
        original.margin,
        original.specific_layout_info().map(|info| (*info).clone()),
    );
    fragment.background_mode = original.background_mode.clone();
    fragment.block_level_layout_info = original.block_level_layout_info.clone();
    Arc::new(fragment.with_baselines(baselines))
}

/// The physical rectangle obtained by restricting `rect`'s block axis to
/// `[clip_start, clip_end)`.
fn clipped_rect(
    rect: &PhysicalRect<Au>,
    clip_start: Au,
    clip_end: Au,
    writing_mode: WritingMode,
) -> PhysicalRect<Au> {
    let mut rect = *rect;
    let size = (clip_end - clip_start).max(Au::zero());
    if writing_mode.is_horizontal() {
        rect.origin.y = clip_start;
        rect.size.height = size;
    } else if writing_mode.is_vertical_rl() {
        // Block offset `b` maps to `x = -b`, so the fragment spans
        // `[-clip_end, -clip_start)`.
        rect.origin.x = -clip_start - size;
        rect.size.width = size;
    } else {
        rect.origin.x = clip_start;
        rect.size.width = size;
    }
    rect
}

/// Translate every fragment in `fragments` by `block_delta` (a logical block offset)
/// in place.
fn translate_block<'a>(
    fragments: impl Iterator<Item = &'a Fragment>,
    block_delta: Au,
    writing_mode: WritingMode,
) {
    if block_delta == Au::zero() {
        return;
    }
    let physical_delta = LogicalVec2 {
        inline: Au::zero(),
        block: block_delta,
    }
    .to_physical_translation(writing_mode)
    .to_size();
    for fragment in fragments {
        if let Some(base) = fragment.base() {
            base.translate_rect(physical_delta);
        }
    }
}

/// Clone a fragment without cloning its payload.
///
/// All [`Fragment`] variants hold their payload behind an `Arc`, and payloads are
/// immutable once laid out, so sharing them between the slices of a fragmented flow
/// is sound.
fn shallow_clone(fragment: &Fragment) -> Fragment {
    match fragment {
        Fragment::LayoutRoot(fragment) => Fragment::LayoutRoot(fragment.clone()),
        Fragment::Box(fragment) => Fragment::Box(fragment.clone()),
        Fragment::Float(fragment) => Fragment::Float(fragment.clone()),
        Fragment::Positioning(fragment) => Fragment::Positioning(fragment.clone()),
        Fragment::Text(fragment) => Fragment::Text(fragment.clone()),
        Fragment::Image(fragment) => Fragment::Image(fragment.clone()),
        Fragment::IFrame(fragment) => Fragment::IFrame(fragment.clone()),
        Fragment::AbsoluteOrFixedPositionedPlaceholder(fragment) => {
            Fragment::AbsoluteOrFixedPositionedPlaceholder(fragment.clone())
        },
    }
}
