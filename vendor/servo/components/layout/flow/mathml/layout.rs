/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The MathML Core layout algorithms.
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//!
//! Every function here works in logical coordinates whose block axis grows
//! towards line-under, which is the convention used by the rest of layout. A
//! math item's geometry is described by three numbers: its inline size, and
//! its ascent (distance from the alphabetic baseline towards line-over) and
//! descent (towards line-under). Fragments are positioned relative to the
//! parent content box, as everywhere else in layout.

use std::sync::Arc;

use app_units::Au;
use fonts::FontMetrics;
use style::Zero;
use style::properties::ComputedValues;

use super::constants::{MathConstants, em};
use super::dictionary::{self, OperatorForm};
use super::{MathChild, MathElementKind, MathMlContainer};
use crate::ContainingBlock;
use crate::context::LayoutContext;
use crate::fragment_tree::{BaseFragmentInfo, BoxFragment, Fragment, MathRule};
use crate::geom::{
    LogicalRect, LogicalSides, LogicalVec2, PhysicalPoint, PhysicalRect, PhysicalSides,
    PhysicalSize, ToLogical,
};
use crate::positioned::{PositioningContext, PositioningContextLength};

/// The result of laying out the math content of one MathML element.
pub(crate) struct MathLayoutResult {
    /// The natural inline size of the content.
    pub inline_size: Au,
    /// Ink ascent above the element's alphabetic baseline.
    pub ascent: Au,
    /// Descent below the element's alphabetic baseline.
    pub descent: Au,
    /// The child fragments, positioned relative to the content box origin.
    pub fragments: Vec<Fragment>,
}

/// State shared by every step of one math layout recursion.
struct MathLayoutContext<'a> {
    layout_context: &'a LayoutContext<'a>,
    containing_block: &'a ContainingBlock<'a>,
    constants: MathConstants,
}

/// The laid out geometry of one child, before it is positioned.
struct ChildLayout {
    fragment: Arc<BoxFragment>,
    /// The child's padding, border and margin sums.
    pbm: LogicalSides<Au>,
    content_size: LogicalVec2<Au>,
    /// Ascent of the child's margin box relative to its own alphabetic
    /// baseline.
    ascent: Au,
    /// Descent of the child's margin box relative to its own alphabetic
    /// baseline.
    descent: Au,
    /// The absolutely positioned descendants found while laying out the child.
    positioning_context: Option<PositioningContext>,
}

impl ChildLayout {
    /// The inline size of the child's margin box.
    fn inline_size(&self) -> Au {
        self.pbm.inline_start + self.content_size.inline + self.pbm.inline_end
    }

    /// Position the child so that its alphabetic baseline sits at
    /// `baseline_position`, with its margin box starting at `inline_position`.
    fn place(
        &mut self,
        inline_position: Au,
        baseline_position: Au,
        containing_block: &ContainingBlock,
        positioning_context: &mut PositioningContext,
    ) -> Fragment {
        let start_corner = LogicalVec2 {
            inline: inline_position + self.pbm.inline_start,
            block: baseline_position - self.ascent + self.pbm.block_start,
        };
        let rect = LogicalRect {
            start_corner,
            size: self.content_size,
        }
        .as_physical(Some(containing_block));
        self.fragment.base.set_rect_origin(rect.origin);

        if let Some(mut child_positioning_context) = self.positioning_context.take() {
            child_positioning_context.adjust_static_position_of_hoisted_fragments_with_offset(
                &rect.origin.to_vector(),
                PositioningContextLength::zero(),
            );
            positioning_context.append(child_positioning_context);
        }

        Fragment::Box(self.fragment.clone())
    }
}

/// Lay out the math content of `container`.
pub(crate) fn layout_container(
    container: &MathMlContainer,
    layout_context: &LayoutContext,
    positioning_context: &mut PositioningContext,
    containing_block: &ContainingBlock,
    style: &ComputedValues,
) -> MathLayoutResult {
    let constants = MathConstants::resolve(
        font_size(style),
        first_font_metrics(layout_context, style).as_deref(),
    );
    let context = MathLayoutContext {
        layout_context,
        containing_block,
        constants,
    };

    match container.kind {
        MathElementKind::Fraction => {
            layout_fraction(container, &context, positioning_context, style)
        },
        MathElementKind::Sub
        | MathElementKind::Sup
        | MathElementKind::SubSup
        | MathElementKind::Under
        | MathElementKind::Over
        | MathElementKind::UnderOver => {
            layout_scripts(container, &context, positioning_context, style)
        },
        MathElementKind::MultiScripts => {
            layout_multiscripts(container, &context, positioning_context, style)
        },
        MathElementKind::Table => layout_table(container, &context, positioning_context, style),
        MathElementKind::Space => layout_space(container, &context, style),
        MathElementKind::Row | MathElementKind::Token | MathElementKind::Prescripts => {
            layout_row(container, &context, positioning_context, style)
        },
    }
}

