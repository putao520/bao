/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / Ruby W1a): table-driven tests for the pure
// ruby plan surface (`flow/ruby/plan.rs` + `flow/ruby/builder.rs`) and the
// pure segment geometry (`flow/ruby/layout.rs`).
//
// The assertions pin the in-tree semantics of CSS Ruby §2.2 (anonymous box
// generation, eight rules), §2.3 (segments, pairing, degenerate cases,
// auto-hiding), HTML Ruby Extensions §2.1.1 (tag-first classification) and
// §3.1/§3.6/§4.6 (column widths, alignment, block axis). Payloads are
// `&'static str` node names; measurements are derived from text lengths with
// the closure below (60 AU per character), so every expected number is
// computable by hand.

use app_units::Au;
use layout::flow::ruby::builder::{ruby_child_kind_for_element, ruby_child_kind_for_text};
use layout::flow::ruby::layout::{
    AlignedDistribution, AnnotationLevelExtents, LevelMeasure, ResolvedRubyPosition, RubyAlignKind,
    RubyExtents, SpanningMeasure, align_distribution, block_extents, line_contribution,
    segment_column_widths,
};
use layout::flow::ruby::plan::{
    RubyChildKind, RubyPlanChild, RubyPlanLevel, RubyPlanSegment, RubyPlanUnitSource, build_plan,
};
use style::values::specified::box_ as stylo;
use web_atoms::local_name;

type Payload = &'static str;

/// The 60 AU per character measurement used by the geometry tests.
fn measure(text: &str) -> Au {
    Au::new(text.chars().count() as i32 * 60)
}

fn child(kind: RubyChildKind, payload: Payload, text: &str) -> RubyPlanChild<Payload> {
    RubyPlanChild {
        kind,
        payload,
        children: Vec::new(),
        text: text.to_owned(),
        collapsed: false,
    }
}

fn collapsed_child(kind: RubyChildKind, payload: Payload, text: &str) -> RubyPlanChild<Payload> {
    RubyPlanChild {
        collapsed: true,
        ..child(kind, payload, text)
    }
}

fn container(
    kind: RubyChildKind,
    payload: Payload,
    text: &str,
    children: Vec<RubyPlanChild<Payload>>,
) -> RubyPlanChild<Payload> {
    RubyPlanChild {
        kind,
        payload,
        children,
        text: text.to_owned(),
        collapsed: false,
    }
}

fn base_texts<P>(segment: &RubyPlanSegment<P>) -> Vec<&str> {
    segment
        .base_units
        .iter()
        .map(|unit| unit.text.as_str())
        .collect()
}

fn cell_spans<P>(level: &RubyPlanLevel<P>) -> Vec<(usize, usize)> {
    level
        .cells
        .iter()
        .map(|cell| (cell.span.start, cell.span.end))
        .collect()
}

fn cell_texts<P>(level: &RubyPlanLevel<P>) -> Vec<&str> {
    level
        .cells
        .iter()
        .map(|cell| cell.unit.text.as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// Classification (flow/ruby/builder.rs)
// ---------------------------------------------------------------------------

#[test]
fn classification_tag_first_for_html_elements() {
    let display = stylo::Display::Inline;
    let in_html = true;
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rb")), display, in_html),
        Some(RubyChildKind::Base)
    );
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rbc")), display, in_html),
        Some(RubyChildKind::BaseContainer)
    );
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rt")), display, in_html),
        Some(RubyChildKind::Annotation)
    );
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rtc")), display, in_html),
        Some(RubyChildKind::AnnotationContainer)
    );
    // `rp` is ignored entirely (HTML §2.1.1 step 1).
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rp")), display, in_html),
        None
    );
}

#[test]
fn classification_tags_only_apply_inside_html_documents() {
    let display = stylo::Display::Inline;
    // The same element names outside HTML documents carry no ruby semantics;
    // the element falls back to its display value (inline → Inline).
    assert_eq!(
        ruby_child_kind_for_element(Some(&local_name!("rb")), display, false),
        Some(RubyChildKind::Inline)
    );
    // No local name at all (pseudo elements): display only.
    assert_eq!(
        ruby_child_kind_for_element(None, display, true),
        Some(RubyChildKind::Inline)
    );
}

