/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / C2): table-driven tests for the pure column
// geometry of CSS Multi-column layout (`flow/multicol/columns.rs`).
//
// The assertions pin:
//  - the column width/count pseudo-algorithm of
//    <https://drafts.csswg.org/css-multicol-1/#pseudo-algorithm> (lines 01–10),
//  - the used column gap arithmetic of
//    <https://drafts.csswg.org/css-multicol-1/#column-gap>,
//  - the greedy column filling of <https://drafts.csswg.org/css-multicol-1/#filling-columns>,
//  - the column balancing of <https://drafts.csswg.org/css-multicol-1/#balancing>.
//
// All lengths are expressed through the `px` helper (60 app units per CSS pixel) so
// that every expected number is computable by hand.

use app_units::Au;
use layout::flow::multicol::columns::{
    ColumnSlice, plan_balanced, plan_filled, resolve_used_columns,
};

/// A CSS pixel length in app units.
fn px(value: i32) -> Au {
    Au::from_px(value)
}

fn slices_tuple(plan: &layout::flow::multicol::columns::ColumnPlan) -> Vec<(i32, i32)> {
    plan.slices
        .iter()
        .map(|slice| (slice.start.to_px(), slice.end.to_px()))
        .collect()
}

// ---------------------------------------------------------------------------
// The pseudo-algorithm (lines 01–10)
// ---------------------------------------------------------------------------

#[test]
fn both_auto_is_a_single_full_width_column() {
    // (01)/(02): N := 1; used-width := available-width.
    let used = resolve_used_columns(px(300), None, None, px(10));
    assert_eq!(used.count, 1);
    assert_eq!(used.width, px(300));
    assert_eq!(used.gap, px(10));
    assert_eq!(used.content_inline_size(), px(300));
}

#[test]
fn column_count_only_divides_the_available_width() {
    // (03)/(04): N := column-count; used-width := (available - gap × (N - 1)) / N.
    let used = resolve_used_columns(px(301), Some(3), None, px(10));
    assert_eq!(used.count, 3);
    // (301 - 10 × 2) / 3 = 281 / 3 = 93.666…px, truncated to whole app units.
    assert_eq!(used.width, Au(5620));
    assert_eq!(used.content_inline_size(), Au(5620) * 3 + px(20));
}

#[test]
fn column_width_only_fits_as_many_columns_as_possible() {
    // (05)/(06): N := max(1, floor((available + gap) / (column-width + gap))).
    let used = resolve_used_columns(px(300), None, Some(px(100)), px(0));
    assert_eq!(used.count, 3);
    // (07): used-width := ((available + gap) / N) - gap.
    assert_eq!(used.width, px(100));

    // 350px + 10px gap fits floor(360 / 110) = 3 columns; the leftover space is
    // distributed so that the columns are 110px wide.
    let used = resolve_used_columns(px(350), None, Some(px(100)), px(10));
    assert_eq!(used.count, 3);
    assert_eq!(used.width, px(110));
}

#[test]
fn column_width_only_is_clamped_to_at_least_one_column() {
    // A container that is narrower than one column still gets a single column.
    let used = resolve_used_columns(px(50), None, Some(px(100)), px(10));
    assert_eq!(used.count, 1);
    // (07): ((50 + 10) / 1) - 10 = 50px.
    assert_eq!(used.width, px(50));
}

#[test]
fn both_specified_makes_column_count_a_maximum() {
    // (08)/(09): N := max(1, min(column-count, floor((available + gap) /
    //                                 (column-width + gap)))).
    let used = resolve_used_columns(px(350), Some(2), Some(px(100)), px(10));
    assert_eq!(used.count, 2);
    // (10): ((350 + 10) / 2) - 10 = 170px.
    assert_eq!(used.width, px(170));

    // `column-count` smaller than what fits: it wins.
    let used = resolve_used_columns(px(1000), Some(2), Some(px(100)), px(0));
    assert_eq!(used.count, 2);
    assert_eq!(used.width, px(500));
}

#[test]
fn a_gap_larger_than_the_container_leaves_one_zero_width_column() {
    let used = resolve_used_columns(px(100), Some(4), None, px(200));
    // (301): (100 - 200 × 3) / 4 is negative and clamps to zero.
    assert_eq!(used.count, 4);
    assert_eq!(used.width, px(0));
}

#[test]
fn a_zero_width_column_still_counts_as_one_column() {
    let used = resolve_used_columns(px(0), None, Some(px(100)), px(0));
    assert_eq!(used.count, 1);
    assert_eq!(used.width, px(0));
}

#[test]
fn negative_available_inline_size_clamps_to_zero() {
    let used = resolve_used_columns(px(-100), None, None, px(10));
    assert_eq!(used.count, 1);
    assert_eq!(used.width, px(0));
}

#[test]
fn column_offsets_and_content_inline_size() {
    let used = resolve_used_columns(px(320), Some(3), None, px(10));
    assert_eq!(used.width, px(100));
    assert_eq!(used.column_inline_offset(0), px(0));
    assert_eq!(used.column_inline_offset(1), px(110));
    assert_eq!(used.column_inline_offset(2), px(220));
    assert_eq!(used.content_inline_size(), px(320));
}

// ---------------------------------------------------------------------------
// Greedy filling (`column-fill: auto` and the definite block size case)
// ---------------------------------------------------------------------------