/// The font size of a style, as used for the em-relative MathML constants.
fn font_size(style: &ComputedValues) -> Au {
    style.get_font().font_size.computed_size().into()
}

/// The metrics of the first available font for `style`, if any font is
/// available.
fn first_font_metrics(
    layout_context: &LayoutContext,
    style: &ComputedValues,
) -> Option<Arc<FontMetrics>> {
    let font_group = layout_context.font_context.font_group(style.clone_font());
    let font = font_group.first(&layout_context.font_context)?;
    Some(font.metrics().clone())
}

/// Lay out one child of a math container at its fit-content size.
fn layout_child(
    child: &MathChild,
    context: &MathLayoutContext,
    _positioning_context: &mut PositioningContext,
) -> ChildLayout {
    let mut child_positioning_context = PositioningContext::default();
    let result = child.with_context(|formatting_context| {
        formatting_context.layout_float_or_atomic_inline(
            context.layout_context,
            &mut child_positioning_context,
            context.containing_block,
        )
    });

    let writing_mode = context.containing_block.style.writing_mode;
    let content_size = result.fragment.content_rect().size.to_logical(writing_mode);
    let baseline = result
        .baselines
        .first
        .unwrap_or(content_size.block)
        .min(content_size.block)
        .max(Au::zero());
    let ascent = result.pbm_sums.block_start + baseline;
    let descent = result.pbm_sums.block_end + (content_size.block - baseline);

    ChildLayout {
        fragment: Arc::new(result.fragment),
        pbm: result.pbm_sums,
        content_size,
        ascent,
        descent,
        positioning_context: Some(child_positioning_context),
    }
}

/// The form of an operator child, as inferred from its position among its
/// siblings and overridden by an explicit `form` attribute.
///
/// <https://w3c.github.io/mathml-core/#dfn-form>
fn operator_form(
    container: &MathMlContainer,
    child: &MathChild,
    index: usize,
    child_count: usize,
) -> OperatorForm {
    let operator = match child.operator {
        Some(ref operator) => operator,
        None => return OperatorForm::Infix,
    };
    if let Some(explicit_form) = operator.explicit_form {
        return explicit_form;
    }

    let is_first = index == 0;
    let is_last = index + 1 == child_count;
    let in_script_position = container.kind.is_scripted() && !is_first;
    // `default_form` distinguishes "the only child" (infix) from "the first of
    // several" (prefix). A grouping element with a single child still uses
    // `infix` unless the child is in a script position.
    let default_form = if child_count > 1 {
        dictionary::default_form(is_first, is_last, in_script_position)
    } else {
        dictionary::default_form(false, false, in_script_position)
    };
    default_form
}

/// The horizontal space taken by an operator child before and after its
/// margin box.
fn operator_spaces(
    container: &MathMlContainer,
    child: &MathChild,
    index: usize,
    child_count: usize,
) -> (Au, Au) {
    let Some(ref operator) = child.operator else {
        return (Au::zero(), Au::zero());
    };
    // Operator spacing only applies when the parent adds space, which is the
    // case for grouping elements (`mrow`, `math`, …).
    if !container.kind.is_grouping_element() {
        return (Au::zero(), Au::zero());
    }
    let form = operator_form(container, child, index, child_count);
    let properties = operator.properties(form);
    let child_font_size = child.with_context(|context| font_size(context.style()));
    (
        properties.lspace.resolve(child_font_size),
        properties.rspace.resolve(child_font_size),
    )
}

