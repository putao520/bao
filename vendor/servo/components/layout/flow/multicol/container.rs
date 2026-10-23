/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / C2): CSS Multi-column layout — the multi-column
// container formatting context.
//
// [`MultiColContainer`] is the formatting context of a box whose computed style has
// a non-`auto` `column-count` or `column-width`. Such a box always establishes an
// independent formatting context
// (<https://drafts.csswg.org/css-multicol-1/#multi-column-container>), so it is
// carried by `IndependentFormattingContextContents::MultiCol`.
//
// Layout happens in two phases:
//
//  1. Each run of in-flow children between `column-span: all` siblings (a *column
//     row*, see <https://drafts.csswg.org/css-multicol-1/#column-span> and §9 of the
//     multi-column specification) is laid out once at the used column inline size
//     with an indefinite block size, through the ordinary block formatting context
//     machinery.
//  2. The resulting flow is fragmented into columns: the break opportunities are
//     collected from the fragment tree (`flow::multicol::fragment`), the column
//     block size is resolved by balancing or by the definite block size
//     (`flow::multicol::columns`), and each column's fragments are cut out of the
//     flow and placed into an anonymous column box.
//
// A `column-span: all` child is laid out at the full inline size of the container
// and splits the column flow into independent column rows, each balanced on its own.

use app_units::Au;
use malloc_size_of_derive::MallocSizeOf;
use servo_arc::Arc as ServoArc;
use style::Zero;
use style::context::SharedStyleContext;
use style::logical_geometry::WritingMode;
use style::properties::ComputedValues;
use style::values::computed::{ColumnCount, ColumnSpan};
use style::values::generics::length::{LengthPercentageOrAuto, LengthPercentageOrNormal};

use super::columns::{self, UsedColumns};
use super::fragment;
use crate::cell::ArcRefCell;
use crate::context::LayoutContext;
use crate::dom::WeakLayoutBox;
use crate::flow::inline::InlineFormattingContext;
use crate::flow::{
    BlockContainer, BlockFormattingContext, BlockLevelBox, CollapsibleWithParentStartMargin,
};
use crate::formatting_contexts::Baselines;
use crate::fragment_tree::{CollapsedBlockMargins, Fragment, PositioningFragment};
use crate::geom::{LogicalRect, LogicalSides1D, LogicalVec2};
use crate::layout_box_base::IndependentFormattingContextLayoutResult;
use crate::positioned::PositioningContext;
use crate::sizing::{ComputeInlineContentSizes, LazySize};
use crate::style_ext::LayoutStyle;
use crate::{ContainingBlock, ContainingBlockSize};

/// The formatting context of a CSS multi-column container.
#[derive(Debug, MallocSizeOf)]
pub(crate) struct MultiColContainer {
    /// The unfragmented block-level content of the container, in tree order. It is
    /// laid out once per column row and then fragmented into the column boxes.
    contents: BlockContainer,

    /// The style of the multi-column container itself.
    style: ServoArc<ComputedValues>,
}

/// A run of the container's content between two `column-span: all` siblings.
enum ColumnRow<'a> {
    /// Consecutive in-flow block-level children that are fragmented into columns.
    BlockLevelBoxes(Vec<ArcRefCell<BlockLevelBox>>),
    /// The container's whole content is a single inline formatting context (it has
    /// no block-level children), which is fragmented into columns as a whole.
    InlineFormattingContext(&'a InlineFormattingContext),
    /// A `column-span: all` child, laid out at the full inline size of the container.
    Spanner(ArcRefCell<BlockLevelBox>),
}

impl MultiColContainer {
    pub(crate) fn new(contents: BlockFormattingContext, style: ServoArc<ComputedValues>) -> Self {
        Self {
            contents: contents.contents,
            style,
        }
    }

    /// Whether a box with this style is a multi-column container that this module
    /// lays out.
    ///
    /// `column-count` and `column-width` are only enabled when the
    /// `layout.columns.enabled` preference is on, so a container that does not
    /// specify either property never reaches this module.
    ///
    /// Column fragmentation is currently restricted to horizontal writing modes:
    /// the balancing and slicing helpers map the column axis through
    /// `WritingMode::is_horizontal`, and vertical multi-column layout requires
    /// orthogonal-flow support that servo's block layout does not have yet. A
    /// vertical `columns` container therefore keeps the pre-existing (unfragmented)
    /// layout. This is a registered scope boundary, not a silent fallback: the
    /// condition is explicit and deterministic.
    pub(crate) fn is_multicol_style(style: &ComputedValues) -> bool {
        style.get_column().is_multicol() && style.writing_mode.is_horizontal()
    }

