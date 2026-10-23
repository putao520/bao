/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! MathML layout.
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//! MathML layout does not exist anywhere in this Servo snapshot (the only
//! MathML-aware layout code is the `math-auto` italics mapping in
//! `flow/inline/mathml_italics.rs`), so this module implements the MathML Core
//! layout model for the element families the MathML test suite exercises:
//!
//! * `mfrac` (§3.3.2 "Fractions"),
//! * `msub`/`msup`/`msubsup` (§3.4 "Scripts and limits"),
//! * `munder`/`mover`/`munderover` (§3.4, limits and stacks),
//! * `mmultiscripts` (§3.4, prescripts and postscripts),
//! * `mtable`/`mtr`/`mtd` (§3.5 "Tabular math", basic row/column alignment),
//! * `mspace` (§3.2.5),
//! * the grouping elements (`math`, `mrow`, `mstyle`, …) and the token
//!   elements (`mi`, `mn`, `mo`, `mtext`, `ms`), which reuse the regular flow
//!   layout engine for their text.
//!
//! # Architecture
//!
//! A MathML element that reaches box tree construction becomes an independent
//! formatting context whose contents are
//! [`crate::formatting_contexts::IndependentFormattingContextContents::MathMl`]
//! — a [`MathMlContainer`]. The container stores one [`MathChild`] per in-flow
//! child, and every child owns its own [`IndependentFormattingContext`]:
//!
//! * math children (`mfrac`, `mrow`, `mo`, …) get another `MathMl` context, so
//!   the whole math subtree is laid out recursively by this module,
//! * token and foreign children (`mi`, `mn`, `mtext`, `ms` and any non-MathML
//!   element) get a regular flow context, so their text is shaped, painted and
//!   selected by the existing inline layout machinery,
//! * a direct text child of a MathML element is wrapped in an anonymous flow
//!   context, which is the layout equivalent of the `mtext` MathML Core
//!   wraps character data in.
//!
//! Layout is a "measure children, then position them" pass: each child is laid
//! out through
//! [`IndependentFormattingContext::layout_float_or_atomic_inline`], which
//! produces its fragments and baselines at fit-content size, and the math
//! algorithms in [`layout`] compute the offsets of those fragments. Fragment
//! coordinates are relative to the parent content box, as everywhere else in
//! layout.
//!
//! # Known gaps (registered as MathML follow-up work)
//!
//! * The OpenType `MATH` table is not exposed by the layout text pipeline, so
//!   glyph variants/assemblies are unavailable and the exact per-font script
//!   shifts cannot be read. Stretching therefore uses the specification's own
//!   fallback (when a stretchy glyph cannot be shaped the layout of `mtext` is
//!   used, §3.2.4.3); `AxisHeight` and the rule thickness come from the real
//!   font metrics (`OS/2.sxHeight` / `post.underlineThickness`).
//! * Only horizontal writing modes are laid out.
//! * `msqrt`/`mroot` lay out their base as a row without a radical sign.
//! * Embellished-operator detection only looks at a direct `<mo>` child.
//! * Presentation attributes are read when the box tree is constructed; a
//!   change to them rebuilds the enclosing block's box tree instead of an
//!   incremental math subtree.

pub(crate) mod constants;
pub(crate) mod dictionary;
mod layout;

#[cfg(test)]
mod tests;

use layout_api::{LayoutElement, LayoutNode};
use malloc_size_of_derive::MallocSizeOf;
use script::layout_dom::ServoLayoutNode;
use style::dom::NodeInfo;
use style::properties::ComputedValues;
use style::selector_parser::PseudoElement;
use web_atoms::{local_name, ns};

use self::dictionary::{OperatorAttributes, OperatorForm, OperatorProperties};
use crate::cell::ArcRefCell;
use crate::context::LayoutContext;
use crate::dom::{LayoutBox, NodeExt, WeakLayoutBox};
use crate::dom_traversal::{Contents, NodeAndStyleInfo};
use crate::flow::inline::construct::InlineFormattingContextBuilder;
use crate::flow::{BlockContainer, BlockFormattingContext, BlockLevelBox};
use crate::formatting_contexts::{
    IndependentFormattingContext, IndependentFormattingContextContents,
};
use crate::layout_box_base::LayoutBoxBase;
use crate::positioned::PositioningContext;
use crate::sizing::{ComputeInlineContentSizes, ContentSizes, InlineContentSizesResult};
use crate::style_ext::{ComputedValuesExt, Display, DisplayInside};
use crate::{ConstraintSpace, ContainingBlock, ContainingBlockSize};

