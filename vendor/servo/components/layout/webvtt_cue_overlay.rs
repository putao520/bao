/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebVTT cue box overlay: text shaping and CSS WebVTT box geometry for the
//! cue boxes the script thread reports as active on a media element
//! (BAO patch, fork-maintained, 2026-09-27, REQ-BRW-047).
//!
//! The script side owns the *what* (which cues are active and their settings,
//! see `HTMLMediaElement::update_active_cue_render_snapshot`); this module
//! owns the *where*: the box geometry follows the CSS WebVTT rendering model
//! (<https://www.w3.org/TR/webvtt1/#css-definitions>) — `line`, `position`,
//! position alignment, text alignment and `size` resolved against the video
//! content box — and the text is shaped with the same font pipeline the
//! inline layout uses. The overlays are painted as part of the video's image
//! fragment; they never become page-visible DOM.

use std::sync::Arc;

use app_units::Au;
use fonts::{FontMetrics, ShapedTextSlice, ShapedTextSliceType, ShapedTextSlicer, ShapingOptions};
use layout_api::{WebVttCueBoxData, WebVttPositionAlign, WebVttTextAlign};
use servo_arc::Arc as ServoArc;
use style::properties::style_structs::Font as FontStyleStruct;
use style::Zero;
use webrender_api::FontInstanceKey;

use crate::context::LayoutContext;
use crate::flow::inline::text_run::FontAndScriptInfo;
use crate::fragment_tree::CueTextOverlay;
use crate::geom::PhysicalPoint;

/// Shape `text` with the font family/size from `style`, using the same
/// shaping entry point (`Font::shape_text`) the inline formatting context
/// uses.
fn shape_line(
    layout_context: &LayoutContext,
    font_style: ServoArc<FontStyleStruct>,
    text: &str,
) -> Option<(Vec<Arc<ShapedTextSlice>>, Arc<FontMetrics>, FontInstanceKey)> {
    if text.is_empty() {
        return None;
    }
    let font_group = layout_context.font_context.font_group(font_style);
    let font = font_group.first(&layout_context.font_context)?;

    // Default shaping configuration: cue text uses the UA cue defaults, not
    // the video element's letter/word spacing.
    let font_and_script_info = FontAndScriptInfo::simple_for_font(font.clone());
    let options: ShapingOptions = (&font_and_script_info).into();

    let shaped_text = font.shape_text(text, &options);
    let mut slicer = ShapedTextSlicer::new(shaped_text);
    let slice = slicer.slice_until_character_offset(
        text.chars().count(),
        ShapedTextSliceType::Word,
    )?;

    Some((
        vec![slice],
        font.metrics.clone(),
        font.key(layout_context.painter_id, &layout_context.font_context),
    ))
}

/// Resolve one cue box's geometry and shape its text lines.
pub(crate) fn build_cue_overlays(
    layout_context: &LayoutContext,
    font_style: ServoArc<FontStyleStruct>,
    cue_boxes: &[WebVttCueBoxData],
    content_width: Au,
    content_height: Au,
) -> Vec<CueTextOverlay> {
    if cue_boxes.is_empty() || content_width <= Au::zero() || content_height <= Au::zero() {
        return Vec::new();
    }

    let mut overlays = Vec::new();
    // Vertical stacking offset for cues with an automatic line position:
    // they sit at the bottom of the video box, in cue order, without gaps.
    let mut auto_line_block_offset = Au::zero();

    for cue_box in cue_boxes {
        // Shape every line of the cue, remembering each line's advance.
        let mut shaped_lines = Vec::new();
        let mut max_line_advance = Au::zero();
        for text_line in &cue_box.text_lines {
            let Some((glyphs, metrics, font_key)) =
                shape_line(layout_context, font_style.clone(), text_line)
            else {
                continue;
            };
            let advance = glyphs.iter().map(|slice| slice.total_advance()).sum();
            max_line_advance.max_assign(advance);
            shaped_lines.push((glyphs, metrics, font_key, advance));
        }
        let Some(&(_, ref first_metrics, _, _)) = shaped_lines.first() else {
            continue;
        };
        let line_gap = first_metrics.line_gap;
        let ascent = first_metrics.ascent;
        let line_count = shaped_lines.len();

        // Box size: width from the cue `size` percentage (at least wide
        // enough for the widest line), height from the number of text lines.
        let box_width = content_width
            .scale_by((cue_box.size / 100.).clamp(0., 1.) as f32)
            .max(max_line_advance);
        let box_height = line_gap.scale_by(line_count as f32);

        // Horizontal position: the `position` percentage anchors the box
        // according to the position alignment; `auto` resolves from the text
        // alignment, like the CSS WebVTT model's automatic position alignment.
        let position = cue_box.position.unwrap_or(50.).clamp(0., 100.);
        let position_align = match cue_box.position_align {
            WebVttPositionAlign::LineLeft => WebVttPositionAlign::LineLeft,
            WebVttPositionAlign::Center => WebVttPositionAlign::Center,
            WebVttPositionAlign::LineRight => WebVttPositionAlign::LineRight,
            WebVttPositionAlign::Auto => match cue_box.align {
                WebVttTextAlign::Start | WebVttTextAlign::Left => WebVttPositionAlign::LineLeft,
                WebVttTextAlign::Center => WebVttPositionAlign::Center,
                WebVttTextAlign::End | WebVttTextAlign::Right => WebVttPositionAlign::LineRight,
            },
        };
        let anchor_offset = match position_align {
            WebVttPositionAlign::LineLeft => Au::zero(),
            WebVttPositionAlign::Center => box_width / 2,
            WebVttPositionAlign::LineRight => box_width,
            WebVttPositionAlign::Auto => box_width,
        };
        let box_left = (content_width.scale_by((position / 100.) as f32) - anchor_offset)
            .max(Au::zero())
            .min((content_width - box_width).max(Au::zero()));

        // Vertical position: a numeric `line` places the box inside the video
        // box (snap-to-lines) or in the remaining space (no snap-to-lines);
        // an automatic `line` bottom-stacks the boxes in cue order.
        let box_top = match (cue_box.line, cue_box.snap_to_lines) {
            (Some(line), true) => content_height
                .scale_by((line / 100.).clamp(0., 1.) as f32)
                .min((content_height - box_height).max(Au::zero())),
            (Some(line), false) => (content_height - box_height)
                .max(Au::zero())
                .scale_by((line / 100.).clamp(0., 1.) as f32),
            (None, _) => {
                let top = content_height - box_height - auto_line_block_offset;
                auto_line_block_offset += box_height;
                top.max(Au::zero())
            },
        };

        for (line_index, (glyphs, metrics, font_key, advance)) in
            shaped_lines.into_iter().enumerate()
        {
            // Text alignment positions the line within the box.
            let line_x = match cue_box.align {
                WebVttTextAlign::Start | WebVttTextAlign::Left => Au::zero(),
                WebVttTextAlign::Center => (box_width - advance).max(Au::zero()) / 2,
                WebVttTextAlign::End | WebVttTextAlign::Right => {
                    (box_width - advance).max(Au::zero())
                },
            };
            let baseline_origin = PhysicalPoint::new(
                box_left + line_x,
                box_top + line_gap.scale_by(line_index as f32) + ascent.min(metrics.ascent),
            );
            overlays.push(CueTextOverlay {
                glyphs,
                font_key,
                font_metrics: metrics,
                baseline_origin,
            });
        }
    }

    overlays
}