#[test]
fn classification_display_second() {
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::None, true),
        None
    );
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::RubyBase, true),
        Some(RubyChildKind::Base)
    );
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::RubyBaseContainer, true),
        Some(RubyChildKind::BaseContainer)
    );
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::RubyText, true),
        Some(RubyChildKind::Annotation)
    );
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::RubyTextContainer, true),
        Some(RubyChildKind::AnnotationContainer)
    );
    // Nested ruby containers and generic boxes are inline content of the
    // surrounding container (§2.2 rules 1-3).
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::Ruby, true),
        Some(RubyChildKind::Inline)
    );
    assert_eq!(
        ruby_child_kind_for_element(None, stylo::Display::InlineBlock, true),
        Some(RubyChildKind::Inline)
    );
}

#[test]
fn classification_of_text_children() {
    assert_eq!(ruby_child_kind_for_text(" \t\n "), RubyChildKind::Whitespace);
    assert_eq!(ruby_child_kind_for_text("漢字"), RubyChildKind::Inline);
    assert_eq!(ruby_child_kind_for_text(""), RubyChildKind::Whitespace);
}

// ---------------------------------------------------------------------------
// CSS Ruby §2.2: anonymous box generation rules
// ---------------------------------------------------------------------------

#[test]
fn rule3_bare_inline_content_becomes_one_anonymous_base() {
    let plan = build_plan(vec![
        child(RubyChildKind::Inline, "text", "漢"),
        child(RubyChildKind::Inline, "em", "字"),
    ]);
    assert_eq!(plan.segments.len(), 1);
    let segment = &plan.segments[0];
    assert!(segment.base_container.element.is_none());
    assert_eq!(base_texts(segment), ["漢字"]);
    let [unit] = &segment.base_units[..] else {
        unreachable!()
    };
    assert!(matches!(unit.source, RubyPlanUnitSource::Content(_)));
    assert!(unit.is_anonymous());
    assert!(segment.levels.is_empty());
}

#[test]
fn rule4_trims_leading_and_trailing_whitespace() {
    let plan = build_plan(vec![
        child(RubyChildKind::Whitespace, "text", " "),
        child(RubyChildKind::Base, "rb", "a"),
        child(RubyChildKind::Whitespace, "text", "  "),
    ]);
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(base_texts(&plan.segments[0]), ["a"]);
}

#[test]
fn rule4_sole_whitespace_content_is_kept() {
    // White space that is the only in-flow content of the ruby container is
    // not trimmed (§2.2 rule 4); rule 6 makes it its own segment.
    let plan = build_plan(vec![child(RubyChildKind::Whitespace, "text", " ")]);
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(base_texts(&plan.segments[0]), [" "]);
    assert!(plan.segments[0].levels.is_empty());
}

#[test]
fn rule5_removes_whitespace_before_annotations() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "a"),
        child(RubyChildKind::Whitespace, "text", " "),
        child(RubyChildKind::Annotation, "rt", "x"),
    ]);
    assert_eq!(plan.segments.len(), 1);
    let segment = &plan.segments[0];
    assert_eq!(base_texts(segment), ["a"]);
    assert_eq!(cell_texts(&segment.levels[0]), ["x"]);
    assert_eq!(cell_spans(&segment.levels[0]), [(0, 1)]);
}

#[test]
fn rule6_inter_base_whitespace_stays_a_column() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "a"),
        child(RubyChildKind::Whitespace, "text", " "),
        child(RubyChildKind::Base, "rb", "b"),
    ]);
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(base_texts(&plan.segments[0]), ["a", " ", "b"]);
    assert!(plan.segments[0].levels.is_empty());
}

#[test]
fn whitespace_beside_a_base_container_forms_its_own_box() {
    // §2.2 rule 8: white space next to a base container becomes a base
    // container box of its own; it neither joins the explicit container nor
    // the following base (§2.3 inter-segment white space handling).
    let plan = build_plan(vec![
        container(
            RubyChildKind::BaseContainer,
            "rbc",
            "a",
            vec![child(RubyChildKind::Base, "rb", "a")],
        ),
        child(RubyChildKind::Whitespace, "text", " "),
        child(RubyChildKind::Base, "rb", "b"),
    ]);
    assert_eq!(plan.segments.len(), 3);
    assert!(plan.segments[0].base_container.element.is_some());
    assert_eq!(base_texts(&plan.segments[0]), ["a"]);
    assert!(plan.segments[1].base_container.element.is_none());
    assert_eq!(base_texts(&plan.segments[1]), [" "]);
    assert_eq!(base_texts(&plan.segments[2]), ["b"]);
}

