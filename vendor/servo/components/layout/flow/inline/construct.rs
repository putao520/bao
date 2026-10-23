/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::borrow::Cow;
use std::cell::LazyCell;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use app_units::Au;
use atomic_refcell::AtomicRefCell;
use icu_properties::CodePointMapData;
use icu_properties::props::BidiClass;
use layout_api::{LayoutElement, LayoutNode};
use servo_base::text::{AssumeUnder4GB, RangeAny, Utf8CodeUnits, Utf32CodeUnits};
use style::Zero;
use style::computed_values::direction::T as Direction;
use style::computed_values::visibility::T as Visibility;
use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
use style::dom::NodeInfo;
use style::properties::ComputedValues;
use style::selector_parser::PseudoElement;
use unicode_bidi::Level;
use unicode_categories::UnicodeCategories;

use super::text_run::{TextRun, get_font_for_first_font_for_style};
use super::{
    InlineBox, InlineBoxIdentifier, InlineBoxes, InlineFormattingContext, InlineItem,
    RubyAnnotationLevel, RubyCellItem, RubySegmentItem, SharedInlineStyles,
};
use crate::PropagatedBoxTreeData;
use crate::cell::ArcRefCell;
use crate::context::LayoutContext;
use crate::dom::{BoxSlot, LayoutBox, NodeExt};
use crate::dom_traversal::{
    BoxTreeString, Contents, NodeAndStyleInfo, NonReplacedContents, TraversalHandler,
};
use crate::flow::BlockLevelBox;
use crate::flow::float::FloatBox;
use crate::flow::inline::text_run::SharedTextRunData;
use crate::flow::inline::text_transform::{OffsetMap, TextTransformationIterator};
use crate::flow::ruby::builder::{ruby_child_kind_for_element, ruby_child_kind_for_text};
use crate::flow::ruby::plan::{
    RubyChildKind, RubyPlanChild, RubyPlanLevel, RubyPlanSegment, RubyPlanUnit, RubyPlanUnitSource,
    build_plan,
};
use crate::flow::{BlockContainer, BlockFormattingContext};
use crate::formatting_contexts::{
    IndependentFormattingContext, IndependentFormattingContextContents,
};
use crate::layout_box_base::LayoutBoxBase;
use crate::positioned::AbsolutelyPositionedBox;
use crate::style_ext::{ComputedValuesExt, Display, DisplayGeneratingBox, DisplayInside};

#[derive(Default)]
pub(crate) struct InlineFormattingContextBuilder {
    /// A stack of [`SharedInlineStyles`] including one for the root, one for each inline box on the
    /// inline box stack, and importantly, one for every `display: contents` element that we are
    /// currently processing. Normally `display: contents` elements don't affect the structure of
    /// the [`InlineFormattingContext`], but the styles they provide do style their children.
    pub shared_inline_styles_stack: Vec<SharedInlineStyles>,

    /// The collection of text strings that make up this [`InlineFormattingContext`] under
    /// construction.
    pub text_segments: Vec<String>,

    /// A slot used to share the full text string of the [`InlineFormattingContext`] this
    /// builder will ultimately build.
    pub text_content_slot: Arc<OnceLock<String>>,

    /// The current offset in the final text string of this [`InlineFormattingContext`],
    /// used to properly set the text range of new [`InlineItem::TextRun`]s.
    current_text_offset: Utf8CodeUnits,

    /// The current character offset in the final text string of this [`InlineFormattingContext`],
    /// used to properly set the text range of new [`InlineItem::TextRun`]s. Note that this is
    /// different from the UTF-8 code point offset.
    current_character_offset: Utf32CodeUnits,

    /// Whether the last processed node ended with whitespace. This is used to
    /// implement rule 4 of <https://www.w3.org/TR/css-text-3/#collapse>:
    ///
    /// > Any collapsible space immediately following another collapsible space—even one
    /// > outside the boundary of the inline containing that space, provided both spaces are
    /// > within the same inline formatting context—is collapsed to have zero advance width.
    /// > (It is invisible, but retains its soft wrap opportunity, if any.)
    last_inline_box_ended_with_collapsible_white_space: bool,

    /// Whether or not the current state of the inline formatting context is on a word boundary
    /// for the purposes of `text-transform: capitalize`.
    on_word_boundary: bool,

    /// Whether or not this inline formatting context will contain floats.
    pub contains_floats: bool,

    /// The current list of [`InlineItem`]s in this [`InlineFormattingContext`] under
    /// construction. This is stored in a flat list to make it easy to access the last
    /// item.
    pub inline_items: Vec<InlineItem>,

    /// The current [`InlineBox`] tree of this [`InlineFormattingContext`] under construction.
    pub inline_boxes: InlineBoxes,

    /// The ongoing stack of inline boxes stack of the builder.
    ///
    /// Contains all the currently ongoing inline boxes we entered so far.
    /// The traversal is at all times as deep in the tree as this stack is,
    /// which is why the code doesn't need to keep track of the actual
    /// container root (see `handle_inline_level_element`).
    ///
    /// When an inline box ends, it's removed from this stack.
    inline_box_stack: Vec<InlineBoxIdentifier>,

    /// Whether this [`InlineFormattingContextBuilder`] is empty for the purposes of ignoring
    /// during box tree construction. An IFC is empty if it only contains TextRuns with
    /// completely collapsible whitespace. When that happens it can be ignored completely.
    pub is_empty: bool,

    /// Whether this IFC belongs to an editing host. An empty editing host keeps a
    /// caret line box so that it can be hit, focused, and typed into
    /// (Chromium-compat).
    pub is_editing_host: bool,

    /// Whether this inline formatting context should process the `::first-letter` pseudo-element.
    /// This typically starts as true, and then is set to false after processing `::first-letter`
    /// or encountering an atomic. But it can also be false from the very beginning if this inline
    /// formatting context doesn't contain the first formatted line.
    should_process_first_letter: bool,

    /// Whether or not the inline formatting context under construction has any kind of
    /// right-to-left content such as a character with an RTL character class or a `dir`
    /// attribute specifying right-to-left content.
    pub has_right_to_left_content: bool,

