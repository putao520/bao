/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Writing Modes support.
//!
//! @trace REQ-BRW-051
//!
//! This module is the coordinate-transform layer between the flow-relative
//! (logical) coordinate space that the Servo layout engine works in and the
//! physical space used to place fragments and paint them. Upstream Servo has
//! no vertical layout engine; block and inline layout here are written in
//! logical coordinates already, so vertical writing modes are implemented by
//! completing the logical-to-physical mapping instead of rewriting layout:
//!
//! 1. `crate::geom` maps logical rectangles, sizes, vectors and points into
//!    physical space honoring both the block flow direction
//!    (`vertical-rl` stacks blocks right-to-left) and the inline base
//!    direction (`direction: rtl` and vertical bottom-to-top flows).
//! 2. [`crate::flow::BoxTree::layout`] maps the physical viewport size into
//!    the document's logical coordinate space using the root element's
//!    writing mode (<https://drafts.csswg.org/css-writing-modes/#principal-flow>).
//! 3. Orthogonal flows (a box whose writing mode is perpendicular to its
//!    containing block) get their available space through the physical
//!    mapping in [`containing_block_size_for_child`]
//!    (<https://drafts.csswg.org/css-writing-modes/#orthogonal-flows>).
//! 4. Shaped text is always laid out horizontally. For vertical flows the
//!    paint process rotates the text run into the inline direction (see
//!    [`TextOrientation`] and `display_list`).
//!
//! Everything in this module is pure geometry; no layout state is stored.

use app_units::Au;
use style::logical_geometry::WritingMode;
use style::properties::ComputedValues;

use crate::geom::LogicalVec2;

/// The orientation used when painting a shaped (horizontal) text run.
///
/// Text shaping always happens along the physical horizontal axis; vertical
/// writing modes are obtained by rotating the painted glyphs.
/// <https://drafts.csswg.org/css-writing-modes/#text-orientation>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextOrientation {
    /// Glyphs are painted in their natural orientation, advancing left to right.
    Horizontal,
    /// Glyphs are painted rotated 90° clockwise, advancing top to bottom. This is
    /// `text-orientation: mixed` (or `sideways`) in `vertical-rl`/`vertical-lr`.
    SidewaysClockwise,
    /// Glyphs are painted rotated 90° counter-clockwise, advancing bottom to top.
    /// Only reachable for `sideways-lr`, which Servo's Stylo configuration cannot
    /// parse (`WritingModeProperty::SidewaysLr` is Gecko-only), but kept so that
    /// the mapping is complete once that value becomes available.
    SidewaysCounterClockwise,
    /// Glyphs are painted upright (not rotated), stacked in the inline
    /// direction (`text-orientation: upright`).
    Upright,
}

/// Return the orientation used to paint text with the given `writing-mode`,
/// `direction` and `text-orientation`.
///
/// Note that `text-orientation` is `layout.unimplemented` in Servo's Stylo
/// configuration, so the value is always `mixed` today; the `upright` mapping
/// is still derived correctly from the computed `WritingMode` bitflags because
/// Stylo already encodes `text-orientation` there.
pub(crate) fn text_orientation(writing_mode: WritingMode) -> TextOrientation {
    if !writing_mode.is_vertical() {
        return TextOrientation::Horizontal;
    }
    if writing_mode.is_upright() {
        return TextOrientation::Upright;
    }
    if writing_mode.is_sideways() && writing_mode.is_vertical_lr() {
        // `sideways-lr` runs bottom-to-top with glyphs rotated counter-clockwise.
        return TextOrientation::SidewaysCounterClockwise;
    }
    TextOrientation::SidewaysClockwise
}

pub(crate) fn text_orientation_for_style(style: &ComputedValues) -> TextOrientation {
    text_orientation(style.writing_mode)
}

/// Whether the writing mode of `style` is perpendicular to `containing_block_style`.
///
/// <https://drafts.csswg.org/css-writing-modes/#orthogonal-flows>
pub(crate) fn is_orthogonal_flow(
    style: &ComputedValues,
    containing_block_style: &ComputedValues,
) -> bool {
    style.writing_mode.is_vertical() != containing_block_style.writing_mode.is_vertical()
}

/// Map a containing block size (expressed in the containing block's logical
/// coordinate space) into the logical coordinate space of a child box.
///
/// For a child with the same orientation as its containing block this is the
/// identity. For an orthogonal flow the size is mapped through physical space,
/// which swaps the inline and block axes: the inline size available to an
/// orthogonal child is the containing block's block size.
///
/// <https://drafts.csswg.org/css-writing-modes/#orthogonal-flows>
pub(crate) fn containing_block_size_for_child(
    containing_block_size: LogicalVec2<Au>,
    containing_block_writing_mode: WritingMode,
    child_writing_mode: WritingMode,
) -> LogicalVec2<Au> {
    if containing_block_writing_mode.is_vertical() == child_writing_mode.is_vertical() {
        return containing_block_size;
    }

    let physical = containing_block_size.to_physical_size(containing_block_writing_mode);
    LogicalVec2::from_physical_size(&physical, child_writing_mode)
}

/// The physical rotation (in radians, clockwise-positive in screen coordinates)
/// to apply to a horizontal text run so that it paints in the inline direction
/// of `writing_mode`, or `None` when no rotation is needed.
///
/// This is used by the display list builder, which paints the glyphs in a
/// reference frame rotated by this angle around the anchor given by
/// [`text_rotation_anchor`].
pub(crate) fn text_rotation_radians(writing_mode: WritingMode) -> Option<f32> {
    match text_orientation(writing_mode) {
        TextOrientation::Horizontal | TextOrientation::Upright => None,
        TextOrientation::SidewaysClockwise => Some(std::f32::consts::FRAC_PI_2),
        TextOrientation::SidewaysCounterClockwise => Some(-std::f32::consts::FRAC_PI_2),
    }
}

/// Whether the given writing mode rotates glyphs when painting text.
pub(crate) fn text_is_rotated(writing_mode: WritingMode) -> bool {
    text_rotation_radians(writing_mode).is_some()
}
