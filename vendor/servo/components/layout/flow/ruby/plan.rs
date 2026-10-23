/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / Ruby W1a): pure ruby box-generation planner.
//
// This module implements, as a payload-generic pure algorithm:
//  - CSS Ruby §2.2 anonymous box generation (rules 2-8),
//  - CSS Ruby §2.3 segment splitting and base/annotation pairing,
//  - HTML Ruby Extensions §2.1.1 pairing (tag-driven unit boundaries).
//
// It is kept free of layout types so that it can be exercised by the
// table-driven integration tests in `layout/tests/ruby_plan.rs`. The DOM glue
// that classifies flat-tree children into [`RubyPlanChild`]s and that consumes
// the resulting [`RubyPlan`] lives in `flow/ruby/builder.rs`.

use std::ops::Range;

/// How a direct child of a ruby container (or of an internal ruby container)
/// is classified for the purposes of box generation.
///
/// Classification is tag-first and display-second (HTML Ruby Extensions
/// §2.1.1 pairs by tags; the computed `display` only matters for generic
/// elements). The DOM glue performs the classification; this module only
/// consumes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RubyChildKind {
    /// An `rb` element, or an element with `display: ruby-base`.
    Base,
    /// An `rbc` element, or an element with `display: ruby-base-container`.
    BaseContainer,
    /// An `rt` element, or an element with `display: ruby-text`.
    Annotation,
    /// An `rtc` element, or an element with `display: ruby-text-container`.
    AnnotationContainer,
    /// Collapsible white space directly inside a ruby container.
    Whitespace,
    /// Any other inline-level content, including nested `<ruby>` elements and
    /// inlinified block-level boxes (§2.2 rule 1).
    Inline,
}

impl RubyChildKind {
    /// Whether children of this kind belong to the base level of a ruby.
    pub fn is_base_like(self) -> bool {
        matches!(
            self,
            RubyChildKind::Base | RubyChildKind::BaseContainer | RubyChildKind::Inline
        )
    }

    /// Whether children of this kind belong to an annotation level of a ruby.
    pub fn is_annotation_like(self) -> bool {
        matches!(
            self,
            RubyChildKind::Annotation | RubyChildKind::AnnotationContainer
        )
    }
}

/// One classified child of a ruby container handed to the planner.
///
/// `P` is the opaque per-node payload the DOM glue needs later when it
/// materializes boxes from the plan (style handles, box slots, ...).
#[derive(Debug)]
pub struct RubyPlanChild<P> {
    pub kind: RubyChildKind,
    /// The payload of this child, moved into the resulting plan.
    pub payload: P,
    /// For containers, the recursively classified children; empty otherwise.
    pub children: Vec<RubyPlanChild<P>>,
    /// The text content of this child, compared before any white space
    /// collapsing or text transformation for auto-hiding (§2.3 hiding).
    pub text: String,
    /// Whether this child is hidden by `visibility: collapse`.
    pub collapsed: bool,
}

/// The source of the content of a base or annotation unit.
#[derive(Debug)]
pub enum RubyPlanUnitSource<P> {
    /// An explicit `rb`/`rt` element (or an element with the matching
    /// `display` value); its contents are the element's children.
    Element(P),
    /// An anonymous unit generated from a run of bare inline content; the
    /// contents are the run's nodes in document order.
    Content(Vec<P>),
    /// An anonymous unit made of white space only. White space that survives
    /// rules 4-6 becomes a unit of its own (§2.2 rule 6), and inter-segment
    /// white space forms a segment of its own (§2.3).
    Text(String),
    /// An anonymous unit with no content at all. Empty ruby containers
    /// "consider to contain a single empty base/annotation" (§2.3), and
    /// unpaired extra bases/annotations pair with empty units.
    Empty,
}

/// A base or annotation unit: the atomic-like pairing and line-breaking unit
/// of a ruby segment.
#[derive(Debug)]
pub struct RubyPlanUnit<P> {
    pub source: RubyPlanUnitSource<P>,
    /// The raw text of this unit, used for auto-hiding comparisons.
    pub text: String,
    /// Whether this unit is hidden by `visibility: collapse`.
    pub collapsed: bool,
}