    /// An [`OffsetMap`] used to map selections from their offset before inline formatting
    /// context text transformation to their offsets after transformation.
    pub offset_map: ArcRefCell<OffsetMap>,
}

impl InlineFormattingContextBuilder {
    /// <https://drafts.csswg.org/css-text/#white-space>:
    /// > Except where specified otherwise, white space processing in CSS affects only the document
    /// > white space characters: spaces (U+0020), tabs (U+0009), and segment breaks.
    ///
    /// From <https://github.com/w3c/csswg-drafts/issues/5147#issuecomment-637816669>:
    /// > HTML clearly treats CR, LF, and CRLF as segment breaks.
    ///
    /// Other browsers also consider the form feed character (0x0c) to be document white space, it
    /// seems.
    ///
    /// Taken all together, this is equivalent to the WhatWG Infra Standard's definition of ASCII
    /// white space.
    pub(crate) fn is_document_white_space(character: char) -> bool {
        character.is_ascii_whitespace()
    }

    pub(crate) fn new(
        info: &NodeAndStyleInfo,
        context: &LayoutContext,
        should_process_first_letter: bool,
    ) -> Self {
        let has_right_to_left_content = info.style.get_inherited_box().direction == Direction::Rtl;
        Self {
            // For the purposes of `text-transform: capitalize` the start of the IFC is a word boundary.
            on_word_boundary: true,
            is_empty: true,
            shared_inline_styles_stack: vec![SharedInlineStyles::from_info_and_context(
                info, context,
            )],
            has_right_to_left_content,
            should_process_first_letter,
            ..Default::default()
        }
    }

    pub(crate) fn currently_processing_inline_box(&self) -> bool {
        !self.inline_box_stack.is_empty()
    }

    fn push_control_character_string(&mut self, string_to_push: &str) {
        self.text_segments.push(string_to_push.to_owned());
        // string_to_push is always small
        self.current_text_offset += Utf8CodeUnits::length_of(AssumeUnder4GB, string_to_push);

        let new_characters = Utf32CodeUnits::length_of(AssumeUnder4GB, string_to_push);
        self.current_character_offset += new_characters;
        self.offset_map
            .borrow_mut()
            .push_range(new_characters, new_characters);
    }

    fn shared_inline_styles(&self) -> SharedInlineStyles {
        self.shared_inline_styles_stack
            .last()
            .expect("Should always have at least one SharedInlineStyles")
            .clone()
    }

    pub(crate) fn push_atomic(
        &mut self,
        independent_formatting_context_creator: impl FnOnce()
            -> ArcRefCell<IndependentFormattingContext>,
        old_layout_box: Option<LayoutBox>,
    ) -> InlineItem {
        // If there is an existing undamaged layout box that's compatible, use that.
        let independent_formatting_context = old_layout_box
            .and_then(|layout_box| match layout_box {
                LayoutBox::InlineLevel(InlineItem::Atomic(atomic, ..)) => Some(atomic),
                _ => None,
            })
            .unwrap_or_else(independent_formatting_context_creator);

        let inline_level_box = InlineItem::Atomic(
            independent_formatting_context,
            self.current_text_offset,
            Level::ltr(), /* This will be assigned later if necessary. */
        );
        self.inline_items.push(inline_level_box.clone());
        self.is_empty = false;

        // Push an object replacement character for this atomic, which will ensure that the line breaker
        // inserts a line breaking opportunity here.
        self.push_control_character_string("\u{fffc}");

        self.last_inline_box_ended_with_collapsible_white_space = false;
        self.on_word_boundary = true;

        // Atomics such as images should prevent any following text as being interpreted as the first letter.
        self.should_process_first_letter = false;

        inline_level_box
    }

    pub(crate) fn push_absolutely_positioned_box(
        &mut self,
        absolutely_positioned_box_creator: impl FnOnce() -> ArcRefCell<AbsolutelyPositionedBox>,
        old_layout_box: Option<LayoutBox>,
    ) -> InlineItem {
        let absolutely_positioned_box = old_layout_box
            .and_then(|layout_box| match layout_box {
                LayoutBox::InlineLevel(InlineItem::OutOfFlowAbsolutelyPositionedBox(
                    positioned_box,
                    ..,
                )) => Some(positioned_box),
                _ => None,
            })
            .unwrap_or_else(absolutely_positioned_box_creator);

        // We cannot just reuse the old inline item, because the `current_text_offset` may have changed.
        let inline_level_box = InlineItem::OutOfFlowAbsolutelyPositionedBox(
            absolutely_positioned_box,
            self.current_text_offset,
        );

        self.inline_items.push(inline_level_box.clone());
        self.is_empty = false;
        inline_level_box
    }

    pub(crate) fn push_float_box(
        &mut self,
        float_box_creator: impl FnOnce() -> ArcRefCell<FloatBox>,
        old_layout_box: Option<LayoutBox>,
    ) -> InlineItem {
        let inline_level_box = old_layout_box
            .and_then(|layout_box| match layout_box {
                LayoutBox::InlineLevel(inline_item) => Some(inline_item),
                _ => None,
            })
            .unwrap_or_else(|| InlineItem::OutOfFlowFloatBox(float_box_creator()));

        debug_assert!(
            matches!(inline_level_box, InlineItem::OutOfFlowFloatBox(..),),
            "Created float box with incompatible `old_layout_box`"
        );

        self.inline_items.push(inline_level_box.clone());
        self.is_empty = false;
        self.contains_floats = true;
        inline_level_box
    }

    pub(crate) fn push_block_level_box(&mut self, block_level: ArcRefCell<BlockLevelBox>) {
        assert!(self.currently_processing_inline_box());
        self.contains_floats = self.contains_floats || block_level.borrow().contains_floats();
        self.inline_items.push(InlineItem::BlockLevel(block_level));
    }

