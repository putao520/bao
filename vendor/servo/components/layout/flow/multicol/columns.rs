/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / C2): CSS Multi-column layout — pure column
// geometry and column balancing.
//
// This module is deliberately free of layout types (`Fragment`, `ContainingBlock`,
// styles, …) so that the whole column-sizing and column-balancing surface can be
// exercised by the table-driven tests in `flow/multicol/tests/multicol_columns.rs`.
// The style reading and the fragment-level fragmentation live in
// `flow/multicol/container.rs` and `flow/multicol/fragment.rs` respectively.
//
// Specification references:
//  - column width / count: <https://drafts.csswg.org/css-multicol-1/#column-sizing>
//    and the pseudo-algorithm in <https://drafts.csswg.org/css-multicol-1/#pseudo-algorithm>
//  - column gaps: <https://drafts.csswg.org/css-multicol-1/#column-gap> (`normal`
//    computes to `1em`)
//  - balancing: <https://drafts.csswg.org/css-multicol-1/#balancing>

use app_units::Au;
use style::Zero;

/// The used column count, column inline size and column gap of a multi-column
/// container, as resolved by the CSS Multi-column pseudo-algorithm.
///
/// The container's content box is partitioned into `count` columns of `width`,
/// separated by `count - 1` gaps of `gap`:
///
/// ```text
/// |<-- width -->|  gap  |<-- width -->|  gap  |<-- width -->|
/// ```
///
/// <https://drafts.csswg.org/css-multicol-1/#pseudo-algorithm>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsedColumns {
    /// The used column count. Always at least 1.
    pub count: u32,
    /// The used column inline size (the `column-width` used value).
    pub width: Au,
    /// The used column gap (the `column-gap` used value; `normal` resolves to `1em`).
    pub gap: Au,
}

impl UsedColumns {
    /// The inline offset of the column box at `index` relative to the inline start
    /// of the container's content box.
    pub fn column_inline_offset(&self, index: u32) -> Au {
        (self.width + self.gap) * index as i32
    }

    /// The inline size occupied by all columns and gaps, i.e. the used inline size
    /// of the container's content box. When the container's available inline size is
    /// larger than this (which can only happen for `column-width` driven layouts),
    /// the remainder is empty space after the last column, exactly like in Chromium
    /// and Gecko.
    pub fn content_inline_size(&self) -> Au {
        if self.count == 0 {
            return Au::zero();
        }
        self.width * self.count as i32 + self.gap * (self.count as i32 - 1)
    }
}

/// Resolve the used column count, column inline size and column gap from the
/// specified values and the available inline size.
///
/// `column_count` and `column_width` are the *specified* values, where `None`
/// stands for the `auto` keyword. `gap` is the used `column-gap` (the caller
/// resolves `normal` to `1em` because it needs the font size for that).
///
/// This is a direct transcription of the pseudo-algorithm of
/// <https://drafts.csswg.org/css-multicol-1/#pseudo-algorithm>:
///
/// ```text
/// (01) if (column-width = auto) and (column-count = auto) then
/// (02)     N := 1; used-width := available-width
/// (03) else if ((column-width = auto) and (column-count != auto)) then
/// (04)     N := column-count; used-width := (available-width - column-gap × (N - 1)) / N
/// (05) else if ((column-width != auto) and (column-count = auto)) then
/// (06)     N := max(1, floor((available-width + column-gap) / (column-width + column-gap)))
/// (07)     used-width := ((available-width + column-gap) / N) - column-gap
/// (08) else /* both non-auto */
/// (09)     N := max(1, min(column-count, floor((available-width + column-gap) /
///                                      (column-width + column-gap))))
/// (10)     used-width := ((available-width + column-gap) / N) - column-gap
/// ```
pub fn resolve_used_columns(
    available_inline_size: Au,
    column_count: Option<u32>,
    column_width: Option<Au>,
    gap: Au,
) -> UsedColumns {
    let available = available_inline_size.max(Au::zero());
    let gap = gap.max(Au::zero());

    // A zero or negative `column-width + column-gap` cannot happen for the
    // specified types (`column-width` is a non-negative length and clamps to at
    // least 1px below), but stay defensive so that the divisions below are safe.
    let effective_column_width = column_width.map(|width| width.max(Au::from_px(1)));

    match (column_count, effective_column_width) {
        // (01)/(02): neither `column-count` nor `column-width` is specified.
        (None, None) => UsedColumns {
            count: 1,
            width: available,
            gap,
        },
        // (03)/(04): only `column-count` is specified.
        (Some(count), None) => {
            let count = (count as i32).max(1);
            let width = ((available - gap * (count - 1)) / count).max(Au::zero());
            UsedColumns {
                count: count as u32,
                width,
                gap,
            }
        },
        // (05)–(07): only `column-width` is specified.
        (None, Some(column_width)) => {
            let count = fit_count(available, column_width, gap);
            let width = (((available + gap) / count) - gap).max(Au::zero());
            UsedColumns {
                count: count as u32,
                width,
                gap,
            }
        },
        // (08)–(10): both are specified; `column-count` is a maximum.
        (Some(count), Some(column_width)) => {
            let count = (count as i32)
                .max(1)
                .min(fit_count(available, column_width, gap));
            let width = (((available + gap) / count) - gap).max(Au::zero());
            UsedColumns {
                count: count as u32,
                width,
                gap,
            }
        },
    }
}

/// `max(1, floor((available-width + column-gap) / (column-width + column-gap)))`
/// from lines (06) and (09) of the pseudo-algorithm.
fn fit_count(available: Au, column_width: Au, gap: Au) -> i32 {
    let unit = column_width + gap;
    if unit <= Au::zero() {
        return 1;
    }
    ((available + gap) / unit).max(1)
}