    pub(crate) fn repair_style(
        &mut self,
        context: &SharedStyleContext,
        node: &script::layout_dom::ServoLayoutNode,
        new_style: &ServoArc<ComputedValues>,
    ) {
        self.style = new_style.clone();
        self.contents.repair_style(context, node, new_style);
    }

    pub(crate) fn attached_to_tree(&self, layout_box: WeakLayoutBox) {
        self.contents.attached_to_tree(layout_box);
    }

    pub(crate) fn subtree_size(&self) -> usize {
        self.contents.subtree_size()
    }

    #[inline]
    pub(crate) fn layout_style(&self) -> LayoutStyle<'_> {
        LayoutStyle::Default(&self.style)
    }

    pub(crate) fn compute_inline_content_sizes(
        &self,
        layout_context: &LayoutContext,
        constraint_space: &crate::ConstraintSpace,
    ) -> crate::sizing::InlineContentSizesResult {
        self.contents
            .compute_inline_content_sizes(layout_context, constraint_space)
    }

    /// Split the container's content into column rows at the `column-span: all`
    /// children.
    ///
    /// Only in-flow block-level children are considered: floats and absolutely
    /// positioned boxes are out of flow and stay in the column flow of the run they
    /// belong to.
    fn column_rows(&self) -> Vec<ColumnRow<'_>> {
        let mut rows = Vec::new();
        let mut current: Vec<ArcRefCell<BlockLevelBox>> = Vec::new();

        let boxes = match &self.contents {
            BlockContainer::BlockLevelBoxes(boxes) => boxes,
            BlockContainer::InlineFormattingContext(inline_formatting_context) => {
                return vec![ColumnRow::InlineFormattingContext(
                    inline_formatting_context,
                )];
            },
        };

        for child in boxes {
            if is_column_spanner(child) {
                if !current.is_empty() {
                    rows.push(ColumnRow::BlockLevelBoxes(std::mem::take(&mut current)));
                }
                rows.push(ColumnRow::Spanner(child.clone()));
            } else {
                current.push(child.clone());
            }
        }

        if !current.is_empty() {
            rows.push(ColumnRow::BlockLevelBoxes(current));
        }

        rows
    }

    /// The used block size of a column row, given the definite block size of the
    /// container (if any) and the balanced plan.
    fn column_block_size(definite_column_block_size: Option<Au>, plan: &columns::ColumnPlan) -> Au {
        definite_column_block_size.unwrap_or(plan.block_size)
    }

    /// Place one column box into `fragments`.
    ///
    /// The children of the column keep the block coordinates of the column flow, so
    /// the column container's block origin is offset by `-slice.start`, which makes
    /// the first fragment of the column start at the column's own block start.
    fn place_column(
        &self,
        fragments: &mut Vec<Fragment>,
        children: Vec<Fragment>,
        index: u32,
        slice: columns::ColumnSlice,
        row_block_position: Au,
        used_columns: UsedColumns,
        container_containing_block: &ContainingBlock,
    ) {
        let rect = LogicalRect {
            start_corner: LogicalVec2 {
                inline: used_columns.column_inline_offset(index),
                block: row_block_position - slice.start,
            },
            size: LogicalVec2 {
                inline: used_columns.width,
                block: slice.block_size(),
            },
        }
        .as_physical(Some(container_containing_block));

        fragments.push(Fragment::Positioning(PositioningFragment::new_anonymous(
            self.style.clone(),
            rect,
            children,
            false, /* is_line_box */
        )));
    }
}

/// Whether this block-level child spans all columns.
fn is_column_spanner(block_level_box: &ArcRefCell<BlockLevelBox>) -> bool {
    let block_level_box = block_level_box.borrow();
    if !matches!(
        &*block_level_box,
        BlockLevelBox::Independent(_) | BlockLevelBox::SameFormattingContextBlock(_)
    ) {
        // Out-of-flow boxes never participate in the column flow.
        return false;
    }
    block_level_box.with_base(|base| base.style.get_column().column_span == ColumnSpan::All)
}

/// The specified `column-count`, or `None` for `auto`.
fn specified_column_count(style: &ComputedValues) -> Option<u32> {
    match style.get_column().column_count {
        ColumnCount::Integer(count) => Some((count.0).max(1) as u32),
        ColumnCount::Auto => None,
    }
}

/// The specified `column-width`, or `None` for `auto`.
fn specified_column_width(style: &ComputedValues) -> Option<Au> {
    match style.get_column().column_width {
        LengthPercentageOrAuto::Auto => None,
        LengthPercentageOrAuto::LengthPercentage(width) => Some(Au::from(width)),
    }
}