pub(crate) use self::layout::MathLayoutResult;

/// The MathML element families with dedicated layout.
///
/// <https://w3c.github.io/mathml-core/#element-syntax>
#[derive(Clone, Copy, Debug, Eq, MallocSizeOf, PartialEq)]
pub(crate) enum MathElementKind {
    /// A grouping element (`math`, `mrow`, `mstyle`, `mphantom`, `mpadded`,
    /// `merror`, `menclose`, `maction`, `semantics`): children are laid out
    /// side by side on a common baseline. `msqrt`/`mroot` currently use this
    /// layout as well (see the module docs).
    Row,
    /// `mfrac` — numerator over denominator.
    Fraction,
    /// `msub`.
    Sub,
    /// `msup`.
    Sup,
    /// `msubsup`.
    SubSup,
    /// `munder`.
    Under,
    /// `mover`.
    Over,
    /// `munderover`.
    UnderOver,
    /// `mmultiscripts`.
    MultiScripts,
    /// `mtable`.
    Table,
    /// A token element (`mi`, `mn`, `mo`, `mtext`, `ms`): its contents are laid
    /// out by the regular flow layout engine.
    Token,
    /// `mspace`.
    Space,
    /// `mprescripts` — a marker inside `mmultiscripts`, generates no box.
    Prescripts,
}

impl MathElementKind {
    /// Classify a MathML element by its local name.
    fn from_element(node: ServoLayoutNode<'_>) -> Option<Self> {
        let element = node.as_element()?;
        let kind = match &**element.local_name() {
            "mfrac" => Self::Fraction,
            "msub" => Self::Sub,
            "msup" => Self::Sup,
            "msubsup" => Self::SubSup,
            "munder" => Self::Under,
            "mover" => Self::Over,
            "munderover" => Self::UnderOver,
            "mmultiscripts" => Self::MultiScripts,
            "mtable" => Self::Table,
            "mspace" => Self::Space,
            "mprescripts" => Self::Prescripts,
            "mi" | "mn" | "mo" | "mtext" | "ms" | "mglyph" => Self::Token,
            // Grouping elements, table parts reached outside a table, and any
            // element this engine does not know are laid out as a row of
            // children.
            _ => Self::Row,
        };
        Some(kind)
    }

    /// Whether this element is a MathML grouping element, which controls
    /// operator form inference (`prefix`/`postfix` for the first/last child)
    /// and whether operator spacing participates.
    ///
    /// <https://w3c.github.io/mathml-core/#dfn-grouping-element>
    pub(crate) fn is_grouping_element(self) -> bool {
        matches!(self, Self::Row)
    }

    /// Whether this element is a token element whose contents are laid out by
    /// the flow layout engine.
    pub(crate) fn is_token(self) -> bool {
        matches!(self, Self::Token)
    }

    /// Whether this element is one of the script/limit schemata.
    pub(crate) fn is_scripted(self) -> bool {
        matches!(
            self,
            Self::Sub
                | Self::Sup
                | Self::SubSup
                | Self::Under
                | Self::Over
                | Self::UnderOver
                | Self::MultiScripts
        )
    }
}

/// A resolved MathML length used by `mspace`.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub(crate) enum MathLength {
    Em(f32),
    Px(app_units::Au),
}

impl MathLength {
    #[inline]
    fn resolve(self, font_size: app_units::Au) -> app_units::Au {
        match self {
            MathLength::Em(factor) => font_size.scale_by(factor),
            MathLength::Px(length) => length,
        }
    }
}

/// Parse a MathML `<length-percentage>` presentation attribute.
///
/// Percentages are not resolved here (they need a containing block), so a
/// percentage value yields `None` and the attribute is treated as absent, as
/// MathML Core does for invalid values.
fn parse_length(value: &str) -> Option<MathLength> {
    let value = value.trim();
    if value.ends_with('%') {
        return None;
    }
    if let Some(em) = value.strip_suffix("em") {
        return Some(MathLength::Em(em.trim().parse::<f32>().ok()?));
    }
    if let Some(px) = value.strip_suffix("px") {
        return Some(MathLength::Px(app_units::Au::from_f32_px(
            px.trim().parse::<f32>().ok()?,
        )));
    }
    let unitless = value.parse::<f32>().ok()?;
    (unitless == 0.0).then_some(MathLength::Em(0.0))
}