/// Lay out a row: children side by side, alphabetic baselines aligned.
///
/// <https://w3c.github.io/mathml-core/#row-mrow>
fn layout_row(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    _style: &ComputedValues,
) -> MathLayoutResult {
    let child_count = container.children.len();
    let mut children = Vec::with_capacity(child_count);
    let mut ascent = Au::zero();
    let mut descent = Au::zero();

    for (index, child) in container.children.iter().enumerate() {
        let layout = layout_child(child, context, positioning_context);
        let (lspace, rspace) = operator_spaces(container, child, index, child_count);
        ascent = ascent.max(layout.ascent);
        descent = descent.max(layout.descent);
        children.push((layout, lspace, rspace));
    }

    // The math axis is the baseline of a row whose content has no ascent: an
    // empty expression keeps a non-zero height so that surrounding layout does
    // not collapse it.
    if children.is_empty() {
        let axis_height = context.constants.axis_height;
        return MathLayoutResult {
            inline_size: Au::zero(),
            ascent: axis_height,
            descent: axis_height,
            fragments: Vec::new(),
        };
    }

    let mut inline_position = Au::zero();
    let mut fragments = Vec::with_capacity(children.len());
    for (mut layout, lspace, rspace) in children {
        let fragment = layout.place(
            inline_position + lspace,
            ascent,
            context.containing_block,
            positioning_context,
        );
        inline_position += lspace + layout.inline_size() + rspace;
        fragments.push(fragment);
    }

    MathLayoutResult {
        inline_size: inline_position,
        ascent,
        descent,
        fragments,
    }
}

/// Lay out an `mspace` from its `width`/`height`/`depth` attributes.
///
/// <https://w3c.github.io/mathml-core/#space-mspace>
fn layout_space(
    container: &MathMlContainer,
    _context: &MathLayoutContext,
    style: &ComputedValues,
) -> MathLayoutResult {
    let dimensions = container.space;
    let font_size = font_size(style);
    let width = dimensions
        .width
        .map(|width| width.resolve(font_size))
        .unwrap_or_else(Au::zero);
    let height = dimensions
        .height
        .map(|height| height.resolve(font_size))
        .unwrap_or_else(Au::zero)
        .max(Au::zero());
    let depth = dimensions
        .depth
        .map(|depth| depth.resolve(font_size))
        .unwrap_or_else(Au::zero)
        .max(Au::zero());
    MathLayoutResult {
        inline_size: width,
        ascent: height,
        descent: depth,
        fragments: Vec::new(),
    }
}

/// Lay out an `mfrac`.
///
/// <https://w3c.github.io/mathml-core/#fraction-with-nonzero-line-thickness>
fn layout_fraction(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    style: &ComputedValues,
) -> MathLayoutResult {
    if container.children.len() != 2 {
        // MathML Core falls back to `mrow` layout when the numerator or the
        // denominator is missing.
        return layout_row(container, context, positioning_context, style);
    }

    let mut numerator = layout_child(&container.children[0], context, positioning_context);
    let mut denominator = layout_child(&container.children[1], context, positioning_context);

    let constants = &context.constants;
    let axis = constants.axis_height;
    let rule = constants.fraction_rule_thickness;
    let half_rule = rule.scale_by(0.5);

    let numerator_shift = constants
        .fraction_numerator_shift_up
        .max(axis + half_rule + constants.fraction_numerator_gap_min + numerator.descent);
    let denominator_shift = constants
        .fraction_denominator_shift_down
        .max(half_rule + constants.fraction_denominator_gap_min + denominator.ascent - axis);

    let numerator_baseline = -numerator_shift;
    let denominator_baseline = denominator_shift;

    let inline_size = numerator.inline_size().max(denominator.inline_size());
    let ascent = (numerator_shift + numerator.ascent)
        .max(denominator.ascent - denominator_shift)
        .max(axis + half_rule);
    let descent = (numerator.descent - numerator_shift)
        .max(denominator_shift + denominator.descent)
        .max(half_rule - axis)
        .max(Au::zero());

    let numerator_x = centered_offset(inline_size, numerator.inline_size());
    let denominator_x = centered_offset(inline_size, denominator.inline_size());
    let mut fragments = vec![
        numerator.place(
            numerator_x,
            ascent + numerator_baseline,
            context.containing_block,
            positioning_context,
        ),
        denominator.place(
            denominator_x,
            ascent + denominator_baseline,
            context.containing_block,
            positioning_context,
        ),
    ];

    // The fraction bar is not expressible as a CSS border or background of the
    // `mfrac` box: it is a solid rule painted at the math axis. The rule is
    // carried on an anonymous fragment (BAO PATCH, REQ-BRW-051 C2) and painted
    // by the display list builder; anonymous fragments are invisible to DOM
    // box queries, which keep pointing at the `mfrac` element itself.
    let bar = solid_rule_fragment(
        style,
        inline_size,
        ascent,
        descent,
        ascent - axis - half_rule,
        rule,
    );
    fragments.push(bar);

    MathLayoutResult {
        inline_size,
        ascent,
        descent,
        fragments,
    }
}