// ---------------------------------------------------------------------------
// CSS Ruby §2.3: segments, degenerate cases, pairing, hiding
// ---------------------------------------------------------------------------

#[test]
fn degenerate_annotation_container_first_assumes_an_empty_base() {
    let plan = build_plan(vec![container(
        RubyChildKind::AnnotationContainer,
        "rtc",
        "x",
        vec![child(RubyChildKind::Annotation, "rt", "x")],
    )]);
    assert_eq!(plan.segments.len(), 1);
    let segment = &plan.segments[0];
    // A zero-base segment assumes a single empty base (§2.1.1 step 7).
    assert_eq!(base_texts(segment), [""]);
    assert!(segment.base_container.element.is_none());
    assert_eq!(cell_texts(&segment.levels[0]), ["x"]);
    assert_eq!(cell_spans(&segment.levels[0]), [(0, 1)]);
}

#[test]
fn degenerate_consecutive_base_containers_form_two_segments() {
    let plan = build_plan(vec![
        container(
            RubyChildKind::BaseContainer,
            "rbc",
            "a",
            vec![child(RubyChildKind::Base, "rb", "a")],
        ),
        container(
            RubyChildKind::BaseContainer,
            "rbc",
            "b",
            vec![child(RubyChildKind::Base, "rb", "b")],
        ),
    ]);
    assert_eq!(plan.segments.len(), 2);
    assert_eq!(base_texts(&plan.segments[0]), ["a"]);
    assert!(plan.segments[0].base_container.element.is_some());
    assert_eq!(base_texts(&plan.segments[1]), ["b"]);
    assert!(plan.segments[1].base_container.element.is_some());
    assert!(plan.segments[0].levels.is_empty());
    assert!(plan.segments[1].levels.is_empty());
}

#[test]
fn degenerate_inter_segment_whitespace_forms_its_own_segment() {
    let plan = build_plan(vec![
        child(RubyChildKind::Annotation, "rt", "x"),
        child(RubyChildKind::Whitespace, "text", " "),
        child(RubyChildKind::Base, "rb", "a"),
    ]);
    // One segment for the annotation (with its assumed empty base), one for
    // the white space, one for the base (§2.3 inter-segment white space).
    assert_eq!(plan.segments.len(), 3);
    assert_eq!(base_texts(&plan.segments[0]), [""]);
    assert_eq!(cell_texts(&plan.segments[0].levels[0]), ["x"]);
    assert_eq!(base_texts(&plan.segments[1]), [" "]);
    assert_eq!(base_texts(&plan.segments[2]), ["a"]);
    assert!(plan.segments[2].levels.is_empty());
}

#[test]
fn pairing_explicit_rt_pairs_the_first_base_only() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "漢"),
        child(RubyChildKind::Base, "rb", "字"),
        child(RubyChildKind::Annotation, "rt", "kanji"),
    ]);
    let segment = &plan.segments[0];
    assert_eq!(base_texts(segment), ["漢", "字"]);
    // A level whose only annotation is an explicit `rt` pairs 1:1; the
    // spanning rule is reserved for anonymous annotations (§2.3).
    assert_eq!(cell_texts(&segment.levels[0]), ["kanji"]);
    assert_eq!(cell_spans(&segment.levels[0]), [(0, 1)]);
}

#[test]
fn pairing_single_anonymous_annotation_spans_whole_segment() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "漢"),
        child(RubyChildKind::Base, "rb", "字"),
        container(
            RubyChildKind::AnnotationContainer,
            "rtc",
            "かんじ",
            vec![child(RubyChildKind::Inline, "text", "かんじ")],
        ),
    ]);
    let segment = &plan.segments[0];
    assert_eq!(base_texts(segment), ["漢", "字"]);
    assert_eq!(cell_spans(&segment.levels[0]), [(0, 2)]);
    // W1a carries no `ruby-merge` value yet: anonymous runs default to
    // `separate` (`merge == false`), so the auto-hide comparison applies.
    assert!(!segment.levels[0].merge);
}