    pub(crate) fn start_inline_box(
        &mut self,
        inline_box_creator: impl FnOnce() -> ArcRefCell<InlineBox>,
        old_layout_box: Option<LayoutBox>,
    ) -> InlineItem {
        // If there is an existing undamaged layout box that's compatible, use the `InlineBox` within it.
        let inline_box = old_layout_box
            .and_then(|layout_box| match layout_box {
                LayoutBox::InlineLevel(InlineItem::StartInlineBox(inline_box)) => Some(inline_box),
                _ => None,
            })
            .unwrap_or_else(inline_box_creator);

        let borrowed_inline_box = inline_box.borrow();

        let style = &borrowed_inline_box.base.style;
        self.push_control_character_string(style.bidi_control_chars().0);
        self.has_right_to_left_content =
            self.has_right_to_left_content || style.get_inherited_box().direction == Direction::Rtl;

        self.shared_inline_styles_stack
            .push(borrowed_inline_box.shared_inline_styles.clone());
        std::mem::drop(borrowed_inline_box);

        let identifier = self.inline_boxes.start_inline_box(inline_box.clone());
        let inline_item = InlineItem::StartInlineBox(inline_box);
        self.inline_items.push(inline_item.clone());
        self.inline_box_stack.push(identifier);
        self.is_empty = false;
        inline_item
    }

    /// End the ongoing inline box in this [`InlineFormattingContextBuilder`], returning
    /// shared references to all of the box tree items that were created for it. More than
    /// a single box tree items may be produced for a single inline box when that inline
    /// box is split around a block-level element.
    pub(crate) fn end_inline_box(&mut self) {
        let identifier = self
            .inline_box_stack
            .pop()
            .expect("Ended non-existent inline box");
        let inline_level_box = self.inline_boxes.get(&identifier);

        self.shared_inline_styles_stack.pop();
        self.inline_items
            .push(InlineItem::EndInlineBox(inline_level_box.clone()));
        self.inline_boxes.end_inline_box(identifier);
        let bidi_control_chars = inline_level_box.borrow().base.style.bidi_control_chars();
        self.push_control_character_string(bidi_control_chars.1);
    }