/// Build an anonymous fragment that paints a single solid rule.
///
/// `rule_block_offset` is the rule's offset from the top of the fragment's
/// content box; the fragment's content box spans `inline_size` by
/// `ascent + descent` starting at the parent content box origin.
fn solid_rule_fragment(
    style: &ComputedValues,
    inline_size: Au,
    ascent: Au,
    descent: Au,
    rule_block_offset: Au,
    rule_thickness: Au,
) -> Fragment {
    let base_fragment_info = BaseFragmentInfo::anonymous();
    let content_rect = PhysicalRect::new(
        PhysicalPoint::zero(),
        PhysicalSize::new(inline_size, ascent + descent),
    );
    let fragment = BoxFragment::new(
        base_fragment_info,
        style.to_arc(),
        Vec::new(),
        content_rect,
        PhysicalSides::zero(),
        PhysicalSides::zero(),
        PhysicalSides::zero(),
        None,
    );
    let rule_color = style.get_inherited_text().color;
    fragment.set_math_rules(vec![MathRule {
        rect: PhysicalRect::new(
            PhysicalPoint::new(Au::zero(), rule_block_offset),
            PhysicalSize::new(inline_size, rule_thickness),
        ),
        color: rule_color,
    }]);
    Fragment::Box(Arc::new(fragment))
}

/// The offset that centers a box of size `inner` inside `outer`.
#[inline]
fn centered_offset(outer: Au, inner: Au) -> Au {
    if outer > inner {
        (outer - inner).scale_by(0.5)
    } else {
        Au::zero()
    }
}

/// The script schemata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScriptSchema {
    Sub,
    Sup,
    SubSup,
    Under,
    Over,
    UnderOver,
}

impl ScriptSchema {
    fn from_kind(kind: MathElementKind) -> Option<Self> {
        Some(match kind {
            MathElementKind::Sub => Self::Sub,
            MathElementKind::Sup => Self::Sup,
            MathElementKind::SubSup => Self::SubSup,
            MathElementKind::Under => Self::Under,
            MathElementKind::Over => Self::Over,
            MathElementKind::UnderOver => Self::UnderOver,
            _ => return None,
        })
    }

    fn uses_a_script_position(self) -> bool {
        matches!(self, Self::Sub | Self::Sup | Self::SubSup)
    }
}

/// Lay out one of the script/limit schemata.
///
/// * <https://w3c.github.io/mathml-core/#subscript-msub>
/// * <https://w3c.github.io/mathml-core/#superscript-msup>
/// * <https://w3c.github.io/mathml-core/#subscriptsuperscript-msubsup>
/// * <https://w3c.github.io/mathml-core/#underscript-munder>
/// * <https://w3c.github.io/mathml-core/#overscript-mover>
/// * <https://w3c.github.io/mathml-core/#underscriptoverscript-munderover>
fn layout_scripts(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    style: &ComputedValues,
) -> MathLayoutResult {
    let Some(schema) = ScriptSchema::from_kind(container.kind) else {
        return layout_row(container, context, positioning_context, style);
    };
    let required_children = match schema {
        ScriptSchema::SubSup | ScriptSchema::UnderOver => 3,
        _ => 2,
    };
    if container.children.len() < required_children {
        // Missing scripts fall back to `mrow` layout.
        return layout_row(container, context, positioning_context, style);
    }

    // `munder`/`mover`/`munderover` use the sub/superscript algorithms when
    // their base is an embellished `movablelimits` operator in compact math
    // style. Embellished-operator detection is limited to a direct `<mo>`
    // base here (see the module docs).
    let base_is_movable_limits_operator = container.children[0]
        .operator
        .map(|operator| operator.properties(OperatorForm::Infix).movablelimits)
        .unwrap_or(false);
    let math_style = style.get_font().math_style;
    if !schema.uses_a_script_position()
        && base_is_movable_limits_operator
        && dictionary::math_style_is_compact(math_style)
    {
        let schema = match schema {
            ScriptSchema::Under => ScriptSchema::Sub,
            ScriptSchema::Over => ScriptSchema::Sup,
            _ => ScriptSchema::SubSup,
        };
        return layout_scripts_with_schema(container, context, positioning_context, schema);
    }

    if schema.uses_a_script_position() {
        return layout_scripts_with_schema(container, context, positioning_context, schema);
    }

    layout_limits(container, context, positioning_context, schema)
}

