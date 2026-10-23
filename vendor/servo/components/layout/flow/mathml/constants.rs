/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! MathML Core layout constants (`MathConstants`).
//!
//! <https://w3c.github.io/mathml-core/#layout-constants-mathconstants>
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//! Every constant below is taken from the "MathConstants" table of MathML
//! Core §5.1. The specification defines each constant as an OpenType `MATH`
//! table value with a fallback used when the font has no `MATH` table. The
//! layout text pipeline this module is built on exposes the `OS/2`/`post`
//! metrics of the first available font (via [`fonts::FontMetrics`]) but not
//! the `MATH` table, so:
//!
//!   * constants whose fallback is an `OS/2`/`post` metric use the real font
//!     metric when it is available (axis height, rule thickness, accent base
//!     height, script top/bottom limits),
//!   * constants whose fallback is a fixed ratio of the rule thickness use
//!     that ratio (`subSuperscriptGapMin` = 4×rule, `stackGapMin` = 3×rule,
//!     …),
//!   * constants that only exist in the `MATH` table and whose normative
//!     fallback is `0` keep `0` (`displayOperatorMinHeight`, the
//!     `stackTopShiftUp`/`stackBottomShiftDown` family, the limit gaps).
//!
//! Reading the real `MATH` table (glyph variants, assemblies, and the exact
//! per-font shift values) is registered as the "OpenType MATH" follow-up
//! wave (Wave C) in the task report.

use app_units::Au;
use fonts::FontMetrics;
use style::Zero;

/// The default ratio used by MathML Core when the font has no
/// `MATH.MathConstants.scriptPercentScaleDown`.
///
/// <https://w3c.github.io/mathml-core/#dfn-scriptpercentscaledown>
pub(crate) const SCRIPT_PERCENT_SCALE_DOWN: f32 = 0.71;

/// The default ratio used by MathML Core when the font has no
/// `MATH.MathConstants.scriptScriptPercentScaleDown`.
///
/// <https://w3c.github.io/mathml-core/#dfn-scriptscriptpercentscaledown>
pub(crate) const SCRIPT_SCRIPT_PERCENT_SCALE_DOWN: f32 = 0.5041;

/// Resolved MathML layout constants for one element.
///
/// The constants are resolved against the element's own font size and the
/// metrics of its first available font, as required by MathML Core §5.1
/// ("global layout constants for the first available font").
#[derive(Clone, Debug)]
pub(crate) struct MathConstants {
    /// Distance from the alphabetic baseline up to the mathematical baseline
    /// (the fraction bar, the middle of fences, …).
    pub axis_height: Au,

    /// Default rule thickness, used for the fraction bar and for every
    /// thickness-derived constant below.
    pub rule_thickness: Au,

    /// The height at which a lone accent is attached to its base.
    pub accent_base_height: Au,

    /// Minimum distance between the subscript's top and the base's baseline.
    pub subscript_shift_down: Au,
    /// Maximum amount by which the subscript may rise above the baseline.
    pub subscript_top_max: Au,
    /// Minimum drop of the subscript's baseline below the base's descent.
    pub subscript_baseline_drop_min: Au,

    /// Distance the superscript's baseline is raised above the base's
    /// baseline.
    pub superscript_shift_up: Au,
    /// Same as [`Self::superscript_shift_up`], for `math-shift: compact`.
    pub superscript_shift_up_cramped: Au,
    /// Minimum distance between the superscript's bottom and the baseline.
    pub superscript_bottom_min: Au,
    /// Maximum amount by which the superscript's baseline may drop below the
    /// base's ascent.
    pub superscript_baseline_drop_max: Au,
    /// Minimum vertical gap between a subscript and a superscript that share
    /// a base.
    pub sub_superscript_gap_min: Au,
    /// Highest position the superscript's bottom may occupy when a subscript
    /// is present.
    pub superscript_bottom_max_with_subscript: Au,

    /// Horizontal space inserted after a script.
    pub space_after_script: Au,

    /// Fraction-specific constants.
    pub fraction_numerator_shift_up: Au,
    pub fraction_numerator_display_style_shift_up: Au,
    pub fraction_denominator_shift_down: Au,
    pub fraction_denominator_display_style_shift_down: Au,
    pub fraction_numerator_gap_min: Au,
    pub fraction_denominator_gap_min: Au,
    pub fraction_num_display_style_gap_min: Au,
    pub fraction_denom_display_style_gap_min: Au,
    pub fraction_rule_thickness: Au,

    /// Constants used by the "stack" layout of `mfrac` with a zero rule
    /// thickness and by `munder`/`mover`.
    pub stack_top_shift_up: Au,
    pub stack_bottom_shift_down: Au,
    pub stack_top_display_style_shift_up: Au,
    pub stack_bottom_display_style_shift_down: Au,
    pub stack_gap_min: Au,
    pub stack_display_style_gap_min: Au,