    /// This is like [`Self::push_text`], except that it might possibly add an anonymous box if
    ///
    ///  - This inline formatting context has a `::first-letter` style.
    ///  - No anonymous box for `::first-letter` has been added yet.
    ///  - First letter content is detected in this text.
    ///
    /// Note that this should only be used when processing text in block containers.
    pub(crate) fn push_text_with_possible_first_letter<'dom>(
        &mut self,
        text: BoxTreeString<'dom>,
        info: &NodeAndStyleInfo<'dom>,
        container_info: &NodeAndStyleInfo<'dom>,
        layout_context: &LayoutContext,
    ) -> bool {
        let selection = info.node.text_node_selection();
        if !self.should_process_first_letter || !container_info.pseudo_element_chain().is_empty() {
            self.push_text(text, info, selection);
            return false;
        }

        let Some(first_letter_info) =
            container_info.with_pseudo_element(layout_context, PseudoElement::FirstLetter)
        else {
            self.push_text(text, info, selection);
            return false;
        };

        let first_letter_range = first_letter_range(&text[..]);
        if first_letter_range.is_empty() {
            return false;
        }

        // Push any leading white space first.
        let first_letter_range_u32 = LazyCell::new(|| {
            // TODO: ensure layout doesn’t handle more than 4 GiB at a time?
            Utf32CodeUnits::length_of(AssumeUnder4GB, &text[..first_letter_range.start])
                ..Utf32CodeUnits::length_of(AssumeUnder4GB, &text[..first_letter_range.end])
        });
        if first_letter_range.start != 0 {
            let leading_whitespace_range = 0..first_letter_range.start;
            let leading_whitespace_selection_range = selection.and_then(|range| {
                let leading_whitespace_range_u32 =
                    RangeAny::from_start_to(first_letter_range_u32.start);
                range.intersect(leading_whitespace_range_u32)
            });

            self.push_text(
                Cow::Borrowed(&text[leading_whitespace_range]).into(),
                info,
                leading_whitespace_selection_range,
            );
        }

        // Push the first-letter text into an anonymous box with the `::first-letter` style.
        let box_slot = first_letter_info.node.box_slot();
        let inline_item = self.start_inline_box(
            || ArcRefCell::new(InlineBox::new(&first_letter_info, layout_context)),
            None,
        );
        box_slot.set(LayoutBox::InlineLevel(inline_item));

        let first_letter_text = Cow::Borrowed(&text[first_letter_range.clone()]);
        let first_letter_selection_range = selection.and_then(|range| {
            range
                .intersect((*first_letter_range_u32).clone().into())
                .map(|range| range.map(|offset| offset - first_letter_range_u32.start))
        });
        self.push_text(
            first_letter_text.into(),
            &first_letter_info,
            first_letter_selection_range,
        );
        self.end_inline_box();
        self.should_process_first_letter = false;

        // Now push the non-first-letter text.
        let remaining_selection_range = selection.and_then(|range| {
            let remaining_text_range_u32 =
                RangeAny::new(Some(first_letter_range_u32.end), range.end());
            range
                .intersect(remaining_text_range_u32)
                .map(|range| range.map(|offset| offset - first_letter_range_u32.end))
        });
        self.push_text(
            Cow::Borrowed(&text[first_letter_range.end..]).into(),
            info,
            remaining_selection_range,
        );

        true
    }

    pub(crate) fn push_text<'dom>(
        &mut self,
        text: BoxTreeString<'dom>,
        info: &NodeAndStyleInfo<'dom>,
        selection: Option<RangeAny<Utf32CodeUnits>>,
    ) {
        let mut offset_map = self.offset_map.borrow_mut();
        let original_size_before = offset_map.total_original_size();

        let bidi_class_map = CodePointMapData::<BidiClass>::new();
        let white_space_collapse = info.style.get_white_space_collapse();
        let mut character_count = Utf32CodeUnits(0);
        let mut new_text = String::with_capacity(text.len());
        for iteration in TextTransformationIterator::new(
            &text,
            &info.style,
            self.last_inline_box_ended_with_collapsible_white_space,
            self.on_word_boundary,
        ) {
            offset_map.push_iteration(&iteration);
            for &character in iteration.characters() {
                character_count.0 += 1;

                // If this character has a strong right-to-left class the new inline formatting context will
                // need to be BiDi-aware. This match is derived from the list of strong right-to-left classes
                // at https://www.unicode.org/reports/tr44/#Bidi_Class_Values.
                self.has_right_to_left_content = self.has_right_to_left_content
                    || matches!(
                        bidi_class_map.get(character),
                        BidiClass::RightToLeft
                            | BidiClass::ArabicLetter
                            | BidiClass::RightToLeftEmbedding
                            | BidiClass::RightToLeftIsolate
                            | BidiClass::RightToLeftOverride
                    );

                self.is_empty = self.is_empty
                    && match white_space_collapse {
                        WhiteSpaceCollapse::Collapse => Self::is_document_white_space(character),
                        WhiteSpaceCollapse::PreserveBreaks => {
                            Self::is_document_white_space(character) && character != '\n'
                        },
                        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces => false,
                    };

                new_text.push(character)
            }
        }

        if new_text.is_empty() {
            return;
        }

        if let Some(last_character) = new_text.chars().next_back() {
            self.on_word_boundary = last_character.is_whitespace();
            self.last_inline_box_ended_with_collapsible_white_space =
                self.on_word_boundary && white_space_collapse != &WhiteSpaceCollapse::Preserve;
        }

        // TODO: ensure layout doesn’t handle more than 4 GiB at a time?
        let new_text_len = Utf8CodeUnits::length_of(AssumeUnder4GB, &new_text);
        let new_utf8_range = self.current_text_offset..self.current_text_offset + new_text_len;
        self.current_text_offset = new_utf8_range.end;

        let new_character_range =
            self.current_character_offset..self.current_character_offset + character_count;
        self.current_character_offset = new_character_range.end;

        self.text_segments.push(new_text);

        let current_inline_styles = self.shared_inline_styles();
        let box_slot = info.node.is_text_node().then(|| info.node.box_slot());
        let text_run = ArcRefCell::new(TextRun::new(
            info.into(),
            SharedTextRunData {
                text_content: self.text_content_slot.clone(),
                inline_styles: current_inline_styles,
                character_range_in_ifc_text: new_character_range,
                original_offset: original_size_before,
                selection: AtomicRefCell::new(selection),
                paint_caret: info.node.text_node_paints_caret(),
                offset_map: self.offset_map.clone(),
            }
            .into(),
            new_utf8_range,
            box_slot
                .as_ref()
                .and_then(|box_slot| box_slot.take_layout_box_as_text_run()),
        ));
        self.inline_items
            .push(InlineItem::TextRun(text_run.clone()));

        if let Some(box_slot) = box_slot {
            box_slot.set(LayoutBox::Text(text_run));
        }
    }

    pub(crate) fn enter_display_contents(&mut self, shared_inline_styles: SharedInlineStyles) {
        self.shared_inline_styles_stack.push(shared_inline_styles);
    }

    pub(crate) fn leave_display_contents(&mut self) {
        self.shared_inline_styles_stack.pop();
    }

    /// Finish the current inline formatting context, returning [`None`] if the context was empty.
    pub(crate) fn finish(
        self,
        layout_context: &LayoutContext,
        has_first_formatted_line: bool,
        is_single_line_text_input: bool,
        default_bidi_level: Level,
    ) -> Option<InlineFormattingContext> {
        if self.is_empty {
            return None;
        }

        assert!(self.inline_box_stack.is_empty());
        debug_assert_eq!(
            self.offset_map.borrow().total_final_size(),
            self.current_character_offset
        );

        Some(InlineFormattingContext::new_with_builder(
            self,
            layout_context,
            has_first_formatted_line,
            is_single_line_text_input,
            default_bidi_level,
        ))
    }

    /// Push a soft wrap opportunity to this [`InlineFormattingContextBuilder`] which pushes
    /// a zero-width space to the text content.
    pub(crate) fn push_soft_wrap_opportunity(&mut self) {
        self.push_control_character_string("\u{200B}");
    }
}

// ---------------------------------------------------------------------------
// Ruby box generation (CSS Ruby §2.2 / §2.3)
//
// BAO patch (fork-maintained, 2026-10-21, REQ-BRW-051 W1): upstream servo has no
// ruby layout. This section is the DOM-side half of the ruby integration: it
// classifies the children of a ruby container, runs the pure planner from
// `flow/ruby/plan.rs`, and materializes every base/annotation unit as an
// independent inline formatting context (one ruby cell).
// ---------------------------------------------------------------------------

/// The payload of one classified ruby child: the node and computed style that
/// are needed to materialize the child's content box once the plan exists.
pub(crate) type RubyPlanPayload<'dom> = NodeAndStyleInfo<'dom>;

/// The classified children of one ruby container, as consumed by
/// [`build_plan`].
pub(crate) type RubyPlanChildren<'dom> = Vec<RubyPlanChild<RubyPlanPayload<'dom>>>;

/// Whether an element is hidden by `visibility: collapse` (§2.3 hiding).
fn is_collapsed(style: &ComputedValues) -> bool {
    style.get_inherited_box().visibility == Visibility::Collapse
}

/// Classify one element child of a ruby container (or of an internal ruby
/// container) for box generation.
///
/// Classification is tag-first and display-second (HTML Ruby Extensions
/// §2.1.1); the DOM-side glue for that is
/// [`ruby_child_kind_for_element`]. Returns [`None`] for the children that a
/// ruby container ignores (`rp` elements, `display: none` subtrees).
fn classify_ruby_child<'dom>(
    context: &LayoutContext,
    info: &NodeAndStyleInfo<'dom>,
    contents: Contents,
) -> Option<RubyPlanChild<RubyPlanPayload<'dom>>> {
    let element = info.node.as_element();
    let local_name = element.as_ref().map(|element| element.local_name());
    let is_html_element_in_html_document = element
        .as_ref()
        .is_some_and(|element| element.is_html_element_in_html_document());
    let kind = ruby_child_kind_for_element(
        local_name,
        info.style.get_box().display,
        is_html_element_in_html_document,
    )?;

    // Internal ruby containers classify their own children recursively; every
    // other child is an atomic-like pair unit whose contents are materialized
    // later.
    let children = match (kind, contents) {
        (
            RubyChildKind::BaseContainer | RubyChildKind::AnnotationContainer,
            Contents::NonReplaced(non_replaced_contents),
        ) => RubyChildCollector::collect(context, info, non_replaced_contents),
        _ => Vec::new(),
    };

    Some(RubyPlanChild {
        kind,
        payload: info.clone(),
        children,
        text: info.node.text_content().to_string(),
        collapsed: is_collapsed(&info.style),
    })
}