/// The `msub`/`msup`/`msubsup` algorithms.
fn layout_scripts_with_schema(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    schema: ScriptSchema,
) -> MathLayoutResult {
    let mut base = layout_child(&container.children[0], context, positioning_context);
    let constants = &context.constants;

    let mut subscript = match schema {
        ScriptSchema::Sub | ScriptSchema::SubSup => Some(layout_child(
            &container.children[1],
            context,
            positioning_context,
        )),
        _ => None,
    };
    let mut superscript = match schema {
        ScriptSchema::Sup => Some(layout_child(
            &container.children[1],
            context,
            positioning_context,
        )),
        ScriptSchema::SubSup => Some(layout_child(
            &container.children[2],
            context,
            positioning_context,
        )),
        _ => None,
    };

    let mut subscript_shift = subscript
        .as_ref()
        .map(|subscript| {
            constants
                .subscript_shift_down
                .max(subscript.ascent - constants.subscript_top_max)
                .max(constants.subscript_baseline_drop_min + base.descent)
        })
        .unwrap_or_else(Au::zero);
    let mut superscript_shift = superscript
        .as_ref()
        .map(|superscript| {
            constants
                .superscript_shift_up
                .max(constants.superscript_bottom_min + superscript.descent)
                .max(base.ascent - constants.superscript_baseline_drop_max)
        })
        .unwrap_or_else(Au::zero);

    // `msubsup` keeps the two scripts apart.
    if let (Some(subscript), Some(superscript)) = (subscript.as_ref(), superscript.as_ref()) {
        let gap = (subscript_shift - subscript.ascent) + (superscript_shift - superscript.descent);
        if gap < constants.sub_superscript_gap_min {
            let mut delta = constants.superscript_bottom_max_with_subscript
                - (superscript_shift - superscript.descent);
            if delta > Au::zero() {
                delta = delta.min(constants.sub_superscript_gap_min - gap);
                superscript_shift += delta;
            }
            let gap =
                (subscript_shift - subscript.ascent) + (superscript_shift - superscript.descent);
            let delta = constants.sub_superscript_gap_min - gap;
            if delta > Au::zero() {
                subscript_shift += delta;
            }
        }
    }

    let scripts_inline_size = subscript
        .as_ref()
        .map(|subscript| subscript.inline_size())
        .unwrap_or_else(Au::zero)
        .max(
            superscript
                .as_ref()
                .map(|superscript| superscript.inline_size())
                .unwrap_or_else(Au::zero),
        );
    let inline_size = base.inline_size() + constants.space_after_script + scripts_inline_size;

    let ascent = base.ascent.max(
        superscript
            .as_ref()
            .map(|superscript| superscript_shift + superscript.ascent)
            .unwrap_or_else(Au::zero),
    );
    let descent = base.descent.max(
        subscript
            .as_ref()
            .map(|subscript| subscript_shift + subscript.descent)
            .unwrap_or_else(Au::zero),
    );

    let mut fragments = Vec::with_capacity(3);
    fragments.push(base.place(
        Au::zero(),
        ascent,
        context.containing_block,
        positioning_context,
    ));
    let scripts_inline_start = base.inline_size() + constants.space_after_script;
    if let Some(subscript) = subscript.as_mut() {
        fragments.push(subscript.place(
            scripts_inline_start,
            ascent + subscript_shift,
            context.containing_block,
            positioning_context,
        ));
    }
    if let Some(superscript) = superscript.as_mut() {
        fragments.push(superscript.place(
            scripts_inline_start,
            ascent - superscript_shift,
            context.containing_block,
            positioning_context,
        ));
    }

    MathLayoutResult {
        inline_size,
        ascent,
        descent,
        fragments,
    }
}

