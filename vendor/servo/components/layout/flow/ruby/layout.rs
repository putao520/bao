/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / Ruby W1a): pure ruby segment geometry.
//
// This module implements, as payload-free pure functions on [`Au`]:
//  - CSS Ruby §3.1.1 column width resolution for one ruby segment
//    (`separate` mode: per-column maxima; spanning annotations distribute
//    their excess evenly over the columns they span),
//  - CSS Ruby §4.2/§4.6 `ruby-align` inline-axis distribution,
//  - CSS Ruby §3.1.2 block-axis extents (zero-gap margin-box stacking of the
//    annotation levels around the base container),
//  - CSS Ruby §3.6 line contribution (grow the base ascent/descent by the
//    over/under extents; add leading when the container line height is
//    larger).
//
// It is kept free of layout types so that it can be exercised by the
// table-driven tests in `flow/ruby/tests/ruby_plan.rs`, next to the planner
// tests that cover the pairing side.

use std::ops::Range;

use app_units::Au;

/// The resolved block-side position of one annotation level (CSS Ruby §4.1
/// `ruby-position`). `alternate` is resolved by the caller into a concrete
/// `Over`/`Under` per level before it reaches this module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedRubyPosition {
    /// Stacked on the line-over side of the base container.
    Over,
    /// Stacked on the line-under side of the base container.
    Under,
    /// `inter-character` (§4.1). Its horizontal face resolves to the `over`
    /// side (§4.1 gives vertical-writing inter-character the same side, and
    /// its own horizontal geometry is a later wave).
    InterCharacter,
}

/// The inline-axis distribution of a base or annotation unit inside its
/// column (CSS Ruby §4.2 `ruby-align`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RubyAlignKind {
    /// Content flush to the inline-start of the column.
    Start,
    /// Content centered in the column.
    Center,
    /// Justify, and center if there is no justification opportunity.
    SpaceBetween,
    /// Half the extra space before, justify the rest; center if there is no
    /// justification opportunity.
    SpaceAround,
}

/// One spanning cell of a measured annotation level: the half-open range of
/// base column indexes it covers and its measured inline size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanningMeasure {
    pub span: Range<usize>,
    pub width: Au,
}

/// The inline-axis measurements of one annotation level of a segment, in
/// column space (§3.1.1). Cells that are hidden or absent are `None`: they
/// take no part in column width resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LevelMeasure {
    /// The measured width of the single-column cell covering each column;
    /// `None` where this level has no cell for the column.
    pub cells: Vec<Option<Au>>,
    /// The spanning cells of this level. Resolution processes them from the
    /// fewest spanned columns to the most (§3.1.1), regardless of input
    /// order.
    pub spans: Vec<SpanningMeasure>,
}

/// The resolved column widths of one ruby segment (§3.1.1, `separate` mode).
///
/// Single-column cells raise their column to the maximum of the column so
/// far. A spanning cell wider than the sum of its spanned columns distributes
/// the excess evenly over them, with any remainder going to the leading
/// columns of the span; wider spans are processed first so that a short span
/// that already stretched its columns reduces the excess a longer span sees.
/// Hidden cells (`None`) are skipped: they claim no width.
pub fn segment_column_widths(bases: &[Au], levels: &[LevelMeasure]) -> Vec<Au> {
    let mut columns: Vec<i32> = bases.iter().map(|width| width.0).collect();
    for level in levels {
        for (column, cell) in columns.iter_mut().zip(level.cells.iter()) {
            if let Some(cell_width) = cell {
                *column = (*column).max(cell_width.0);
            }
        }
        let mut spans: Vec<&SpanningMeasure> = level.spans.iter().collect();
        spans.sort_by_key(|spanning| spanning.span.len());
        for spanning in spans {
            let range = &spanning.span;
            // Out-of-segment or empty spans have no columns to stretch
            // (defensive; pairing produces in-range spans).
            if range.start >= range.end || range.end > columns.len() {
                continue;
            }
            let spanned_sum: i32 = columns[range.start..range.end].iter().sum();
            let excess = spanning.width.0 - spanned_sum;
            if excess <= 0 {
                continue;
            }
            let count = range.len() as i32;
            let share = excess / count;
            let remainder = excess % count;
            for (offset, column) in columns[range.start..range.end].iter_mut().enumerate() {
                *column += share + if (offset as i32) < remainder { 1 } else { 0 };
            }
        }
    }
    columns.into_iter().map(Au::new).collect()
}