/// Collects the classified flat-tree children of one ruby container.
///
/// `display: contents` children are flattened into the ruby container by the
/// traversal itself (their own `enter`/`leave` notifications carry no box), so
/// they need no handling here.
struct RubyChildCollector<'a, 'dom> {
    context: &'a LayoutContext<'a>,
    children: RubyPlanChildren<'dom>,
}

impl<'a, 'dom> RubyChildCollector<'a, 'dom> {
    fn collect(
        context: &'a LayoutContext<'a>,
        info: &NodeAndStyleInfo<'dom>,
        contents: NonReplacedContents,
    ) -> RubyPlanChildren<'dom> {
        let mut collector = Self {
            context,
            children: Vec::new(),
        };
        contents.traverse(context, info, &mut collector);
        collector.children
    }
}

impl<'dom> TraversalHandler<'dom> for RubyChildCollector<'_, 'dom> {
    fn handle_text(&mut self, info: &NodeAndStyleInfo<'dom>, text: BoxTreeString<'dom>) {
        if text.is_empty() {
            return;
        }
        self.children.push(RubyPlanChild {
            kind: ruby_child_kind_for_text(&text),
            payload: info.clone(),
            children: Vec::new(),
            text: text.to_string(),
            collapsed: is_collapsed(&info.style),
        });
    }

    fn handle_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        _display: DisplayGeneratingBox,
        contents: Contents,
        _box_slot: BoxSlot<'dom>,
    ) {
        if let Some(child) = classify_ruby_child(self.context, info, contents) {
            self.children.push(child);
        }
    }

    fn enter_display_contents(&mut self, _: SharedInlineStyles) {}

    fn leave_display_contents(&mut self) {}
}

/// The classified child list of the anonymous ruby container that wraps one
/// internal ruby box (CSS Ruby §2.2 rule 2).
pub(crate) fn anonymous_ruby_container_children<'dom>(
    context: &LayoutContext,
    info: &NodeAndStyleInfo<'dom>,
    contents: Contents,
) -> RubyPlanChildren<'dom> {
    classify_ruby_child(context, info, contents)
        .into_iter()
        .collect()
}

impl InlineFormattingContextBuilder {
    /// Push the ruby items of one ruby container into this inline formatting
    /// context: classify the children of the container, build the plan with the
    /// pure planner and materialize one [`InlineItem::RubyBase`] plus one
    /// [`InlineItem::Annotation`] per annotation level for every segment.
    pub(crate) fn push_ruby_container(
        &mut self,
        context: &LayoutContext,
        info: &NodeAndStyleInfo<'_>,
        contents: NonReplacedContents,
        propagated_data: PropagatedBoxTreeData,
    ) {
        let children = RubyChildCollector::collect(context, info, contents);
        self.push_ruby_segments(context, info, children, propagated_data);
    }

    /// Push the ruby items of a ruby container whose children have already been
    /// classified (used by the anonymous container generation of §2.2 rule 2).
    pub(crate) fn push_ruby_segments(
        &mut self,
        context: &LayoutContext,
        info: &NodeAndStyleInfo<'_>,
        children: RubyPlanChildren<'_>,
        propagated_data: PropagatedBoxTreeData,
    ) {
        let plan = build_plan(children);
        let bidi_level = info.style.to_bidi_level();
        for segment in plan.segments {
            let (base_cells, levels) =
                materialize_ruby_segment(context, info, segment, propagated_data);
            let segment_item = ArcRefCell::new(RubySegmentItem {
                base_cells,
                levels: levels.clone(),
                bidi_level,
            });
            self.inline_items.push(InlineItem::RubyBase(segment_item));
            for level in levels {
                self.inline_items.push(InlineItem::Annotation(level));
            }
            self.is_empty = false;
        }
    }
}

/// Materialize one segment of a ruby plan: the base cells and the annotation
/// levels with their own cells.
fn materialize_ruby_segment<'dom>(
    context: &LayoutContext,
    container_info: &NodeAndStyleInfo<'dom>,
    segment: RubyPlanSegment<RubyPlanPayload<'dom>>,
    propagated_data: PropagatedBoxTreeData,
) -> (Vec<RubyCellItem>, Vec<ArcRefCell<RubyAnnotationLevel>>) {
    let base_cells = segment
        .base_units
        .into_iter()
        .map(|unit| {
            let mut cell = materialize_ruby_unit(context, container_info, unit, propagated_data);
            // A base unit occupies exactly one column.
            cell.span_start = 0;
            cell.span_end = 1;
            cell
        })
        .collect();

    let levels = segment
        .levels
        .into_iter()
        .map(|level| {
            let position = ruby_annotation_position(container_info, &level);
            let cells = level
                .cells
                .into_iter()
                .map(|cell| {
                    let span_start = cell.span.start;
                    let span_end = cell.span.end;
                    let mut item =
                        materialize_ruby_unit(context, container_info, cell.unit, propagated_data);
                    item.span_start = span_start;
                    item.span_end = span_end;
                    item.hidden = cell.hidden;
                    item
                })
                .collect();
            ArcRefCell::new(RubyAnnotationLevel { cells, position })
        })
        .collect();

    (base_cells, levels)
}