/// The `munder`/`mover`/`munderover` stack algorithms.
///
/// <https://w3c.github.io/mathml-core/#underscriptoverscript-munderover>
fn layout_limits(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    schema: ScriptSchema,
) -> MathLayoutResult {
    let constants = &context.constants;
    let mut base = layout_child(&container.children[0], context, positioning_context);
    let base_is_large_operator = container.children[0]
        .operator
        .map(|operator| operator.properties(OperatorForm::Infix).largeop)
        .unwrap_or(false);

    let mut under = match schema {
        ScriptSchema::Under | ScriptSchema::UnderOver => Some(layout_child(
            &container.children[1],
            context,
            positioning_context,
        )),
        _ => None,
    };
    let mut over = match schema {
        ScriptSchema::Over => Some(layout_child(
            &container.children[1],
            context,
            positioning_context,
        )),
        ScriptSchema::UnderOver => Some(layout_child(
            &container.children[2],
            context,
            positioning_context,
        )),
        _ => None,
    };

    // The gap between the base and its limits comes from the limit constants
    // for large operators and from the (under/over)bar constants otherwise.
    let under_gap = if base_is_large_operator {
        constants.lower_limit_gap_min
    } else {
        constants.underbar_vertical_gap
    };
    let over_gap = if base_is_large_operator {
        constants.upper_limit_gap_min
    } else {
        constants.overbar_vertical_gap
    };

    let under_shift = under
        .as_ref()
        .map(|under| {
            if base_is_large_operator {
                (base.descent + under_gap + under.ascent)
                    .max(constants.lower_limit_baseline_drop_min)
            } else {
                base.descent + under_gap + under.ascent
            }
        })
        .unwrap_or_else(Au::zero);
    let over_shift = over
        .as_ref()
        .map(|over| {
            if base_is_large_operator {
                (base.ascent + over_gap + over.descent).max(constants.upper_limit_baseline_rise_min)
            } else {
                base.ascent + over_gap + over.descent
            }
        })
        .unwrap_or_else(Au::zero);

    let inline_size = base
        .inline_size()
        .max(
            under
                .as_ref()
                .map(|under| under.inline_size())
                .unwrap_or_else(Au::zero),
        )
        .max(
            over.as_ref()
                .map(|over| over.inline_size())
                .unwrap_or_else(Au::zero),
        );

    let ascent = base.ascent.max(
        over.as_ref()
            .map(|over| over_shift + over.ascent)
            .unwrap_or_else(Au::zero),
    );
    let descent = base.descent.max(
        under
            .as_ref()
            .map(|under| under_shift + under.descent)
            .unwrap_or_else(Au::zero),
    );

    let mut fragments = Vec::with_capacity(3);
    fragments.push(base.place(
        centered_offset(inline_size, base.inline_size()),
        ascent,
        context.containing_block,
        positioning_context,
    ));
    if let Some(under) = under.as_mut() {
        let x = centered_offset(inline_size, under.inline_size());
        fragments.push(under.place(
            x,
            ascent + under_shift,
            context.containing_block,
            positioning_context,
        ));
    }
    if let Some(over) = over.as_mut() {
        let x = centered_offset(inline_size, over.inline_size());
        fragments.push(over.place(
            x,
            ascent - over_shift,
            context.containing_block,
            positioning_context,
        ));
    }

    MathLayoutResult {
        inline_size,
        ascent,
        descent,
        fragments,
    }
}

/// One (subscript, superscript) pair of an `mmultiscripts` expression.
struct ScriptPair {
    subscript: Option<ChildLayout>,
    superscript: Option<ChildLayout>,
    subscript_shift: Au,
    superscript_shift: Au,
    inline_size: Au,
}

/// Lay out one prescript/postscript pair of an `mmultiscripts` expression.
fn layout_script_pair(
    subscript: Option<&MathChild>,
    superscript: Option<&MathChild>,
    base: &ChildLayout,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
) -> ScriptPair {
    let constants = &context.constants;
    let subscript = subscript.map(|child| layout_child(child, context, positioning_context));
    let superscript = superscript.map(|child| layout_child(child, context, positioning_context));
    let subscript_shift = subscript
        .as_ref()
        .map(|subscript| {
            constants
                .subscript_shift_down
                .max(subscript.ascent - constants.subscript_top_max)
                .max(constants.subscript_baseline_drop_min + base.descent)
        })
        .unwrap_or_else(Au::zero);
    let superscript_shift = superscript
        .as_ref()
        .map(|superscript| {
            constants
                .superscript_shift_up
                .max(constants.superscript_bottom_min + superscript.descent)
                .max(base.ascent - constants.superscript_baseline_drop_max)
        })
        .unwrap_or_else(Au::zero);
    let inline_size = subscript
        .as_ref()
        .map(|subscript| subscript.inline_size())
        .unwrap_or_else(Au::zero)
        .max(
            superscript
                .as_ref()
                .map(|superscript| superscript.inline_size())
                .unwrap_or_else(Au::zero),
        );
    ScriptPair {
        subscript,
        superscript,
        subscript_shift,
        superscript_shift,
        inline_size,
    }
}