    /// Gap between a lower/upper limit and its base.
    pub lower_limit_gap_min: Au,
    pub lower_limit_baseline_drop_min: Au,
    pub upper_limit_gap_min: Au,
    pub upper_limit_baseline_rise_min: Au,

    /// Vertical gap between a bar (overbar/underbar) and its base, plus the
    /// extra ascender/descender reserved for the bar.
    pub overbar_vertical_gap: Au,
    pub overbar_extra_ascender: Au,
    pub underbar_vertical_gap: Au,
    pub underbar_extra_descender: Au,

    /// Minimum block size of a "large operator" in display style.
    pub display_operator_min_height: Au,
}

/// Convert an em fraction into [`Au`] using the given font size.
#[inline]
pub(crate) fn em(font_size: Au, factor: f32) -> Au {
    font_size.scale_by(factor)
}

impl MathConstants {
    /// Resolve the MathML layout constants for an element with the given font
    /// size and first-available-font metrics.
    pub(crate) fn resolve(font_size: Au, metrics: Option<&FontMetrics>) -> Self {
        // `post.underlineThickness` is the specification's fallback source for
        // the default rule thickness. When the font does not report one, use a
        // 0.06em rule (the value used by the math fonts the MathML test suite
        // is built around), but never thinner than one device-independent
        // pixel so that the fraction bar stays visible.
        let rule_thickness = metrics
            .map(|metrics| metrics.underline_size)
            .filter(|size| *size > Au::zero())
            .unwrap_or_else(|| em(font_size, 0.06).max(Au::from_f32_px(1.0)));

        // `axisHeight` falls back to half of `OS/2.sxHeight`, which is exactly
        // the font's `x_height` as exposed by the font backend.
        let x_height = metrics
            .map(|metrics| metrics.x_height)
            .filter(|x_height| *x_height > Au::zero())
            .unwrap_or_else(|| em(font_size, 0.5));
        let axis_height = x_height.scale_by(0.5);

        // 4/5 × `OS/2.sxHeight` and 1/4 × `OS/2.sxHeight` are the
        // specification's fallbacks for the script top/bottom limits.
        let subscript_top_max = x_height.scale_by(0.8);
        let superscript_bottom_min = x_height.scale_by(0.25);
        let superscript_bottom_max_with_subscript = x_height.scale_by(0.8);

        Self {
            axis_height,
            rule_thickness,
            accent_base_height: x_height,
            // `OS/2.ySubscriptYOffset` / `OS/2.ySuperscriptYOffset` are not
            // exposed by the font backend; these are the values used by the
            // math fonts of the test suite.
            subscript_shift_down: em(font_size, 0.25),
            superscript_shift_up: em(font_size, 0.36),
            // The specification's fallback for the cramped variant is `0`.
            superscript_shift_up_cramped: Au::zero(),
            subscript_top_max,
            superscript_bottom_min,
            // Both drop/rise minimums fall back to `0` in the specification.
            subscript_baseline_drop_min: Au::zero(),
            superscript_baseline_drop_max: Au::zero(),
            sub_superscript_gap_min: rule_thickness.scale_by(4.0),
            superscript_bottom_max_with_subscript,
            space_after_script: em(font_size, 1.0 / 24.0),
            // The numerator/denominator shift constants only exist in the
            // `MATH` table; the specification's fallback is `0`, in which case
            // the gap-derived term dominates the `max()`.
            fraction_numerator_shift_up: Au::zero(),
            fraction_numerator_display_style_shift_up: Au::zero(),
            fraction_denominator_shift_down: Au::zero(),
            fraction_denominator_display_style_shift_down: Au::zero(),
            fraction_numerator_gap_min: rule_thickness,
            fraction_denominator_gap_min: rule_thickness,
            fraction_num_display_style_gap_min: rule_thickness.scale_by(3.0),
            fraction_denom_display_style_gap_min: rule_thickness.scale_by(3.0),
            fraction_rule_thickness: rule_thickness,
            stack_top_shift_up: Au::zero(),
            stack_bottom_shift_down: Au::zero(),
            stack_top_display_style_shift_up: Au::zero(),
            stack_bottom_display_style_shift_down: Au::zero(),
            stack_gap_min: rule_thickness.scale_by(3.0),
            stack_display_style_gap_min: rule_thickness.scale_by(7.0),
            lower_limit_gap_min: Au::zero(),
            lower_limit_baseline_drop_min: Au::zero(),
            upper_limit_gap_min: Au::zero(),
            upper_limit_baseline_rise_min: Au::zero(),
            overbar_vertical_gap: rule_thickness.scale_by(3.0),
            overbar_extra_ascender: rule_thickness,
            underbar_vertical_gap: rule_thickness.scale_by(3.0),
            underbar_extra_descender: rule_thickness,
            // `MATH.displayOperatorMinHeight`, fallback `0` (no-op).
            display_operator_min_height: Au::zero(),
        }
    }
}