/// The computed `ruby-position` of one annotation level: the explicit
/// annotation container's if there is one, otherwise the value inherited by the
/// first annotation that generated the level (which is the ruby container's).
fn ruby_annotation_position<'dom>(
    container_info: &NodeAndStyleInfo<'dom>,
    level: &RubyPlanLevel<RubyPlanPayload<'dom>>,
) -> style::values::computed::RubyPosition {
    level
        .container
        .element
        .as_ref()
        .or_else(|| {
            level.cells.iter().find_map(|cell| match &cell.unit.source {
                RubyPlanUnitSource::Element(info) => Some(info),
                _ => None,
            })
        })
        .map_or_else(
            || container_info.style.get_inherited_text().ruby_position,
            |info| info.style.get_inherited_text().ruby_position,
        )
}

/// Materialize one base or annotation unit of a ruby segment into a cell.
fn materialize_ruby_unit<'dom>(
    context: &LayoutContext,
    container_info: &NodeAndStyleInfo<'dom>,
    unit: RubyPlanUnit<RubyPlanPayload<'dom>>,
    propagated_data: PropagatedBoxTreeData,
) -> RubyCellItem {
    let (contents, whitespace_inline_size, align) = match unit.source {
        RubyPlanUnitSource::Element(info) => {
            let align = info.style.get_inherited_text().ruby_align;
            let cell = materialize_ruby_element_cell(context, &info, propagated_data);
            (Some(cell), None, align)
        },
        RubyPlanUnitSource::Content(nodes) => {
            let align = container_info.style.get_inherited_text().ruby_align;
            let cell =
                materialize_ruby_anonymous_cell(context, container_info, nodes, propagated_data);
            (Some(cell), None, align)
        },
        // CSS Ruby §2.2 rule 6: a surviving white space unit is a base unit of
        // its own. Its column keeps the advance of the space; the cell itself
        // generates no fragment.
        RubyPlanUnitSource::Text(text) => {
            let align = container_info.style.get_inherited_text().ruby_align;
            let width = ruby_whitespace_inline_size(context, container_info, &text);
            (None, Some(width), align)
        },
        // CSS Ruby §2.3: an unpaired extra base or annotation pairs with an
        // empty unit, which claims no width and generates no fragment.
        RubyPlanUnitSource::Empty => {
            let align = container_info.style.get_inherited_text().ruby_align;
            (None, None, align)
        },
    };
    RubyCellItem {
        contents,
        span_start: 0,
        span_end: 1,
        // `visibility: collapse` on a base still occupies its column (only
        // annotations are hidden without taking space, §2.3); painting honors
        // the visibility, so the cell stays in layout.
        hidden: false,
        whitespace_inline_size,
        align,
    }
}

/// The inline size of a surviving white space unit (§2.2 rule 6): the advance
/// of the space (or of the whole run under `white-space: preserve`) in the ruby
/// container's font.
///
/// A white space cell has no inline formatting context of its own: a space at
/// the start of its own line would be trimmed away by the line breaker, losing
/// the inter-base advance that §2.2 rule 6 preserves.
fn ruby_whitespace_inline_size(
    context: &LayoutContext,
    container_info: &NodeAndStyleInfo<'_>,
    text: &str,
) -> Au {
    let style = &container_info.style;
    let Some(font) = get_font_for_first_font_for_style(style, &context.font_context) else {
        return Au::zero();
    };
    let inherited_text = style.get_inherited_text();
    let font_size = style.get_font().font_size.computed_size().into();
    let word_spacing = inherited_text.word_spacing.to_used_value(font_size);
    let letter_spacing = inherited_text.letter_spacing.0.to_used_value(font_size);
    let single_space_advance = font.metrics().space_advance + word_spacing + letter_spacing;
    let count = match inherited_text.white_space_collapse {
        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces => text.chars().count(),
        WhiteSpaceCollapse::Collapse | WhiteSpaceCollapse::PreserveBreaks => 1,
    };
    single_space_advance.scale_by(count as f32)
}

/// Builds the content of one ruby base or annotation cell as an independent
/// inline formatting context.
///
/// CSS Ruby §2.2 rule 1: block-level children of a ruby container are
/// inlinified. Inside a ruby base or annotation, every flow-level child is
/// therefore materialized as an inline box regardless of its outer display
/// type; replaced elements and non-flow inner display types become atomic
/// inline-level independent formatting contexts.
struct RubyUnitBuilder<'a, 'dom> {
    context: &'a LayoutContext<'a>,
    propagated_data: PropagatedBoxTreeData,
    info: NodeAndStyleInfo<'dom>,
    builder: InlineFormattingContextBuilder,
}

impl<'a, 'dom> RubyUnitBuilder<'a, 'dom> {
    fn new(
        context: &'a LayoutContext<'a>,
        info: &NodeAndStyleInfo<'dom>,
        propagated_data: PropagatedBoxTreeData,
    ) -> Self {
        Self {
            context,
            propagated_data,
            info: info.clone(),
            builder: InlineFormattingContextBuilder::new(info, context, false),
        }
    }

    /// Push one node of an anonymous unit: the text of a text node, or the box
    /// of an element.
    fn push_node(&mut self, info: &NodeAndStyleInfo<'dom>) {
        let node = info.node;
        if node.is_text_node() {
            let selection = node.text_node_selection();
            let text = node.text_content();
            if text.is_empty() {
                return;
            }
            self.builder.push_text(text.into(), info, selection);
            return;
        }

        match Display::from(info.style.get_box().display) {
            Display::None => {},
            Display::Contents => {
                let shared_inline_styles =
                    SharedInlineStyles::from_info_and_context(info, self.context);
                node.box_slot()
                    .set(LayoutBox::DisplayContents(shared_inline_styles.clone()));
                self.builder.enter_display_contents(shared_inline_styles);
                NonReplacedContents::OfElement.traverse(self.context, info, self);
                self.builder.leave_display_contents();
            },
            Display::GeneratingBox(display) => {
                let contents = Contents::for_element(node, self.context);
                let display = display.used_value_for_contents(&contents, info);
                self.push_element_box(info, display, contents);
            },
        }
    }