/// The `width`/`height`/`depth` attributes of an `mspace`.
///
/// <https://w3c.github.io/mathml-core/#space-mspace>
#[derive(Clone, Copy, Debug, Default, MallocSizeOf)]
pub(crate) struct MathSpaceDimensions {
    pub width: Option<MathLength>,
    pub height: Option<MathLength>,
    pub depth: Option<MathLength>,
}

/// The operator data of an `<mo>` element.
///
/// The dictionary properties differ per form, and the form can only be
/// determined once the operator's position among its siblings is known, so all
/// three forms are resolved when the box tree is constructed.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub(crate) struct MathOperator {
    prefix: OperatorProperties,
    infix: OperatorProperties,
    postfix: OperatorProperties,
    /// The `form` attribute, which overrides the inferred form.
    pub(crate) explicit_form: Option<OperatorForm>,
}

impl MathOperator {
    /// The properties for `form`.
    pub(crate) fn properties(self, form: OperatorForm) -> OperatorProperties {
        match form {
            OperatorForm::Prefix => self.prefix,
            OperatorForm::Infix => self.infix,
            OperatorForm::Postfix => self.postfix,
        }
    }
}

/// One in-flow child of a [`MathMlContainer`].
#[derive(Debug, MallocSizeOf)]
pub(crate) struct MathChild {
    /// The child's box.
    ///
    /// The box is stored as a [`BlockLevelBox`] even for inline-level children:
    /// it is the storage form that lets the DOM node keep a handle to its
    /// fragments (box queries, restyle) while the math layout owns the
    /// geometry. Math children contain a `MathMl` formatting context, token
    /// children a regular flow context.
    pub(crate) block_level_box: ArcRefCell<BlockLevelBox>,
    /// The `<mo>` properties when the child is an operator.
    pub(crate) operator: Option<MathOperator>,
    /// Whether this child is an `mtr`, i.e. a row of an `mtable`.
    pub(crate) is_table_row: bool,
}

impl MathChild {
    /// Run `callback` with the child's [`IndependentFormattingContext`].
    pub(crate) fn with_context<T>(
        &self,
        callback: impl FnOnce(&IndependentFormattingContext) -> T,
    ) -> T {
        let block_level_box = self.block_level_box.borrow();
        let BlockLevelBox::Independent(context) = &*block_level_box else {
            unreachable!("A math child always stores an independent formatting context");
        };
        callback(context)
    }
}

/// The MathML formatting context of one MathML element.
#[derive(Debug, MallocSizeOf)]
pub(crate) struct MathMlContainer {
    /// What kind of MathML layout this element requires.
    pub(crate) kind: MathElementKind,
    /// The element's in-flow children, in tree order.
    pub(crate) children: Vec<MathChild>,
    /// For `mmultiscripts`: the index in [`Self::children`] at which the
    /// prescripts start, i.e. the number of children before `<mprescripts>`.
    pub(crate) prescripts_start: Option<usize>,
    /// For `mspace`: the resolved `width`/`height`/`depth` attributes.
    pub(crate) space: MathSpaceDimensions,
}

impl MathMlContainer {
    /// Build the container for a MathML element, recursively building the
    /// formatting context of every in-flow child.
    pub(crate) fn construct(context: &LayoutContext, info: &NodeAndStyleInfo<'_>) -> Self {
        let kind = MathElementKind::from_element(info.node).unwrap_or(MathElementKind::Row);
        let mut container = Self {
            kind,
            children: Vec::new(),
            prescripts_start: None,
            space: MathSpaceDimensions::default(),
        };

        if kind == MathElementKind::Space {
            container.space = read_space_dimensions(info.node);
            return container;
        }

        for child in info.node.flat_tree_children() {
            if child.is_text_node() {
                if let Some(math_child) = Self::construct_text_child(context, info, &child) {
                    container.children.push(math_child);
                }
            } else if child.as_element().is_some() {
                let Some(child_kind) = MathElementKind::from_element(child) else {
                    continue;
                };
                if child_kind == MathElementKind::Prescripts {
                    // `<mprescripts>` is a marker, not a box.
                    let index = container.children.len();
                    container.prescripts_start.get_or_insert(index);
                    continue;
                }
                if let Some(math_child) = Self::construct_element_child(context, &child, child_kind)
                {
                    container.children.push(math_child);
                }
            }
        }

        container
    }