#[test]
fn pairing_extra_annotations_pair_empty_bases() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "a"),
        child(RubyChildKind::Annotation, "rt", "x"),
        child(RubyChildKind::Annotation, "rt", "y"),
    ]);
    let segment = &plan.segments[0];
    // The column count grows to fit the extra annotation; the trailing base
    // column is empty (§2.3).
    assert_eq!(base_texts(segment), ["a", ""]);
    assert_eq!(cell_texts(&segment.levels[0]), ["x", "y"]);
    assert_eq!(cell_spans(&segment.levels[0]), [(0, 1), (1, 2)]);
}

// ---------------------------------------------------------------------------
// §2.3 auto-hiding
// ---------------------------------------------------------------------------

#[test]
fn autohide_hides_identical_text() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "漢"),
        child(RubyChildKind::Annotation, "rt", "漢"),
    ]);
    let cell = &plan.segments[0].levels[0].cells[0];
    assert!(cell.hidden);
    // The collapse flag folded into `hidden` is cleared on the unit.
    assert!(!cell.unit.collapsed);
}

#[test]
fn autohide_keeps_different_text_visible() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "漢"),
        child(RubyChildKind::Annotation, "rt", "か"),
    ]);
    assert!(!plan.segments[0].levels[0].cells[0].hidden);
}

#[test]
fn autohide_spanning_annotation_compares_the_whole_base_text() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "漢"),
        child(RubyChildKind::Base, "rb", "字"),
        container(
            RubyChildKind::AnnotationContainer,
            "rtc",
            "漢字",
            vec![child(RubyChildKind::Inline, "text", "漢字")],
        ),
    ]);
    let cell = &plan.segments[0].levels[0].cells[0];
    assert_eq!(cell.span, 0..2);
    assert!(cell.hidden);
}

#[test]
fn autohide_collapsed_annotation_hides_and_clears_the_collapse_flag() {
    let plan = build_plan(vec![
        child(RubyChildKind::Base, "rb", "base"),
        collapsed_child(RubyChildKind::Annotation, "rt", "x"),
    ]);
    let cell = &plan.segments[0].levels[0].cells[0];
    assert!(cell.hidden);
    assert!(!cell.unit.collapsed);
}

// ---------------------------------------------------------------------------
// flow/ruby/layout.rs: column widths (§3.1.1)
// ---------------------------------------------------------------------------

#[test]
fn column_width_single_cells_take_the_column_max() {
    let bases = [measure("漢"), measure("字")];
    let level = LevelMeasure {
        cells: vec![Some(measure("か")), Some(measure("かんじ"))],
        spans: Vec::new(),
    };
    let widths = segment_column_widths(&bases, &[level]);
    assert_eq!(widths, vec![measure("かんじ"), measure("かんじ")]);
}

#[test]
fn column_width_spanning_excess_is_split_with_remainder_on_leading_columns() {
    // Three columns of one character each; the spanning annotation is wider
    // than their sum by 10 AU: 3 AU for every column plus 1 AU for the first
    // column (remainder of 10 / 3).
    let bases = [measure("a"), measure("b"), measure("c")];
    let level = LevelMeasure {
        cells: vec![None, None, None],
        spans: vec![SpanningMeasure {
            span: 0..3,
            width: Au::new(190),
        }],
    };
    let widths = segment_column_widths(&bases, &[level]);
    assert_eq!(widths, vec![Au::new(64), Au::new(63), Au::new(63)]);
}

#[test]
fn column_width_multiple_spans_are_processed_shortest_first() {
    let bases = [
        measure("a"),
        measure("b"),
        measure("c"),
        measure("d"),
    ];
    let level = LevelMeasure {
        cells: vec![None, None, None, None],
        spans: vec![
            // Out of order on purpose: the two-column span is resolved first
            // (§3.1.1), and the whole-segment span sees the stretched
            // columns.
            SpanningMeasure {
                span: 2..4,
                width: Au::new(200),
            },
            SpanningMeasure {
                span: 0..4,
                width: Au::new(400),
            },
        ],
    };
    let widths = segment_column_widths(&bases, &[level]);
    assert_eq!(
        widths,
        vec![Au::new(80), Au::new(80), Au::new(120), Au::new(120)]
    );
}