    fn push_element_box(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display: DisplayGeneratingBox,
        contents: Contents,
    ) {
        match display {
            DisplayGeneratingBox::OutsideInside { inside, .. } => match (inside, contents) {
                (DisplayInside::Flow { .. }, Contents::NonReplaced(non_replaced_contents)) => {
                    let old_layout_box = info.node.box_slot().take_layout_box();
                    let inline_item = self.builder.start_inline_box(
                        || ArcRefCell::new(InlineBox::new(info, self.context)),
                        old_layout_box,
                    );
                    info.node
                        .box_slot()
                        .set(LayoutBox::InlineLevel(inline_item));
                    non_replaced_contents.traverse(self.context, info, self);
                    self.builder.end_inline_box();
                },
                (inside, contents) => {
                    let atomic = self.push_atomic(info, inside, contents);
                    info.node.box_slot().set(LayoutBox::InlineLevel(atomic));
                },
            },
            DisplayGeneratingBox::LayoutInternal(layout_internal) => {
                let atomic = self.push_atomic(info, layout_internal.display_inside(), contents);
                info.node.box_slot().set(LayoutBox::InlineLevel(atomic));
            },
        }
    }

    fn push_atomic(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
    ) -> InlineItem {
        let context = self.context;
        let propagated_data = self.propagated_data;
        let old_layout_box = info.node.box_slot().take_layout_box();
        self.builder.push_atomic(
            || {
                ArcRefCell::new(IndependentFormattingContext::construct(
                    context,
                    info,
                    display_inside,
                    contents,
                    propagated_data,
                ))
            },
            old_layout_box,
        )
    }

    /// Finish the cell: an independent formatting context whose contents are
    /// the inline formatting context built above.
    fn finish(mut self) -> ArcRefCell<IndependentFormattingContext> {
        let bidi_level = self.info.style.to_bidi_level();
        // A ruby cell always generates a box, even when its content is empty,
        // so that its column and its fragment position are well defined.
        self.builder.is_empty = false;
        let inline_formatting_context = self
            .builder
            .finish(self.context, false, false, bidi_level)
            .expect("A ruby cell IFC always generates a root inline box");
        ArcRefCell::new(IndependentFormattingContext::new(
            LayoutBoxBase::new((&self.info).into(), self.info.style.clone()),
            IndependentFormattingContextContents::Flow(
                BlockFormattingContext::from_block_container(
                    BlockContainer::InlineFormattingContext(inline_formatting_context),
                ),
            ),
            self.propagated_data,
        ))
    }
}

impl<'dom> TraversalHandler<'dom> for RubyUnitBuilder<'_, 'dom> {
    fn handle_text(&mut self, info: &NodeAndStyleInfo<'dom>, text: BoxTreeString<'dom>) {
        if text.is_empty() {
            return;
        }
        self.builder
            .push_text(text, info, info.node.text_node_selection());
    }

    fn handle_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display: DisplayGeneratingBox,
        contents: Contents,
        _box_slot: BoxSlot<'dom>,
    ) {
        RubyUnitBuilder::push_element_box(self, info, display, contents);
    }

    fn enter_display_contents(&mut self, styles: SharedInlineStyles) {
        self.builder.enter_display_contents(styles);
    }

    fn leave_display_contents(&mut self) {
        self.builder.leave_display_contents();
    }
}

/// Materialize the cell of one explicit ruby base or annotation element.
fn materialize_ruby_element_cell<'dom>(
    context: &LayoutContext,
    info: &NodeAndStyleInfo<'dom>,
    propagated_data: PropagatedBoxTreeData,
) -> ArcRefCell<IndependentFormattingContext> {
    // Ruby cells are rebuilt from their element's content on every box tree
    // construction; reuse across layouts is a later optimization.
    let _ = info.node.box_slot().take_layout_box();
    let contents = Contents::for_element(info.node, context);
    let cell = match contents {
        Contents::NonReplaced(non_replaced_contents) => {
            let mut unit_builder = RubyUnitBuilder::new(context, info, propagated_data);
            non_replaced_contents.traverse(context, info, &mut unit_builder);
            unit_builder.finish()
        },
        // A replaced element (or a widget) that is a ruby base or annotation
        // generates its own atomic inline box inside the segment; the normal
        // independent formatting context construction handles its intrinsic
        // size and its replaced content.
        contents @ (Contents::Replaced(_) | Contents::Widget(_)) => {
            ArcRefCell::new(IndependentFormattingContext::construct(
                context,
                info,
                DisplayInside::FlowRoot {
                    is_list_item: false,
                },
                contents,
                propagated_data,
            ))
        },
    };
    info.node
        .box_slot()
        .set(LayoutBox::InlineLevel(InlineItem::Atomic(
            cell.clone(),
            Utf8CodeUnits::default(),
            Level::ltr(),
        )));
    cell
}

/// Materialize the cell of one anonymous unit made of bare inline content.
fn materialize_ruby_anonymous_cell<'dom>(
    context: &LayoutContext,
    container_info: &NodeAndStyleInfo<'dom>,
    nodes: Vec<RubyPlanPayload<'dom>>,
    propagated_data: PropagatedBoxTreeData,
) -> ArcRefCell<IndependentFormattingContext> {
    let anonymous_info = ruby_anonymous_info(context, container_info);
    let mut unit_builder = RubyUnitBuilder::new(context, &anonymous_info, propagated_data);
    for node_info in nodes {
        unit_builder.push_node(&node_info);
    }
    unit_builder.finish()
}

/// The anonymous box style inherited by the anonymous base/annotation units of
/// one ruby container (§2.2 rule 3).
fn ruby_anonymous_info<'dom>(
    context: &LayoutContext,
    container_info: &NodeAndStyleInfo<'dom>,
) -> NodeAndStyleInfo<'dom> {
    container_info
        .with_pseudo_element(context, PseudoElement::ServoAnonymousBox)
        .expect("Should always be able to construct info for anonymous boxes")
}