impl<P> RubyPlanUnit<P> {
    /// Whether this unit was generated anonymously (not an explicit
    /// `rb`/`rt` element). A level whose only annotation is anonymous spans
    /// the whole segment (§2.3 pairing).
    pub fn is_anonymous(&self) -> bool {
        !matches!(self.source, RubyPlanUnitSource::Element(_))
    }

    /// Whether this unit has no renderable content: neither an element, nor
    /// a bare-content payload run, nor any text.
    pub fn is_empty(&self) -> bool {
        match &self.source {
            RubyPlanUnitSource::Empty => self.text.is_empty(),
            RubyPlanUnitSource::Text(_) | RubyPlanUnitSource::Content(_) => false,
            RubyPlanUnitSource::Element(_) => false,
        }
    }
}

/// A base container or annotation container of a segment.
#[derive(Debug)]
pub struct RubyPlanContainer<P> {
    /// The explicit `rbc`/`rtc` element of this container, if any. An
    /// anonymous container has no element and no content of its own; its
    /// units are attached to the segment instead.
    pub element: Option<P>,
}

/// One paired annotation: a unit plus the range of base columns it spans.
#[derive(Debug)]
pub struct RubyPlanCell<P> {
    pub unit: RubyPlanUnit<P>,
    /// The half-open range of base column indexes spanned by this unit.
    /// A single-column cell has a range of length one; a spanning annotation
    /// (§2.3: a level whose only annotation is anonymous) spans the whole
    /// segment.
    pub span: Range<usize>,
    /// Whether this cell is hidden: either `visibility: collapse` on the
    /// annotation, or auto-hidden because the annotation text is identical to
    /// the base text and `ruby-merge` is not `merge` (§2.3 hiding).
    pub hidden: bool,
}

/// One annotation level (annotation container) of a segment.
#[derive(Debug)]
pub struct RubyPlanLevel<P> {
    pub container: RubyPlanContainer<P>,
    pub cells: Vec<RubyPlanCell<P>>,
    /// Whether `ruby-merge: merge` applies to this level. Merged levels
    /// disable auto-hiding (§2.3 hiding). Only explicit annotation containers
    /// carry their own value in W1a; anonymous `rt` runs default to `false`
    /// (merge *rendering* itself is a later wave per the design).
    pub merge: bool,
}

/// One ruby segment: one base container plus zero or more annotation levels,
/// all paired with the same columns.
#[derive(Debug)]
pub struct RubyPlanSegment<P> {
    pub base_container: RubyPlanContainer<P>,
    /// The base units of this segment, in document order. These occupy the
    /// columns of the segment; extra columns implied by unpaired annotation
    /// units are backed by empty base units appended during pairing.
    pub base_units: Vec<RubyPlanUnit<P>>,
    pub levels: Vec<RubyPlanLevel<P>>,
}

/// The complete plan for one ruby container.
#[derive(Debug)]
pub struct RubyPlan<P> {
    pub segments: Vec<RubyPlanSegment<P>>,
}

/// The items the segment grouping loop consumes, after white space
/// interpretation and inline run merging.
enum GroupingItem<P> {
    /// An explicit base unit, in its own anonymous-or-explicit container.
    Base(RubyPlanUnit<P>),
    /// An explicit base container element with its already-generated units.
    BaseContainer {
        container: RubyPlanContainer<P>,
        units: Vec<RubyPlanUnit<P>>,
    },
    /// Bare inline content (or white space absorbed into an inline run).
    InlinePart(RubyPlanUnit<P>),
    /// An explicit annotation unit.
    Annotation(RubyPlanUnit<P>),
    /// An explicit annotation container element with its units.
    AnnotationContainer {
        container: RubyPlanContainer<P>,
        units: Vec<RubyPlanUnit<P>>,
        merge: bool,
    },
    /// White space between base container-ish boxes: an anonymous base
    /// container holding one anonymous base unit with the white space
    /// (§2.2 rule 6).
    WhitespaceBaseContainer(RubyPlanUnit<P>),
    /// Inter-segment white space: forms a segment of its own (§2.3).
    WhitespaceSegment(RubyPlanUnit<P>),
}