/// The result of the §4.6 inline-axis distribution inside one column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlignedDistribution {
    /// The inline-start offset of the content inside the column.
    pub start_offset: Au,
    /// The total extra space to distribute across the content's justification
    /// opportunities (zero when the alignment justifies nothing).
    pub justify_extra: Au,
}

/// Distribute the free space of one column according to `ruby-align`
/// (§4.2, per the §4.6 table). `extra` is the free space (column width minus
/// content width; callers guarantee it is not negative) and
/// `justification_opportunities` is the number of expansion opportunities of
/// the content.
///
/// The resulting `justify_extra` is the total inserted into the
/// opportunities; the caller inserts the last share after the content when
/// there are no opportunities (space-around's trailing half).
pub fn align_distribution(
    align: RubyAlignKind,
    extra: Au,
    justification_opportunities: usize,
) -> AlignedDistribution {
    let extra = extra.0;
    let opportunities = justification_opportunities as i32;
    let (start_offset, justify_extra) = match align {
        RubyAlignKind::Start => (0, 0),
        RubyAlignKind::Center => (extra / 2, 0),
        RubyAlignKind::SpaceBetween => {
            if opportunities == 0 {
                // No justification opportunity: fall back to center (§4.2).
                (extra / 2, 0)
            } else {
                (0, extra / opportunities)
            }
        },
        RubyAlignKind::SpaceAround => {
            if opportunities == 0 {
                (extra / 2, extra / 2)
            } else {
                (extra / 2, (extra / 2) / opportunities)
            }
        },
    };
    AlignedDistribution {
        start_offset: Au::new(start_offset),
        justify_extra: Au::new(justify_extra),
    }
}

/// The block-axis extents a ruby adds around its base container (§3.1.2).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RubyExtents {
    /// Total margin-box height stacked on the line-over side of the base
    /// container.
    pub over: Au,
    /// Total margin-box height stacked on the line-under side of the base
    /// container.
    pub under: Au,
}

/// One annotation level's block-axis contribution: its resolved position and
/// the margin-box height of its container (content plus padding, border and
/// margin — §3.3 gives annotation containers their PBM back).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnnotationLevelExtents {
    pub position: ResolvedRubyPosition,
    pub margin_box: Au,
}

/// Stack the annotation levels around the base container (§3.1.2): the
/// containers abut the base container and each other with zero gaps, so the
/// extent on each side is the plain sum of that side's margin boxes.
pub fn block_extents(levels: &[AnnotationLevelExtents]) -> RubyExtents {
    let mut over = 0;
    let mut under = 0;
    for level in levels {
        match level.position {
            ResolvedRubyPosition::Under => under += level.margin_box.0,
            ResolvedRubyPosition::Over | ResolvedRubyPosition::InterCharacter => {
                over += level.margin_box.0
            },
        }
    }
    RubyExtents {
        over: Au::new(over),
        under: Au::new(under),
    }
}

/// The ruby's contribution to the line box that carries it (§3.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RubyLineMetrics {
    /// Baseline-to-top distance: the base ascent grown by the `over` extent.
    pub ascent: Au,
    /// Baseline-to-bottom distance: the base descent grown by the `under`
    /// extent.
    pub descent: Au,
    /// The used line height: the container's line height, raised to the full
    /// ruby extent when the extent is larger (§3.6 minimum leading).
    pub line_height: Au,
    /// The leading added on top of the ruby extent (zero unless the extent
    /// is smaller than the container's line height).
    pub leading: Au,
}

/// Compute the line contribution of one ruby (§3.6): annotation containers
/// and annotations take no part in the measured height of the inline content,
/// so the line is computed from the base container as a normal inline box and
/// then grown by the over/under extents. If the ruby container's line height
/// is smaller than the distance from the topmost annotation edge to the
/// bottommost annotation edge, leading is added so that adjacent ruby lines
/// do not overlap; annotations that still overflow the line box are the
/// author's problem.
pub fn line_contribution(
    base_ascent: Au,
    base_descent: Au,
    extents: &RubyExtents,
    container_line_height: Au,
) -> RubyLineMetrics {
    let ascent = base_ascent.0 + extents.over.0;
    let descent = base_descent.0 + extents.under.0;
    let ruby_height = ascent + descent;
    let line_height = container_line_height.0.max(ruby_height);
    RubyLineMetrics {
        ascent: Au::new(ascent),
        descent: Au::new(descent),
        line_height: Au::new(line_height),
        leading: Au::new(line_height - ruby_height),
    }
}