/// The used `column-gap`.
///
/// <https://drafts.csswg.org/css-multicol-1/#column-gap>: `normal` computes to
/// `1em`, i.e. to the font size of the multi-column container.
fn used_column_gap(style: &ComputedValues, available_inline_size: Au) -> Au {
    match style.get_column_gap() {
        LengthPercentageOrNormal::Normal => Au::from(style.get_font().font_size.computed_size()),
        LengthPercentageOrNormal::LengthPercentage(length_percentage) => length_percentage
            .maybe_to_used_value(Some(available_inline_size))
            .unwrap_or_default(),
    }
}

/// The writing mode used by the fragmentation helpers.
///
/// Multi-column layout is restricted to horizontal writing modes in
/// [`MultiColContainer::is_multicol_style`]; this accessor keeps the restriction in
/// one place.
fn fragmentation_writing_mode(style: &ComputedValues) -> WritingMode {
    style.writing_mode
}

/// The outcome of fragmenting one column row into columns.
struct ColumnFlowResult {
    fragments: Vec<Fragment>,
    block_size: Au,
    baselines: Baselines,
    depends_on_block_constraints: bool,
}

impl MultiColContainer {
    /// Lay out the container: resolve the column geometry, lay out and fragment every
    /// column row, and stack the rows (with the spanning boxes in between) in the
    /// block direction.
    pub(crate) fn layout(
        &self,
        layout_context: &LayoutContext,
        positioning_context: &mut PositioningContext,
        containing_block: &ContainingBlock,
    ) -> IndependentFormattingContextLayoutResult {
        let style = &self.style;
        let writing_mode = fragmentation_writing_mode(style);
        debug_assert!(
            writing_mode.is_horizontal(),
            "Multi-column layout requires a horizontal writing mode"
        );

        let available_inline_size = containing_block.size.inline;
        let gap = used_column_gap(style, available_inline_size);
        let used_columns = columns::resolve_used_columns(
            available_inline_size,
            specified_column_count(style),
            specified_column_width(style),
            gap,
        );

        // The containing block of the column content: the used column inline size,
        // with the container's block size constraint so that percentages resolve
        // against the multi-column container's content box.
        let column_containing_block = ContainingBlock {
            size: ContainingBlockSize {
                inline: used_columns.width,
                block: containing_block.size.block,
            },
            style,
        };

        // The containing block used to position the column boxes and the spanning
        // boxes inside the container's content box.
        let container_containing_block = ContainingBlock {
            size: ContainingBlockSize {
                inline: available_inline_size,
                block: containing_block.size.block,
            },
            style,
        };

        // A definite block size fixes the column block size; otherwise the columns
        // are balanced. (`column-fill` is not available in servo's property set, see
        // the module documentation of `flow::multicol`.)
        let definite_column_block_size = containing_block.size.block.to_definite();

        let mut fragments = Vec::new();
        let mut block_position = Au::zero();
        let mut depends_on_block_constraints = false;
        let mut baselines = Baselines::default();

        for row in self.column_rows() {
            let row_block_position = block_position;
            let result = match row {
                ColumnRow::BlockLevelBoxes(boxes) => {
                    let contains_floats = boxes.iter().any(|box_| box_.borrow().contains_floats());
                    let formatting_context = BlockFormattingContext {
                        contents: BlockContainer::BlockLevelBoxes(boxes),
                        contains_floats,
                    };
                    let result = formatting_context.layout(
                        layout_context,
                        positioning_context,
                        &column_containing_block,
                        &LazySize::intrinsic(),
                        None, /* base */
                    );
                    self.fragment_column_flow(
                        result,
                        used_columns,
                        definite_column_block_size,
                        row_block_position,
                        &container_containing_block,
                        writing_mode,
                    )
                },
                ColumnRow::InlineFormattingContext(inline_formatting_context) => {
                    let result = inline_formatting_context.layout(
                        layout_context,
                        positioning_context,
                        &column_containing_block,
                        None, /* sequential_layout_state */
                        CollapsibleWithParentStartMargin(false),
                        LogicalSides1D::new(false, false),
                    );
                    self.fragment_column_flow(
                        result,
                        used_columns,
                        definite_column_block_size,
                        row_block_position,
                        &container_containing_block,
                        writing_mode,
                    )
                },
                ColumnRow::Spanner(child) => {
                    let contains_floats = child.borrow().contains_floats();
                    let formatting_context = BlockFormattingContext {
                        contents: BlockContainer::BlockLevelBoxes(vec![child]),
                        contains_floats,
                    };
                    let result = formatting_context.layout(
                        layout_context,
                        positioning_context,
                        &container_containing_block,
                        &LazySize::intrinsic(),
                        None, /* base */
                    );

                    let block_size = result.content_block_size.max(Au::zero());
                    let rect = LogicalRect {
                        start_corner: LogicalVec2 {
                            inline: Au::zero(),
                            block: row_block_position,
                        },
                        size: LogicalVec2 {
                            inline: available_inline_size,
                            block: block_size,
                        },
                    }
                    .as_physical(Some(&container_containing_block));
                    let fragment = Fragment::Positioning(PositioningFragment::new_anonymous(
                        self.style.clone(),
                        rect,
                        result.fragments,
                        false, /* is_line_box */
                    ));

                    ColumnFlowResult {
                        fragments: vec![fragment],
                        block_size,
                        baselines: result.baselines.offset(row_block_position),
                        depends_on_block_constraints: result.depends_on_block_constraints,
                    }
                },
            };

            fragments.extend(result.fragments);
            block_position += result.block_size;
            depends_on_block_constraints |= result.depends_on_block_constraints;
            // The first baseline set comes from the first row that has one; the last
            // baseline set is taken from the last row that has one.
            if baselines.first.is_none() {
                baselines.first = result.baselines.first;
            }
            if result.baselines.last.is_some() {
                baselines.last = result.baselines.last;
            }
        }

        IndependentFormattingContextLayoutResult {
            fragments,
            content_block_size: block_position,
            collapsible_margins_in_children: CollapsedBlockMargins::zero(),
            content_inline_size_for_table: None,
            baselines,
            depends_on_block_constraints,
            specific_layout_info: None,
        }
    }