    /// Build an anonymous flow formatting context for a text node that is a
    /// direct child of a MathML element.
    ///
    /// MathML Core treats character data inside MathML elements as if it was
    /// wrapped in an `mtext`, which is what an anonymous inline formatting
    /// context containing the text run does here.
    fn construct_text_child<'dom>(
        context: &LayoutContext,
        info: &NodeAndStyleInfo<'dom>,
        node: &ServoLayoutNode<'dom>,
    ) -> Option<MathChild> {
        let anonymous_info = info
            .with_pseudo_element(context, PseudoElement::ServoAnonymousBox)
            .unwrap_or_else(|| info.clone());
        let text_info = NodeAndStyleInfo::new(*node, node.style(&context.style_context));
        let mut builder = InlineFormattingContextBuilder::new(
            &anonymous_info,
            context,
            false, /* should_process_first_letter */
        );
        builder.push_text(
            node.text_content().into(),
            &text_info,
            node.text_node_selection(),
        );
        let inline_formatting_context = builder.finish(
            context,
            true,  /* has_first_formatted_line */
            false, /* is_single_line_text_box */
            anonymous_info.style.to_bidi_level(),
        )?;

        let block_formatting_context = BlockFormattingContext::from_block_container(
            BlockContainer::InlineFormattingContext(inline_formatting_context),
        );
        let formatting_context = IndependentFormattingContext::new(
            LayoutBoxBase::new((&anonymous_info).into(), anonymous_info.style.clone()),
            IndependentFormattingContextContents::Flow(block_formatting_context),
            Default::default(),
        );
        // Text nodes do not own a box slot: their only box is the anonymous
        // formatting context built above, whose fragments belong to the
        // anonymous box of the element.
        Some(MathChild {
            block_level_box: ArcRefCell::new(BlockLevelBox::Independent(formatting_context)),
            operator: None,
            is_table_row: false,
        })
    }

    /// Build the formatting context of an element child.
    fn construct_element_child(
        context: &LayoutContext,
        node: &ServoLayoutNode<'_>,
        kind: MathElementKind,
    ) -> Option<MathChild> {
        let style = node.style(&context.style_context);
        if let Display::None = Display::from(style.get_box().display) {
            return None;
        }

        let info = NodeAndStyleInfo::new(*node, style);
        let contents = Contents::for_element(*node, context);

        // Token elements and foreign elements are laid out by the regular flow
        // layout engine; every other MathML element gets another math
        // formatting context.
        let display_inside = if kind.is_token() {
            DisplayInside::Flow {
                is_list_item: false,
            }
        } else {
            DisplayInside::Math
        };
        let formatting_context = IndependentFormattingContext::construct(
            context,
            &info,
            display_inside,
            contents,
            Default::default(),
        );

        let block_level_box = ArcRefCell::new(BlockLevelBox::Independent(formatting_context));
        node.box_slot()
            .set(LayoutBox::BlockLevel(block_level_box.clone()));

        let element = node.as_element();
        let operator = element
            .filter(|element| &**element.local_name() == "mo")
            .map(|element| read_operator(element, *node));
        let is_table_row = element
            .map(|element| &**element.local_name() == "mtr")
            .unwrap_or(false);

        Some(MathChild {
            block_level_box,
            operator,
            is_table_row,
        })
    }

    /// Lay out this container's math content.
    pub(crate) fn layout(
        &self,
        layout_context: &LayoutContext,
        positioning_context: &mut PositioningContext,
        containing_block: &ContainingBlock,
        style: &ComputedValues,
    ) -> MathLayoutResult {
        layout::layout_container(
            self,
            layout_context,
            positioning_context,
            containing_block,
            style,
        )
    }

    /// The number of boxes in this container's subtree.
    pub(crate) fn subtree_size(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|child| child.with_context(|context| context.subtree_size()))
            .sum::<usize>()
    }

    /// Mark every child box as attached to its parent box.
    pub(crate) fn attached_to_tree(&self, layout_box: WeakLayoutBox) {
        for child in &self.children {
            child.with_context(|context| context.attached_to_tree(layout_box.clone()));
        }
    }
}