/// The mutable state of the segment grouping loop.
struct SegmentBuilder<P> {
    base_container: RubyPlanContainer<P>,
    base_units: Vec<RubyPlanUnit<P>>,
    /// Whether the base container is still open for more base-ish children.
    /// An explicit `rbc` element is a closed box: children that follow it in
    /// document order start a new base container.
    base_container_open: bool,
    levels: Vec<OpenLevel<P>>,
}

struct OpenLevel<P> {
    container: RubyPlanContainer<P>,
    units: Vec<RubyPlanUnit<P>>,
    /// Whether this level was generated as an anonymous run of `rt` units.
    /// Consecutive `rt` units merge into one run; an explicit `rtc` is one
    /// closed level of its own.
    open_run: bool,
    merge: bool,
}

impl<P> SegmentBuilder<P> {
    fn new_anonymous() -> Self {
        Self {
            base_container: RubyPlanContainer { element: None },
            base_units: Vec::new(),
            base_container_open: true,
            levels: Vec::new(),
        }
    }

    fn new_with_base_container(container: RubyPlanContainer<P>) -> Self {
        Self {
            base_container: container,
            base_units: Vec::new(),
            // An explicit container element is closed for planning purposes:
            // its own children have already been turned into units.
            base_container_open: false,
            levels: Vec::new(),
        }
    }
}

/// Build the [`RubyPlan`] for one ruby container from its classified
/// children.
pub fn build_plan<P>(children: Vec<RubyPlanChild<P>>) -> RubyPlan<P> {
    let items = interpret_children(children);
    let segments = group_into_segments(items);
    RubyPlan { segments }
}