    /// Fragment one laid-out column row into its column boxes.
    ///
    /// `result` must be the layout of the row's content at the used column inline
    /// size with an indefinite block size.
    fn fragment_column_flow(
        &self,
        result: IndependentFormattingContextLayoutResult,
        used_columns: UsedColumns,
        definite_column_block_size: Option<Au>,
        row_block_position: Au,
        container_containing_block: &ContainingBlock,
        writing_mode: WritingMode,
    ) -> ColumnFlowResult {
        let IndependentFormattingContextLayoutResult {
            fragments: mut flow_fragments,
            depends_on_block_constraints,
            baselines,
            ..
        } = result;

        // The block extent covered by columns is the end of the last top-level
        // fragment, not the flow's content block size: the latter also includes
        // trailing (possibly collapsed-through) margins, which must not create a
        // trailing, empty column.
        let content_end = fragment::flow_block_end(&flow_fragments, writing_mode).max(Au::zero());
        let mut break_offsets = Vec::new();
        break_offsets.push(Au::zero());
        fragment::collect_break_offsets(
            &flow_fragments,
            Au::zero(),
            writing_mode,
            &mut break_offsets,
        );
        break_offsets.push(content_end);
        break_offsets.sort_unstable();
        break_offsets.dedup();

        let plan = match definite_column_block_size {
            // A definite block size caps the column block size. `column-fill` is
            // Gecko-only in stylo and therefore cannot be read here, so the initial
            // value (`balance`) is approximated: the content is balanced first, and
            // only when the balanced columns would be taller than the definite block
            // size are the columns filled to that block size (additional columns then
            // overflow the container in the inline direction, as required by
            // <https://drafts.csswg.org/css-multicol-1/#overflow-columns>).
            Some(column_block_size) => {
                let balanced =
                    columns::plan_balanced(&break_offsets, content_end, used_columns.count);
                if balanced.block_size <= column_block_size {
                    balanced
                } else {
                    columns::plan_filled(&break_offsets, content_end, column_block_size)
                }
            },
            None => columns::plan_balanced(&break_offsets, content_end, used_columns.count),
        };
        let column_block_size = Self::column_block_size(definite_column_block_size, &plan);

        let mut fragments = Vec::with_capacity(plan.slices.len());
        for (index, slice) in plan.slices.iter().enumerate() {
            let children = fragment::take_fragment_range(
                &mut flow_fragments,
                slice.start,
                slice.end,
                writing_mode,
            );
            self.place_column(
                &mut fragments,
                children,
                index as u32,
                *slice,
                row_block_position,
                used_columns,
                container_containing_block,
            );
        }

        ColumnFlowResult {
            fragments,
            block_size: column_block_size,
            baselines: baselines.offset(row_block_position),
            depends_on_block_constraints,
        }
    }
}