/// Lay out an `mmultiscripts`.
///
/// <https://w3c.github.io/mathml-core/#prescripts-and-tensor-indices-mmultiscripts>
fn layout_multiscripts(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    style: &ComputedValues,
) -> MathLayoutResult {
    if container.children.is_empty() {
        return layout_row(container, context, positioning_context, style);
    }

    let constants = &context.constants;
    let prescripts_start = container
        .prescripts_start
        .unwrap_or(container.children.len());

    let mut base = layout_child(&container.children[0], context, positioning_context);

    let mut postscript_pairs = Vec::new();
    let mut index = 1;
    while index < prescripts_start {
        postscript_pairs.push(layout_script_pair(
            container.children.get(index),
            container.children.get(index + 1),
            &base,
            context,
            positioning_context,
        ));
        index += 2;
    }

    let mut prescript_pairs = Vec::new();
    if container.prescripts_start.is_some() {
        let mut index = prescripts_start;
        while index < container.children.len() {
            prescript_pairs.push(layout_script_pair(
                container.children.get(index),
                container.children.get(index + 1),
                &base,
                context,
                positioning_context,
            ));
            index += 2;
        }
    }

    let space_after_script = constants.space_after_script;
    let prescripts_inline_size = prescript_pairs.iter().fold(Au::zero(), |size, pair| {
        size + pair.inline_size + space_after_script
    });
    let postscripts_inline_size = postscript_pairs.iter().fold(Au::zero(), |size, pair| {
        size + pair.inline_size + space_after_script
    });
    let inline_size = prescripts_inline_size + base.inline_size() + postscripts_inline_size;

    let mut ascent = base.ascent;
    let mut descent = base.descent;
    for pair in prescript_pairs.iter().chain(postscript_pairs.iter()) {
        if let Some(superscript) = pair.superscript.as_ref() {
            ascent = ascent.max(pair.superscript_shift + superscript.ascent);
        }
        if let Some(subscript) = pair.subscript.as_ref() {
            descent = descent.max(pair.subscript_shift + subscript.descent);
        }
    }

    let mut fragments = Vec::with_capacity(2 + prescript_pairs.len() + postscript_pairs.len());

    // Prescripts are right-aligned against the start of the base.
    let mut inline_position = Au::zero();
    for mut pair in prescript_pairs {
        let pair_start = inline_position;
        if let Some(subscript) = pair.subscript.as_mut() {
            let x = pair_start + (pair.inline_size - subscript.inline_size());
            fragments.push(subscript.place(
                x,
                ascent + pair.subscript_shift,
                context.containing_block,
                positioning_context,
            ));
        }
        if let Some(superscript) = pair.superscript.as_mut() {
            let x = pair_start + (pair.inline_size - superscript.inline_size());
            fragments.push(superscript.place(
                x,
                ascent - pair.superscript_shift,
                context.containing_block,
                positioning_context,
            ));
        }
        inline_position += pair.inline_size + space_after_script;
    }

    fragments.push(base.place(
        inline_position,
        ascent,
        context.containing_block,
        positioning_context,
    ));
    inline_position += base.inline_size();

    for mut pair in postscript_pairs {
        if let Some(subscript) = pair.subscript.as_mut() {
            fragments.push(subscript.place(
                inline_position,
                ascent + pair.subscript_shift,
                context.containing_block,
                positioning_context,
            ));
        }
        if let Some(superscript) = pair.superscript.as_mut() {
            fragments.push(superscript.place(
                inline_position,
                ascent - pair.superscript_shift,
                context.containing_block,
                positioning_context,
            ));
        }
        inline_position += pair.inline_size + space_after_script;
    }

    MathLayoutResult {
        inline_size,
        ascent,
        descent,
        fragments,
    }
}