/// Read the `width`/`height`/`depth` attributes of an `mspace`.
fn read_space_dimensions(node: ServoLayoutNode<'_>) -> MathSpaceDimensions {
    let Some(element) = node.as_element() else {
        return MathSpaceDimensions::default();
    };
    MathSpaceDimensions {
        width: element
            .attribute_as_str(&ns!(), &local_name!("width"))
            .and_then(parse_length),
        height: element
            .attribute_as_str(&ns!(), &local_name!("height"))
            .and_then(parse_length),
        depth: element
            .attribute_as_str(&ns!(), &local_name!("depth"))
            .and_then(parse_length),
    }
}

/// Read and resolve the dictionary-based properties of an `<mo>`.
fn read_operator<'a>(element: impl LayoutElement<'a>, node: ServoLayoutNode<'_>) -> MathOperator {
    let attributes = OperatorAttributes {
        form: element.attribute_as_str(&ns!(), &local_name!("form")),
        lspace: element.attribute_as_str(&ns!(), &local_name!("lspace")),
        rspace: element.attribute_as_str(&ns!(), &local_name!("rspace")),
        stretchy: element.attribute_as_str(&ns!(), &local_name!("stretchy")),
        symmetric: element.attribute_as_str(&ns!(), &local_name!("symmetric")),
        largeop: element.attribute_as_str(&ns!(), &local_name!("largeop")),
        movablelimits: element.attribute_as_str(&ns!(), &local_name!("movablelimits")),
        fence: element.attribute_as_str(&ns!(), &local_name!("fence")),
        separator: element.attribute_as_str(&ns!(), &local_name!("separator")),
    };

    // The inferred form is not known yet, so resolve every form and let the
    // layout pick the one implied by the operator's position.
    let content = single_character_content(node);
    let explicit_form = attributes.form.and_then(dictionary::parse_form);
    let formless = OperatorAttributes {
        form: None,
        ..attributes
    };
    MathOperator {
        prefix: dictionary::resolve(content, OperatorForm::Prefix, &formless).1,
        infix: dictionary::resolve(content, OperatorForm::Infix, &formless).1,
        postfix: dictionary::resolve(content, OperatorForm::Postfix, &formless).1,
        explicit_form,
    }
}

/// The single character an `<mo>` contains, when its text content is exactly
/// one character after whitespace trimming.
///
/// <https://w3c.github.io/mathml-core/#operator-fence-separator-or-accent-mo>
fn single_character_content(node: ServoLayoutNode<'_>) -> Option<char> {
    let mut content = String::new();
    for child in node.flat_tree_children() {
        if !child.is_text_node() {
            return None;
        }
        content.push_str(&child.text_content());
    }
    let mut characters = content.trim().chars();
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}

impl ComputeInlineContentSizes for MathMlContainer {
    fn compute_inline_content_sizes(
        &self,
        layout_context: &LayoutContext,
        constraint_space: &ConstraintSpace,
    ) -> InlineContentSizesResult {
        // MathML has no soft wrap opportunities: an expression is laid out at
        // its natural size, so the min-content and max-content inline sizes are
        // both the natural inline size. That size is obtained by running the
        // real layout with an effectively unbounded inline size (and is cached
        // by `LayoutBoxBase::inline_content_sizes`).
        let containing_block = ContainingBlock {
            size: ContainingBlockSize {
                inline: unbounded_inline_size(),
                block: constraint_space.block_size,
            },
            style: constraint_space.style,
        };
        let mut positioning_context = PositioningContext::default();
        let result = layout::layout_container(
            self,
            layout_context,
            &mut positioning_context,
            &containing_block,
            constraint_space.style,
        );
        InlineContentSizesResult {
            sizes: ContentSizes {
                min_content: result.inline_size,
                max_content: result.inline_size,
            },
            depends_on_block_constraints: false,
        }
    }
}

/// An inline size large enough that no math expression wraps inside it. MathML
/// never wraps, so this is only used while measuring intrinsic sizes.
fn unbounded_inline_size() -> app_units::Au {
    app_units::Au::from_px(1 << 24)
}