/// A block-axis range of the fragmented column flow, in the block coordinates of
/// the flow (i.e. starting at zero at the block start of the container's content
/// box).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColumnSlice {
    pub start: Au,
    pub end: Au,
}

impl ColumnSlice {
    /// The block size of the content that landed in this column.
    pub fn block_size(&self) -> Au {
        self.end - self.start
    }
}

/// The result of laying a fragmented flow out into columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnPlan {
    /// The column boxes in block order. The slices are contiguous: the first one
    /// starts at zero and each slice starts where the previous one ended.
    pub slices: Vec<ColumnSlice>,
    /// The used block size of the columns, i.e. the block size of the tallest
    /// column content. This is the block size the multi-column container's
    /// content box gets in the `height: auto` case.
    pub block_size: Au,
}

/// Fill the columns sequentially with the given column block size.
///
/// This implements the `column-fill: auto` behaviour, and also serves as the
/// simulation primitive of the balancing search in [`plan_balanced`]: it answers
/// "how many columns does the content need if each column is `height` tall".
///
/// `break_offsets` are the block offsets (in the flow's coordinates) at which the
/// flow may be broken. They must be sorted and must not contain duplicate values.
/// `content_end` is the block end of the flow; it behaves like a mandatory break
/// opportunity (a column always ends there at the latest).
pub fn plan_filled(break_offsets: &[Au], content_end: Au, height: Au) -> ColumnPlan {
    if content_end <= Au::zero() {
        return ColumnPlan {
            slices: Vec::new(),
            block_size: Au::zero(),
        };
    }

    // A zero block size would never advance the flow; clamp to at least 1au so
    // that the loop below always terminates.
    let height = height.max(Au::from_px(1));

    let mut slices = Vec::new();
    let mut start = Au::zero();
    while start < content_end {
        let cut = next_cut(break_offsets, start, start + height, content_end);
        slices.push(ColumnSlice { start, end: cut });
        start = cut;
    }

    let block_size = slices
        .iter()
        .map(ColumnSlice::block_size)
        .max()
        .unwrap_or_else(Au::zero);

    ColumnPlan { slices, block_size }
}

/// The largest break opportunity in `(start, limit]`, or—when no break opportunity
/// fits into the column—the first break opportunity after `limit`, so that no
/// unbreakable content is ever sliced. Content that is monolithic (an image, a
/// replaced element, a scroll container, …) therefore overflows the column instead
/// of being cut, exactly as required by
/// <https://drafts.csswg.org/css-multicol-1/#fragmentation>.
fn next_cut(break_offsets: &[Au], start: Au, limit: Au, content_end: Au) -> Au {
    if let Some(&cut) = break_offsets
        .iter()
        .rev()
        .find(|&&offset| offset > start && offset <= limit)
    {
        return cut;
    }

    if let Some(&cut) = break_offsets.iter().find(|&&offset| offset > limit) {
        return cut;
    }

    // The flow ends inside this column.
    content_end.max(start + Au::from_px(1))
}

/// Balance the fragmented flow into at most `max_columns` equally tall columns.
///
/// This implements the column balancing of
/// <https://drafts.csswg.org/css-multicol-1/#balancing>:
///
///  - When the block size is indefinite the content is balanced, i.e. the used
///    column block size is the smallest one for which the content fits into the
///    specified number of columns. This is found with a binary search over the
///    block size, using [`plan_filled`] as the simulation.
///  - The binary search's lower bound is `content_end / max_columns`: no column
///    block size below that can hold the whole flow without overflowing columns.
///  - Break opportunities are only ever honoured *between* fragments, so a
///    monolithic fragment taller than the current candidate is never split. This
///    makes the feasibility predicate monotone in practice (a larger column block
///    size never needs more columns), which is what the binary search relies on.
///
/// The returned [`ColumnPlan::block_size`] is the *actual* tallest column content,
/// which can be smaller than the searched block size because the cuts snap to the
/// last break opportunity of each column.
pub fn plan_balanced(break_offsets: &[Au], content_end: Au, max_columns: u32) -> ColumnPlan {
    if content_end <= Au::zero() {
        return ColumnPlan {
            slices: Vec::new(),
            block_size: Au::zero(),
        };
    }

    let max_columns = max_columns.max(1) as i32;

    // Lower bound of the search: the content cannot possibly fit into more than
    // `max_columns` columns that are all shorter than the average content size.
    let mut low = ceil_div(content_end, max_columns).max(Au::from_px(1));
    // The whole content in a single column is always feasible.
    let mut high = content_end;
    while low < high {
        let middle = low + (high - low) / 2;
        let plan = plan_filled(break_offsets, content_end, middle);
        if plan.slices.len() <= max_columns as usize {
            high = middle;
        } else {
            low = middle + Au::from_px(1);
        }
    }

    let plan = plan_filled(break_offsets, content_end, low);

    // The cuts snap to break opportunities, so the columns can end up shorter than
    // the searched block size. Use that tighter block size if the content still
    // fits into the allowed number of columns.
    if plan.block_size < low {
        let tightened = plan_filled(break_offsets, content_end, plan.block_size);
        if tightened.slices.len() <= max_columns as usize {
            return tightened;
        }
    }

    plan
}

/// `ceil(dividend / divisor)` for a non-negative dividend and a positive divisor.
///
/// The divisor is a *count* (a number of columns), not a length, so the rounding
/// term is `divisor - 1` app units rather than `divisor - 1` pixels.
fn ceil_div(dividend: Au, divisor: i32) -> Au {
    debug_assert!(divisor > 0);
    (dividend + Au(divisor - 1)) / divisor
}