/// Lay out an `mtable`.
///
/// <https://w3c.github.io/mathml-core/#tabular-math-mtable>
///
/// This is the basic row/column alignment surface required by the MathML test
/// suite: rows stack, column widths are the maximum cell inline size, cells are
/// baseline-aligned inside their row and centered inside their column, and the
/// block center of the table is aligned with the math axis. `rowspan`,
/// `columnspan`, `rowalign`/`columnalign`/`columnwidth` and table frames are
/// not implemented (registered as follow-up work).
fn layout_table(
    container: &MathMlContainer,
    context: &MathLayoutContext,
    positioning_context: &mut PositioningContext,
    style: &ComputedValues,
) -> MathLayoutResult {
    let cell_font_size = font_size(style);
    let x_height = first_font_metrics(context.layout_context, style)
        .map(|metrics| metrics.x_height)
        .filter(|x_height| *x_height > Au::zero())
        .unwrap_or_else(|| em(cell_font_size, 0.5));
    // `mtd { padding: 0.5ex 0.4em }` from the MathML Core user agent sheet.
    let padding_inline = em(cell_font_size, 0.4);
    let padding_block = x_height.scale_by(0.5);

    struct Row<'a> {
        child: &'a MathChild,
        cells: Vec<ChildLayout>,
    }

    let mut rows = Vec::with_capacity(container.children.len());
    for child in &container.children {
        let mut cells = Vec::new();
        if child.is_table_row {
            child.with_context(|formatting_context| {
                if let Some(row_container) = formatting_context.math_container() {
                    for cell in &row_container.children {
                        cells.push(layout_child(cell, context, positioning_context));
                    }
                }
            });
        }
        if cells.is_empty() {
            // A row with no cells, or table content that is not wrapped in a
            // row, contributes a single cell with the child's own layout.
            cells.push(layout_child(child, context, positioning_context));
        }
        rows.push(Row { child, cells });
    }

    if rows.is_empty() {
        return MathLayoutResult {
            inline_size: Au::zero(),
            ascent: context.constants.axis_height,
            descent: context.constants.axis_height,
            fragments: Vec::new(),
        };
    }

    let column_count = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    let mut column_layout_sizes = vec![Au::zero(); column_count];
    for row in &rows {
        for (column, cell) in row.cells.iter().enumerate() {
            column_layout_sizes[column] =
                column_layout_sizes[column].max(cell.inline_size() + padding_inline.scale_by(2.0));
        }
    }

    let mut row_ascent = Vec::with_capacity(rows.len());
    let mut row_descent = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut ascent = Au::zero();
        let mut descent = Au::zero();
        for cell in &row.cells {
            ascent = ascent.max(cell.ascent + padding_block);
            descent = descent.max(cell.descent + padding_block);
        }
        row_ascent.push(ascent);
        row_descent.push(descent);
    }

    let inline_size = column_layout_sizes
        .iter()
        .fold(Au::zero(), |size, column| size + *column);
    let block_size = row_ascent
        .iter()
        .zip(row_descent.iter())
        .fold(Au::zero(), |size, (ascent, descent)| {
            size + *ascent + *descent
        });

    let mut fragments = Vec::with_capacity(rows.len());
    let mut block_position = Au::zero();
    for (row_index, row) in rows.iter_mut().enumerate() {
        let row_block_size = row_ascent[row_index] + row_descent[row_index];
        let mut row_fragments = Vec::with_capacity(row.cells.len());
        let mut inline_position = Au::zero();
        for (column, cell) in row.cells.iter_mut().enumerate() {
            let column_size = column_layout_sizes[column];
            let available = column_size - padding_inline.scale_by(2.0);
            let x =
                inline_position + padding_inline + centered_offset(available, cell.inline_size());
            row_fragments.push(cell.place(
                x,
                row_ascent[row_index],
                context.containing_block,
                positioning_context,
            ));
            inline_position += column_size;
        }

        // Wrap the row's cells in a fragment for the `mtr` element so that row
        // geometry is queryable from script, and place that fragment at the
        // row's block position inside the table.
        let row_fragment = row.child.with_context(|formatting_context| {
            let content_rect = LogicalRect {
                start_corner: LogicalVec2 {
                    inline: Au::zero(),
                    block: block_position,
                },
                size: LogicalVec2 {
                    inline: inline_size,
                    block: row_block_size,
                },
            }
            .as_physical(Some(context.containing_block));
            let fragment = BoxFragment::new(
                formatting_context.base_fragment_info(),
                formatting_context.style().clone(),
                row_fragments,
                content_rect,
                PhysicalSides::zero(),
                PhysicalSides::zero(),
                PhysicalSides::zero(),
                None,
            );
            let fragment = Arc::new(fragment);
            formatting_context
                .base
                .set_fragment(Fragment::Box(fragment.clone()));
            Fragment::Box(fragment)
        });
        fragments.push(row_fragment);
        block_position += row_block_size;
    }

    // The table's center is aligned with the math axis.
    let axis = context.constants.axis_height;
    let half_block_size = block_size.scale_by(0.5);
    MathLayoutResult {
        inline_size,
        ascent: half_block_size + axis,
        descent: (half_block_size - axis).max(Au::zero()),
        fragments,
    }
}