/// Apply §2.2 rules 4 (leading/trailing trim), 5 (inter-level white space
/// removal) and 6 (intra-level white space interpretation) to the children of
/// one ruby container, merge adjacent inline content into anonymous base run
/// units (§2.2 rule 3/8), and recurse into explicit containers to generate
/// their units.
fn interpret_children<P>(children: Vec<RubyPlanChild<P>>) -> Vec<GroupingItem<P>> {
    // Rule 4: leading and trailing intra-ruby white space is trimmed, unless
    // it is the only in-flow content of the ruby container.
    let has_non_whitespace = children
        .iter()
        .any(|child| child.kind != RubyChildKind::Whitespace);
    let mut trimmed = children;
    if has_non_whitespace {
        while matches!(
            trimmed.first().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.remove(0);
        }
        while matches!(
            trimmed.last().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.pop();
        }
    }

    // Rule 5: inter-level white space removal. A white space run is removed
    // when it is followed by an annotation container, or when it is followed
    // by an annotation and not preceded by an annotation.
    let kinds: Vec<RubyChildKind> = trimmed.iter().map(|child| child.kind).collect();
    // For each white space run, whether it survives rule 5. White space that
    // survives is represented by its first child; the rest of the run is
    // dropped (adjacent white space is one collapsible run).
    let mut run_representative: Vec<Option<usize>> = vec![None; trimmed.len()];
    let mut index = 0;
    while index < trimmed.len() {
        if kinds[index] != RubyChildKind::Whitespace {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < trimmed.len() && kinds[index] == RubyChildKind::Whitespace {
            index += 1;
        }
        let run_end = index;
        let previous_kind = if run_start > 0 {
            Some(kinds[run_start - 1])
        } else {
            None
        };
        let next_kind = kinds.get(run_end).copied();
        let remove = match next_kind {
            Some(RubyChildKind::AnnotationContainer) => true,
            Some(RubyChildKind::Annotation) => previous_kind != Some(RubyChildKind::Annotation),
            _ => false,
        };
        if !remove {
            run_representative[run_start] = Some(run_start);
        }
    }

    // Rule 6 + inline run merging: build the grouping items.
    let mut items: Vec<GroupingItem<P>> = Vec::with_capacity(trimmed.len());
    let mut inline_run: Option<(Vec<P>, String, bool)> = None;
    let mut pending_whitespace: Option<(
        String,
        bool,
        Option<RubyChildKind>,
        Option<RubyChildKind>,
    )> = None;

    // Walk forward, consuming run representatives in order.
    let mut consumed: Vec<Option<RubyPlanChild<P>>> = trimmed.into_iter().map(Some).collect();
    for index in 0..consumed.len() {
        let kind = kinds[index];
        // Emit any pending white space item *before* the child that follows
        // it, since rule 6 interpretation needs both neighbors.
        if let Some((text, collapsed, previous_kind, next_kind)) = pending_whitespace.take() {
            let item = whitespace_item(text, collapsed, previous_kind, next_kind);
            push_item(&mut items, &mut inline_run, item);
        }
        if kind == RubyChildKind::Whitespace {
            // Only the representative of a surviving run is interpreted; the
            // rest of the run was already folded into it.
            if run_representative[index].is_some() {
                let child = consumed[index].take().expect("Child consumed twice");
                let next_kind = kinds[index + 1..]
                    .iter()
                    .copied()
                    .find(|kind| *kind != RubyChildKind::Whitespace);
                pending_whitespace = Some((
                    child.text,
                    child.collapsed,
                    last_non_whitespace_kind(&kinds, index),
                    next_kind,
                ));
            }
            continue;
        }
        let child = consumed[index].take().expect("Child consumed twice");
        match kind {
            RubyChildKind::Inline => {
                let run = inline_run.get_or_insert_with(|| (Vec::new(), String::new(), false));
                run.0.push(child.payload);
                run.1.push_str(&child.text);
                run.2 |= child.collapsed;
            },
            _ => {
                flush_inline_run(&mut items, &mut inline_run);
                items.push(non_whitespace_item(child));
            },
        }
    }
    if let Some((text, collapsed, previous_kind, next_kind)) = pending_whitespace.take() {
        let item = whitespace_item(text, collapsed, previous_kind, next_kind);
        push_item(&mut items, &mut inline_run, item);
    }
    flush_inline_run(&mut items, &mut inline_run);
    items
}

fn last_non_whitespace_kind(kinds: &[RubyChildKind], before: usize) -> Option<RubyChildKind> {
    kinds[..before]
        .iter()
        .rev()
        .copied()
        .find(|kind| *kind != RubyChildKind::Whitespace)
}

/// Fold the accumulated inline run into one anonymous base unit.
fn flush_inline_run<P>(items: &mut Vec<GroupingItem<P>>, run: &mut Option<(Vec<P>, String, bool)>) {
    if let Some((payloads, text, collapsed)) = run.take() {
        if !payloads.is_empty() {
            items.push(GroupingItem::InlinePart(RubyPlanUnit {
                source: RubyPlanUnitSource::Content(payloads),
                text,
                collapsed,
            }));
        }
    }
}

/// Feed one item into the item list, keeping inline parts contiguous so that
/// adjacent inline content forms a single anonymous base unit.
fn push_item<P>(
    items: &mut Vec<GroupingItem<P>>,
    inline_run: &mut Option<(Vec<P>, String, bool)>,
    item: GroupingItem<P>,
) {
    match item {
        GroupingItem::InlinePart(unit) => {
            let source = match unit.source {
                RubyPlanUnitSource::Content(payloads) => payloads,
                source => {
                    // Not a content run (defensive); emit as-is.
                    items.push(GroupingItem::InlinePart(RubyPlanUnit {
                        source,
                        text: unit.text,
                        collapsed: unit.collapsed,
                    }));
                    return;
                },
            };
            let run = inline_run.get_or_insert_with(|| (Vec::new(), String::new(), false));
            run.0.extend(source);
            run.1.push_str(&unit.text);
            run.2 |= unit.collapsed;
        },
        item => {
            flush_inline_run(items, inline_run);
            items.push(item);
        },
    }
}

/// Turn a surviving white space run into its grouping item (§2.2 rule 6).
fn whitespace_item<P>(
    text: String,
    collapsed: bool,
    previous_kind: Option<RubyChildKind>,
    next_kind: Option<RubyChildKind>,
) -> GroupingItem<P> {
    let previous_base_like = previous_kind.is_some_and(RubyChildKind::is_base_like);
    let previous_annotation_like = previous_kind.is_some_and(RubyChildKind::is_annotation_like);
    let next_base_like = next_kind.is_some_and(RubyChildKind::is_base_like);
    let next_annotation_like = next_kind.is_some_and(RubyChildKind::is_annotation_like);

    // A unit made from white space is anonymous and has no content of its
    // own; only its text is kept for inter-linear spacing semantics.
    let make_unit = |text: String| RubyPlanUnit {
        source: RubyPlanUnitSource::Text(text.clone()),
        text,
        collapsed,
    };

    if previous_base_like && next_base_like {
        if previous_kind == Some(RubyChildKind::BaseContainer)
            || next_kind == Some(RubyChildKind::BaseContainer)
        {
            return GroupingItem::WhitespaceBaseContainer(make_unit(text));
        }
        // Inter-base white space between two structural base boxes.
        if previous_kind == Some(RubyChildKind::Inline) || next_kind == Some(RubyChildKind::Inline)
        {
            // Adjacent to bare inline content: join the inline run instead.
            return GroupingItem::InlinePart(make_unit(text));
        }
        return GroupingItem::Base(make_unit(text));
    }
    if previous_annotation_like && next_annotation_like {
        return GroupingItem::Annotation(make_unit(text));
    }
    GroupingItem::WhitespaceSegment(make_unit(text))
}

/// Turn one non-whitespace child into a grouping item, recursing into
/// explicit containers to generate their units.
fn non_whitespace_item<P>(child: RubyPlanChild<P>) -> GroupingItem<P> {
    match child.kind {
        RubyChildKind::Base => GroupingItem::Base(RubyPlanUnit {
            source: RubyPlanUnitSource::Element(child.payload),
            text: child.text,
            collapsed: child.collapsed,
        }),
        RubyChildKind::BaseContainer => {
            let (container_element, units) = container_units_with_payload(child);
            GroupingItem::BaseContainer {
                container: RubyPlanContainer {
                    element: container_element,
                },
                units,
            }
        },
        RubyChildKind::Annotation => GroupingItem::Annotation(RubyPlanUnit {
            source: RubyPlanUnitSource::Element(child.payload),
            text: child.text,
            collapsed: child.collapsed,
        }),
        RubyChildKind::AnnotationContainer => {
            let (container_element, units) = annotation_container_units(child);
            GroupingItem::AnnotationContainer {
                container: RubyPlanContainer {
                    element: container_element,
                },
                units,
                merge: false,
            }
        },
        RubyChildKind::Inline | RubyChildKind::Whitespace => unreachable!(
            "Inline and Whitespace children are handled by interpret_children() before this point"
        ),
    }
}

/// Generate the units of an explicit container's children and return the
/// container's own payload alongside.
///
/// Inside a base container every `Base` child and every maximal run of bare
/// content is one unit; inside an annotation container every `Annotation`
/// child and every maximal run of non-annotation content is one unit
/// (HTML §2.1.1 step 5). Annotation children of a base container (and vice
/// versa) are treated as bare content: out-of-place internal ruby boxes are
/// wrapped in anonymous containers in a later wave.
fn container_units_with_payload<P>(
    container: RubyPlanChild<P>,
) -> (Option<P>, Vec<RubyPlanUnit<P>>) {
    let RubyPlanChild {
        kind,
        payload,
        children,
        collapsed: container_collapsed,
        ..
    } = container;
    let is_unit_separator = |child_kind: RubyChildKind| -> bool {
        match kind {
            RubyChildKind::BaseContainer => child_kind == RubyChildKind::Base,
            RubyChildKind::AnnotationContainer => child_kind == RubyChildKind::Annotation,
            _ => false,
        }
    };

    // Rule 4 inside the container: trim leading/trailing white space unless
    // it is the only content.
    let has_non_whitespace = children
        .iter()
        .any(|child| child.kind != RubyChildKind::Whitespace);
    let mut trimmed = children;
    if has_non_whitespace {
        while matches!(
            trimmed.first().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.remove(0);
        }
        while matches!(
            trimmed.last().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.pop();
        }
    }

    let mut units: Vec<RubyPlanUnit<P>> = Vec::new();
    let mut run: Option<(Vec<P>, String, bool)> = None;
    {
        let flush = |units: &mut Vec<RubyPlanUnit<P>>, run: &mut Option<(Vec<P>, String, bool)>| {
            if let Some((payloads, text, collapsed)) = run.take() {
                if !payloads.is_empty() {
                    units.push(RubyPlanUnit {
                        source: RubyPlanUnitSource::Content(payloads),
                        text,
                        collapsed,
                    });
                }
            }
        };

        for child in trimmed {
            if is_unit_separator(child.kind) {
                flush(&mut units, &mut run);
                units.push(RubyPlanUnit {
                    source: RubyPlanUnitSource::Element(child.payload),
                    text: child.text,
                    collapsed: child.collapsed,
                });
                continue;
            }
            let run = run.get_or_insert_with(|| (Vec::new(), String::new(), false));
            match child.kind {
                RubyChildKind::Whitespace => {
                    // White space inside a container joins the surrounding
                    // bare-content run (or starts one of its own).
                    run.1.push_str(&child.text);
                    run.2 |= child.collapsed;
                    if child.collapsed && run.0.is_empty() {
                        run.2 = true;
                    }
                },
                _ => {
                    run.0.push(child.payload);
                    run.1.push_str(&child.text);
                    run.2 |= child.collapsed;
                },
            }
        }
        flush(&mut units, &mut run);
    }

    if units.is_empty() {
        units.push(RubyPlanUnit {
            source: RubyPlanUnitSource::Empty,
            text: String::new(),
            // An empty container "counts as containing one empty base/annotation";
            // a collapsed empty container has nothing to show anyway.
            collapsed: container_collapsed,
        });
    }
    (Some(payload), units)
}

/// Generate the units of an explicit annotation container, keeping the
/// container payload (HTML §2.1.1 step 5).
fn annotation_container_units<P>(container: RubyPlanChild<P>) -> (Option<P>, Vec<RubyPlanUnit<P>>) {
    let RubyPlanChild {
        kind,
        payload,
        children,
        collapsed: container_collapsed,
        ..
    } = container;
    let is_unit_separator =
        |child_kind: RubyChildKind| -> bool { child_kind == RubyChildKind::Annotation };

    // Rule 4 inside the container: trim leading/trailing white space unless
    // it is the only content.
    let has_non_whitespace = children
        .iter()
        .any(|child| child.kind != RubyChildKind::Whitespace);
    let mut trimmed = children;
    if has_non_whitespace {
        while matches!(
            trimmed.first().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.remove(0);
        }
        while matches!(
            trimmed.last().map(|child| child.kind),
            Some(RubyChildKind::Whitespace)
        ) {
            trimmed.pop();
        }
    }

    let mut units: Vec<RubyPlanUnit<P>> = Vec::new();
    let mut run: Option<(Vec<P>, String, bool)> = None;
    {
        let flush = |units: &mut Vec<RubyPlanUnit<P>>, run: &mut Option<(Vec<P>, String, bool)>| {
            if let Some((payloads, text, collapsed)) = run.take() {
                if !payloads.is_empty() || !text.is_empty() {
                    units.push(RubyPlanUnit {
                        source: RubyPlanUnitSource::Content(payloads),
                        text,
                        collapsed,
                    });
                }
            }
        };

        for child in trimmed {
            if is_unit_separator(child.kind) {
                flush(&mut units, &mut run);
                units.push(RubyPlanUnit {
                    source: RubyPlanUnitSource::Element(child.payload),
                    text: child.text,
                    collapsed: child.collapsed,
                });
                continue;
            }
            let run = run.get_or_insert_with(|| (Vec::new(), String::new(), false));
            run.1.push_str(&child.text);
            run.2 |= child.collapsed;
            if child.kind != RubyChildKind::Whitespace {
                run.0.push(child.payload);
            }
        }
        flush(&mut units, &mut run);
    }

    if units.is_empty() {
        units.push(RubyPlanUnit {
            source: RubyPlanUnitSource::Empty,
            text: String::new(),
            // An empty container "counts as containing one empty base/annotation";
            // a collapsed empty container has nothing to show anyway.
            collapsed: container_collapsed,
        });
    }
    (Some(payload), units)
}

/// Group the interpreted items into segments (§2.3).
fn group_into_segments<P>(items: Vec<GroupingItem<P>>) -> Vec<RubyPlanSegment<P>> {
    let mut segments: Vec<RubyPlanSegment<P>> = Vec::new();
    let mut current: Option<SegmentBuilder<P>> = None;
    // The anonymous base run being accumulated into the current segment.
    let mut base_run: Option<(Vec<P>, String, bool)> = None;

    macro_rules! finish_current {
        () => {{
            flush_base_run(&mut current, &mut base_run);
            if let Some(builder) = current.take() {
                segments.push(builder.finish());
            }
        }};
    }

    for item in items {
        // `item` is moved into the match below; decide the variant first so the
        // `Base | InlinePart` arm does not need to re-inspect the moved value.
        let is_inline = matches!(&item, GroupingItem::InlinePart(_));
        match item {
            GroupingItem::BaseContainer { container, units } => {
                finish_current!();
                let mut builder = SegmentBuilder::new_with_base_container(container);
                builder.base_units = units;
                current = Some(builder);
            },
            GroupingItem::AnnotationContainer {
                container,
                units,
                merge,
            } => {
                flush_base_run(&mut current, &mut base_run);
                let builder = current.get_or_insert_with(SegmentBuilder::new_anonymous);
                // An annotation container always opens a new level, and no
                // further base-ish child may join this segment: bases after
                // annotations start a new segment (§2.3).
                builder.base_container_open = false;
                builder.levels.push(OpenLevel {
                    container,
                    units,
                    open_run: false,
                    merge,
                });
            },
            GroupingItem::WhitespaceBaseContainer(unit) => {
                finish_current!();
                let mut builder = SegmentBuilder::new_anonymous();
                builder.base_units.push(unit);
                // The white space container is closed: following base-ish
                // children start their own container.
                builder.base_container_open = false;
                current = Some(builder);
            },
            GroupingItem::WhitespaceSegment(unit) => {
                finish_current!();
                let mut builder = SegmentBuilder::new_anonymous();
                builder.base_units.push(unit);
                builder.base_container_open = false;
                current = Some(builder);
            },
            GroupingItem::Base(unit) | GroupingItem::InlinePart(unit) => {
                if is_inline {
                    let needs_new_segment = match current.as_ref() {
                        Some(builder) => !builder.base_container_open,
                        None => true,
                    };
                    if needs_new_segment {
                        finish_current!();
                        current = Some(SegmentBuilder::new_anonymous());
                    }
                    let run = base_run.get_or_insert_with(|| (Vec::new(), String::new(), false));
                    match unit.source {
                        RubyPlanUnitSource::Content(payloads) => {
                            run.0.extend(payloads);
                        },
                        _ => {
                            // An absorbed white space unit has no payload.
                        },
                    }
                    run.1.push_str(&unit.text);
                    run.2 |= unit.collapsed;
                } else {
                    // An explicit base unit after annotation containers must
                    // start a new segment (the previous segment is finished
                    // by its annotation containers; §2.3).
                    if current
                        .as_ref()
                        .is_some_and(|builder| !builder.base_container_open)
                    {
                        finish_current!();
                    }
                    flush_base_run(&mut current, &mut base_run);
                    let builder = current.get_or_insert_with(SegmentBuilder::new_anonymous);
                    builder.base_units.push(unit);
                }
            },
            GroupingItem::Annotation(unit) => {
                flush_base_run(&mut current, &mut base_run);
                let builder = current.get_or_insert_with(SegmentBuilder::new_anonymous);
                // Units after the first annotation of this segment belong to
                // an annotation level; a base-ish child that follows starts a
                // new segment (§2.3).
                builder.base_container_open = false;
                push_annotation_unit(builder, unit);
            },
        }
    }

    finish_current!();
    segments
}

/// Fold the accumulated base run units into the current segment's base units.
fn flush_base_run<P>(
    current: &mut Option<SegmentBuilder<P>>,
    base_run: &mut Option<(Vec<P>, String, bool)>,
) {
    if let Some((payloads, text, collapsed)) = base_run.take() {
        if !payloads.is_empty() || !text.is_empty() {
            let builder = current.get_or_insert_with(SegmentBuilder::new_anonymous);
            builder.base_units.push(RubyPlanUnit {
                source: RubyPlanUnitSource::Content(payloads),
                text,
                collapsed,
            });
        }
    }
}

impl<P> SegmentBuilder<P> {
    /// Pair the level's units against the segment's base units and freeze the
    /// segment (§2.3 pairing / §2.1.1 steps 6-7).
    fn finish(mut self) -> RubyPlanSegment<P> {
        // A segment always has at least one column: a zero-base segment
        // "assumes an empty base" (§2.1.1 step 7 / §2.3 degenerate cases).
        if self.base_units.is_empty() {
            self.base_units.push(RubyPlanUnit {
                source: RubyPlanUnitSource::Empty,
                text: String::new(),
                collapsed: false,
            });
        }
        // Extra annotation units pair with trailing empty base units, so the
        // column count grows to fit them (§2.3: "多余 annotation 配空 base").
        let base_count = self.base_units.len();
        let mut column_count = base_count;
        for level in &self.levels {
            if level.units.len() > 1 {
                column_count = column_count.max(level.units.len());
            }
        }
        while self.base_units.len() < column_count {
            self.base_units.push(RubyPlanUnit {
                source: RubyPlanUnitSource::Empty,
                text: String::new(),
                collapsed: false,
            });
        }

        let base_units = &self.base_units;
        let levels = self
            .levels
            .into_iter()
            .map(|level| finish_level(level, base_units))
            .collect();

        RubyPlanSegment {
            base_container: self.base_container,
            base_units: self.base_units,
            levels,
        }
    }
}

/// Whether an annotation's text matches its spanned base text for
/// auto-hiding (§2.3 hiding). The comparison happens on the raw text
/// content, before white space collapsing and text transformation.
fn annotation_matches_base(annotation_text: &str, base_text: &str) -> bool {
    !annotation_text.is_empty() && annotation_text == base_text
}

/// Pair one level's units against the segment's base count, resolving
/// auto-hiding by comparing texts (§2.3).
fn finish_level<P>(level: OpenLevel<P>, base_units: &[RubyPlanUnit<P>]) -> RubyPlanLevel<P> {
    let unit_count = level.units.len();
    let single_anonymous = unit_count == 1 && level.units[0].is_anonymous();
    let base_count = base_units.len();

    let mut cells = Vec::with_capacity(unit_count);
    if single_anonymous {
        // The only annotation of this level is anonymous: it spans the whole
        // segment (§2.3 pairing).
        let mut unit = level
            .units
            .into_iter()
            .next()
            .expect("Checked length above");
        let spanned_text: String = base_units.iter().map(|base| base.text.as_str()).collect();
        let hidden =
            unit.collapsed || (!level.merge && annotation_matches_base(&unit.text, &spanned_text));
        unit.collapsed = false;
        cells.push(RubyPlanCell {
            unit,
            span: 0..base_count,
            hidden,
        });
    } else {
        for (index, mut unit) in level.units.into_iter().enumerate() {
            let spanned_text = base_units
                .get(index)
                .map(|base| base.text.as_str())
                .unwrap_or("");
            let hidden = unit.collapsed
                || (!level.merge && annotation_matches_base(&unit.text, spanned_text));
            unit.collapsed = false;
            cells.push(RubyPlanCell {
                unit,
                span: index..index + 1,
                hidden,
            });
        }
    }

    RubyPlanLevel {
        container: level.container,
        cells,
        merge: level.merge,
    }
}

/// Push one annotation unit into the current level, opening a new anonymous
/// run level when the previous level was an explicit container or absent.
fn push_annotation_unit<P>(builder: &mut SegmentBuilder<P>, unit: RubyPlanUnit<P>) {
    match builder.levels.last_mut() {
        Some(level) if level.open_run => level.units.push(unit),
        _ => {
            builder.levels.push(OpenLevel {
                container: RubyPlanContainer { element: None },
                units: vec![unit],
                open_run: true,
                merge: false,
            });
        },
    }
}