#[test]
fn column_width_hidden_cells_claim_no_width() {
    // Hidden (or absent) cells are `None` and are skipped; a spanning cell
    // that is not wider than the columns it spans changes nothing.
    let bases = [measure("a"), measure("b"), measure("c")];
    let hidden = LevelMeasure {
        cells: vec![None, Some(measure("か")), None],
        spans: Vec::new(),
    };
    let narrow_span = LevelMeasure {
        cells: vec![],
        spans: vec![SpanningMeasure {
            span: 0..3,
            width: Au::new(100),
        }],
    };
    let widths = segment_column_widths(&bases, &[hidden, narrow_span]);
    assert_eq!(widths, vec![measure("a"), measure("か"), measure("c")]);
}

// ---------------------------------------------------------------------------
// flow/ruby/layout.rs: ruby-align (§4.6 table)
// ---------------------------------------------------------------------------

#[test]
fn align_distribution_follows_the_section_4_6_table() {
    let extra = Au::new(100);
    // Start: no offset, no justification.
    assert_eq!(
        align_distribution(RubyAlignKind::Start, extra, 3),
        AlignedDistribution {
            start_offset: Au::new(0),
            justify_extra: Au::new(0)
        }
    );
    // Center: half the extra up front.
    assert_eq!(
        align_distribution(RubyAlignKind::Center, Au::new(101), 3),
        AlignedDistribution {
            start_offset: Au::new(50),
            justify_extra: Au::new(0)
        }
    );
    // Space-between: everything into the opportunities; without any
    // opportunity it falls back to center.
    assert_eq!(
        align_distribution(RubyAlignKind::SpaceBetween, extra, 4),
        AlignedDistribution {
            start_offset: Au::new(0),
            justify_extra: Au::new(25)
        }
    );
    assert_eq!(
        align_distribution(RubyAlignKind::SpaceBetween, extra, 0),
        AlignedDistribution {
            start_offset: Au::new(50),
            justify_extra: Au::new(0)
        }
    );
    // Space-around: half up front, half into the opportunities; without any
    // opportunity the trailing half stays trailing.
    assert_eq!(
        align_distribution(RubyAlignKind::SpaceAround, extra, 4),
        AlignedDistribution {
            start_offset: Au::new(50),
            justify_extra: Au::new(12)
        }
    );
    assert_eq!(
        align_distribution(RubyAlignKind::SpaceAround, extra, 0),
        AlignedDistribution {
            start_offset: Au::new(50),
            justify_extra: Au::new(50)
        }
    );
}

// ---------------------------------------------------------------------------
// flow/ruby/layout.rs: block axis (§3.1.2 / §3.6)
// ---------------------------------------------------------------------------

#[test]
fn block_extents_stack_margin_boxes_without_gaps() {
    let levels = [
        AnnotationLevelExtents {
            position: ResolvedRubyPosition::Over,
            margin_box: Au::new(20),
        },
        AnnotationLevelExtents {
            position: ResolvedRubyPosition::Under,
            margin_box: Au::new(30),
        },
        AnnotationLevelExtents {
            position: ResolvedRubyPosition::Over,
            margin_box: Au::new(15),
        },
    ];
    let extents = block_extents(&levels);
    assert_eq!(extents.over, Au::new(35));
    assert_eq!(extents.under, Au::new(30));
}

#[test]
fn inter_character_resolves_to_the_over_side() {
    let levels = [
        AnnotationLevelExtents {
            position: ResolvedRubyPosition::Over,
            margin_box: Au::new(10),
        },
        AnnotationLevelExtents {
            position: ResolvedRubyPosition::InterCharacter,
            margin_box: Au::new(15),
        },
    ];
    let extents = block_extents(&levels);
    assert_eq!(extents.over, Au::new(25));
    assert_eq!(extents.under, Au::new(0));
}

#[test]
fn line_contribution_grows_by_extents_and_adds_leading() {
    let extents = RubyExtents {
        over: Au::new(30),
        under: Au::new(10),
    };
    // The ruby extent (ascent 110 + descent 30 = 140) exceeds the container
    // line height: the line grows, no extra leading.
    let metrics = line_contribution(Au::new(80), Au::new(20), &extents, Au::new(100));
    assert_eq!(metrics.ascent, Au::new(110));
    assert_eq!(metrics.descent, Au::new(30));
    assert_eq!(metrics.line_height, Au::new(140));
    assert_eq!(metrics.leading, Au::new(0));

    // A taller container line height adds leading on top of the extent.
    let metrics = line_contribution(Au::new(80), Au::new(20), &extents, Au::new(160));
    assert_eq!(metrics.line_height, Au::new(160));
    assert_eq!(metrics.leading, Au::new(20));
}