/// Construct the block container of a ruby container that establishes an
/// independent formatting context (float, absolutely positioned, flex/grid
/// item, `display: block ruby`). The container always generates a block box,
/// even when it has no content.
pub(crate) fn construct_ruby_container<'dom>(
    context: &LayoutContext,
    info: &NodeAndStyleInfo<'dom>,
    contents: NonReplacedContents,
    propagated_data: PropagatedBoxTreeData,
) -> BlockContainer {
    let mut builder = InlineFormattingContextBuilder::new(info, context, false);
    builder.is_empty = false;
    builder.push_ruby_container(context, info, contents, propagated_data);
    let inline_formatting_context = builder
        .finish(context, false, false, info.style.to_bidi_level())
        .expect("A ruby container IFC always generates a root inline box");
    BlockContainer::InlineFormattingContext(inline_formatting_context)
}

/// Computes the range of the first letter.
///
/// The range includes any preceding punctuation and white space, and any trailing punctuation. Any
/// non-punctuation following the letter/number/symbol of first-letter ends the range. Intervening
/// spaces within trailing punctuation are not supported yet.
///
/// If the resulting range is empty, no compatible first-letter text was found.
///
/// <https://drafts.csswg.org/css-pseudo/#first-letter-pattern>
fn first_letter_range(text: &str) -> Range<usize> {
    enum State {
        /// All characters that precede the `PrecedingWhitespaceAndPunctuation` state.
        Start,
        /// All preceding punctuation and intervening whitepace that precedes the `Lns` state.
        PrecedingPunctuation,
        /// Unicode general category L: letter, N: number and S: symbol
        Lns,
        /// All punctuation (but no whitespace or other characters), that
        /// come after the `Lns` state.
        TrailingPunctuation,
    }

    let mut start = 0;
    let mut state = State::Start;
    for (index, character) in text.char_indices() {
        match &mut state {
            State::Start => {
                if character.is_letter() || character.is_number() || character.is_symbol() {
                    start = index;
                    state = State::Lns;
                } else if character.is_punctuation() {
                    start = index;
                    state = State::PrecedingPunctuation
                }
            },
            State::PrecedingPunctuation => {
                if character.is_letter() || character.is_number() || character.is_symbol() {
                    state = State::Lns;
                } else if !character.is_separator_space() && !character.is_punctuation() {
                    return 0..0;
                }
            },
            State::Lns => {
                // TODO: Implement support for intervening spaces
                // <https://drafts.csswg.org/css-pseudo/#first-letter-pattern>
                if character.is_punctuation()
                    && !character.is_punctuation_open()
                    && !character.is_punctuation_dash()
                {
                    state = State::TrailingPunctuation;
                } else {
                    return start..index;
                }
            },
            State::TrailingPunctuation => {
                // TODO: Implement support for intervening spaces
                // <https://drafts.csswg.org/css-pseudo/#first-letter-pattern>
                if character.is_punctuation()
                    && !character.is_punctuation_open()
                    && !character.is_punctuation_dash()
                {
                    continue;
                } else {
                    return start..index;
                }
            },
        }
    }

    match state {
        State::Start | State::PrecedingPunctuation => 0..0,
        State::Lns | State::TrailingPunctuation => start..text.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_first_letter_eq(text: &str, expected: &str) {
        let range = first_letter_range(text);
        assert_eq!(&text[range], expected);
    }

    #[test]
    fn test_first_letter_range() {
        // All spaces
        assert_first_letter_eq("", "");
        assert_first_letter_eq("  ", "");

        // Spaces and punctuation only
        assert_first_letter_eq("(", "");
        assert_first_letter_eq(" (", "");
        assert_first_letter_eq("( ", "");
        assert_first_letter_eq("()", "");

        // Invalid chars
        assert_first_letter_eq("\u{0903}", "");

        // First letter only
        assert_first_letter_eq("A", "A");
        assert_first_letter_eq(" A", "A");
        assert_first_letter_eq("A ", "A");
        assert_first_letter_eq(" A ", "A");

        // Word
        assert_first_letter_eq("App", "A");
        assert_first_letter_eq(" App", "A");
        assert_first_letter_eq("App ", "A");

        // Preceding punctuation(s), intervening spaces and first letter
        assert_first_letter_eq(r#""A"#, r#""A"#);
        assert_first_letter_eq(r#" "A"#, r#""A"#);
        assert_first_letter_eq(r#""A "#, r#""A"#);
        assert_first_letter_eq(r#"" A"#, r#"" A"#);
        assert_first_letter_eq(r#" "A "#, r#""A"#);
        assert_first_letter_eq(r#"("A"#, r#"("A"#);
        assert_first_letter_eq(r#" ("A"#, r#"("A"#);
        assert_first_letter_eq(r#"( "A"#, r#"( "A"#);
        assert_first_letter_eq(r#"[ ( "A"#, r#"[ ( "A"#);

        // First letter and succeeding punctuation(s)
        // TODO: modify test cases when intervening spaces in succeeding puntuations is supported
        assert_first_letter_eq(r#"A""#, r#"A""#);
        assert_first_letter_eq(r#"A" "#, r#"A""#);
        assert_first_letter_eq(r#"A)]"#, r#"A)]"#);
        assert_first_letter_eq(r#"A" )]"#, r#"A""#);
        assert_first_letter_eq(r#"A)] >"#, r#"A)]"#);

        // All
        assert_first_letter_eq(r#" ("A" )]"#, r#"("A""#);
        assert_first_letter_eq(r#" ("A")] >"#, r#"("A")]"#);

        // Non ASCII chars
        assert_first_letter_eq("一", "一");
        assert_first_letter_eq(" 一 ", "一");
        assert_first_letter_eq("一二三", "一");
        assert_first_letter_eq(" 一二三 ", "一");
        assert_first_letter_eq("（一二三）", "（一");
        assert_first_letter_eq(" （一二三） ", "（一");
        assert_first_letter_eq("（（一", "（（一");
        assert_first_letter_eq(" （ （一", "（ （一");
        assert_first_letter_eq("一）", "一）");
        assert_first_letter_eq("一））", "一））");
        assert_first_letter_eq("一） ）", "一）");
    }
}