#[test]
fn filling_snaps_to_break_opportunities() {
    let breaks = [px(0), px(50), px(100)];
    let plan = plan_filled(&breaks, px(100), px(60));
    assert_eq!(slices_tuple(&plan), vec![(0, 50), (50, 100)]);
    // The tallest column content is 50px, not the 60px fill height.
    assert_eq!(plan.block_size, px(50));
}

#[test]
fn filling_covers_the_flow_contiguously() {
    let breaks = [px(0), px(10), px(30), px(55), px(99), px(100)];
    let plan = plan_filled(&breaks, px(100), px(25));
    assert_eq!(plan.slices.first().unwrap().start, px(0));
    assert_eq!(plan.slices.last().unwrap().end, px(100));
    for pair in plan.slices.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "column slices must be contiguous");
    }
}

#[test]
fn filling_never_slices_a_monolithic_fragment() {
    // Only two break opportunities: the flow start and the end of one 100px tall
    // monolithic fragment. A 20px column height cannot cut it.
    let breaks = [px(0), px(100)];
    let plan = plan_filled(&breaks, px(100), px(20));
    assert_eq!(slices_tuple(&plan), vec![(0, 100)]);
    assert_eq!(plan.block_size, px(100));
}

#[test]
fn filling_overflows_a_too_tall_monolithic_fragment_then_continues() {
    // A 40px monolithic fragment first, then breakable content in 10px steps.
    let breaks = [px(0), px(40), px(50), px(60), px(70), px(100)];
    let plan = plan_filled(&breaks, px(100), px(20));
    // The first column overflows up to 40px (the next break opportunity) because
    // the monolithic fragment may not be cut; the remaining columns then fill in
    // 20px steps.
    assert_eq!(
        slices_tuple(&plan),
        vec![(0, 40), (40, 60), (60, 70), (70, 100)]
    );
    // The tallest column content is the overflowing first column.
    assert_eq!(plan.block_size, px(40));
}

#[test]
fn filling_an_empty_flow_has_no_columns() {
    let plan = plan_filled(&[px(0)], px(0), px(50));
    assert!(plan.slices.is_empty());
    assert_eq!(plan.block_size, px(0));
}

#[test]
fn filling_always_advances() {
    // Degenerate input: a zero block size must still terminate and advance.
    let breaks = [px(0), px(10), px(20)];
    let plan = plan_filled(&breaks, px(20), px(0));
    assert_eq!(slices_tuple(&plan), vec![(0, 10), (10, 20)]);
}

// ---------------------------------------------------------------------------
// Balancing
// ---------------------------------------------------------------------------

#[test]
fn balancing_finds_the_smallest_fitting_column_height() {
    // Break opportunities every 10px; two columns must reach 100px, so the
    // balanced height is 50px.
    let breaks: Vec<Au> = (0..=10).map(|index| px(index * 10)).collect();
    let plan = plan_balanced(&breaks, px(100), 2);
    assert_eq!(slices_tuple(&plan), vec![(0, 50), (50, 100)]);
    assert_eq!(plan.block_size, px(50));
}

#[test]
fn balancing_uses_at_most_the_requested_number_of_columns() {
    // Break opportunities every 5px. The smallest column height for which three
    // columns reach the end of the flow is 35px: 35 + 35 + 30 = 100.
    let breaks: Vec<Au> = (0..=20).map(|index| px(index * 5)).collect();
    let plan = plan_balanced(&breaks, px(100), 3);
    assert_eq!(slices_tuple(&plan), vec![(0, 35), (35, 70), (70, 100)]);
    assert_eq!(plan.block_size, px(35));
}

#[test]
fn balancing_a_single_monolithic_fragment_stays_one_column() {
    let breaks = [px(0), px(100)];
    let plan = plan_balanced(&breaks, px(100), 3);
    assert_eq!(slices_tuple(&plan), vec![(0, 100)]);
    assert_eq!(plan.block_size, px(100));
}

#[test]
fn balancing_an_empty_flow_has_no_columns() {
    let plan = plan_balanced(&[px(0)], px(0), 3);
    assert!(plan.slices.is_empty());
    assert_eq!(plan.block_size, px(0));
}

#[test]
fn balancing_never_returns_more_columns_than_requested_or_slices_beyond_the_flow() {
    // Irregular break opportunities, including one exactly at the flow end.
    let breaks = [px(0), px(7), px(23), px(41), px(64), px(88), px(100)];
    for max_columns in 1..=5 {
        let plan = plan_balanced(&breaks, px(100), max_columns);
        assert!(
            plan.slices.len() <= max_columns as usize,
            "max_columns={max_columns} produced {} columns",
            plan.slices.len()
        );
        assert_eq!(plan.slices.first().unwrap().start, px(0));
        assert_eq!(plan.slices.last().unwrap().end, px(100));
        for pair in plan.slices.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for slice in &plan.slices {
            let slice: &ColumnSlice = slice;
            assert!(slice.end > slice.start, "every column must advance the flow");
        }
    }
}

#[test]
fn balancing_one_column_holds_the_whole_flow() {
    let breaks = [px(0), px(30), px(70), px(100)];
    let plan = plan_balanced(&breaks, px(100), 1);
    assert_eq!(slices_tuple(&plan), vec![(0, 100)]);
    assert_eq!(plan.block_size, px(100));
}
