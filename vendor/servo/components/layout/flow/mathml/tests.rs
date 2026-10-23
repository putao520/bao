/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Unit tests for the pure parts of MathML layout: the layout constants, the
//! operator dictionary resolution and the presentation-attribute parsers.
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21). These tests do not
//! need a `LayoutContext`, so they can run without a document or a font
//! backend. The geometry of a laid out expression is covered by the MathML
//! tests of the WPT suite (`mathml/presentation-markup/**`).

use app_units::Au;
use style::values::computed::MathStyle;

use super::constants::{MathConstants, SCRIPT_PERCENT_SCALE_DOWN};
use super::dictionary::{self, MathSpace, OperatorAttributes, OperatorForm, OperatorProperties};
use super::{MathElementKind, MathLength, parse_length};

fn close(left: Au, right: Au) -> bool {
    (left - right).abs() <= Au::new(1)
}

/// A font size of 10px, i.e. 600 app units.
fn font_size() -> Au {
    Au::from_px(10)
}

#[test]
fn constants_fall_back_to_the_specification_defaults_without_a_font() {
    let constants = MathConstants::resolve(font_size(), None);

    // The specification falls back to half of `OS/2.sxHeight`, and a missing
    // x-height is assumed to be 0.5em (CSS `ex` fallback).
    assert!(close(constants.axis_height, Au::from_px(3)));

    // `post.underlineThickness` falls back to a 0.06em rule, but never below
    // one pixel.
    assert!(close(constants.rule_thickness, Au::from_px(1)));

    // The script scaling itself is applied by the MathML user agent sheet
    // (`mathml.css`), because this stylo snapshot has no `font-size: math`.
    assert_eq!(SCRIPT_PERCENT_SCALE_DOWN, 0.71);

    // Thickness-derived constants.
    assert!(close(
        constants.sub_superscript_gap_min,
        constants.rule_thickness.scale_by(4.0)
    ));
    assert!(close(
        constants.stack_display_style_gap_min,
        constants.rule_thickness.scale_by(7.0)
    ));
    assert!(close(
        constants.overbar_vertical_gap,
        constants.rule_thickness.scale_by(3.0)
    ));

    // `MATH`-only constants keep the specification's `0` fallback.
    assert_eq!(constants.display_operator_min_height, Au::zero());
    assert_eq!(constants.fraction_numerator_shift_up, Au::zero());
    assert_eq!(constants.upper_limit_gap_min, Au::zero());
}

#[test]
fn constants_scale_with_the_font_size() {
    let small = MathConstants::resolve(Au::from_px(10), None);
    let large = MathConstants::resolve(Au::from_px(20), None);

    // Em-relative constants double with the font size.
    assert!(close(
        large.space_after_script,
        small.space_after_script.scale_by(2.0)
    ));
    assert!(close(
        large.subscript_shift_down,
        small.subscript_shift_down.scale_by(2.0)
    ));
}

#[test]
fn dictionary_looks_up_operator_spacing_for_each_form() {
    let attributes = OperatorAttributes::default();

    let (form, plus_infix) = dictionary::resolve(Some('+'), OperatorForm::Infix, &attributes);
    assert_eq!(form, OperatorForm::Infix);
    assert!(close(
        plus_infix.lspace.resolve(font_size()),
        font_size().scale_by(4.0 / 18.0)
    ));
    assert!(close(
        plus_infix.rspace.resolve(font_size()),
        font_size().scale_by(4.0 / 18.0)
    ));

    // A prefix `+` carries no spacing.
    let (_, plus_prefix) = dictionary::resolve(Some('+'), OperatorForm::Prefix, &attributes);
    assert_eq!(plus_prefix.lspace.resolve(font_size()), Au::zero());
    assert_eq!(plus_prefix.rspace.resolve(font_size()), Au::zero());
}

#[test]
fn dictionary_fences_are_stretchy_and_symmetric() {
    let attributes = OperatorAttributes::default();
    let (_, open) = dictionary::resolve(Some('('), OperatorForm::Prefix, &attributes);
    assert!(open.stretchy);
    assert!(open.symmetric);
    assert!(open.fence);
    assert!(!open.largeop);
    assert_eq!(open.lspace.resolve(font_size()), Au::zero());

    let (_, close) = dictionary::resolve(Some(')'), OperatorForm::Postfix, &attributes);
    assert!(close.stretchy);
    assert!(close.fence);
}

#[test]
fn dictionary_large_operators_are_symmetric_with_movable_limits() {
    let attributes = OperatorAttributes::default();
    let (_, sum) = dictionary::resolve(Some('\u{2211}'), OperatorForm::Prefix, &attributes);
    assert!(sum.largeop);
    assert!(sum.symmetric);
    assert!(sum.movablelimits);

    let (_, integral) = dictionary::resolve(Some('\u{222B}'), OperatorForm::Prefix, &attributes);
    assert!(integral.largeop);
    // Integrals are large operators, but their limits do not move: only the
    // n-ary category carries `movablelimits`.
    assert!(!integral.movablelimits);
}

#[test]
fn dictionary_unknown_operators_use_the_default_category() {
    let attributes = OperatorAttributes::default();
    let (_, unknown) = dictionary::resolve(None, OperatorForm::Infix, &attributes);
    assert!(close(
        unknown.lspace.resolve(font_size()),
        font_size().scale_by(0.2777777777777778)
    ));
    assert!(!unknown.stretchy);
    assert!(!unknown.largeop);

    // Multi-character content is not in the dictionary either.
    let (_, multi_character) = dictionary::resolve(None, OperatorForm::Prefix, &attributes);
    assert!(close(
        multi_character.rspace.resolve(font_size()),
        font_size().scale_by(0.2777777777777778)
    ));
}

#[test]
fn dictionary_force_default_infix_bar() {
    let attributes = OperatorAttributes::default();

    // Infix `|` has no category flags, unlike the fence forms of `|`.
    let (_, infix_bar) = dictionary::resolve(Some('|'), OperatorForm::Infix, &attributes);
    assert!(!infix_bar.fence);
    assert!(!infix_bar.stretchy);
    assert!(close(
        infix_bar.lspace.resolve(font_size()),
        font_size().scale_by(0.2777777777777778)
    ));

    let (_, prefix_bar) = dictionary::resolve(Some('|'), OperatorForm::Prefix, &attributes);
    assert!(prefix_bar.fence);
    assert!(prefix_bar.stretchy);
}

#[test]
fn dictionary_attributes_override_the_dictionary() {
    let attributes = OperatorAttributes {
        form: Some("prefix"),
        lspace: Some("1em"),
        rspace: Some("50%"),
        stretchy: Some("TRUE"),
        largeop: Some("true"),
        ..Default::default()
    };

    // The explicit `form` attribute wins over the markup-inferred form.
    let (form, properties) = dictionary::resolve(Some('+'), OperatorForm::Infix, &attributes);
    assert_eq!(form, OperatorForm::Prefix);
    assert_eq!(
        properties.lspace.resolve(font_size()),
        font_size().scale_by(1.0)
    );
    // A percentage resolves against the dictionary value (0 for prefix `+`).
    assert_eq!(properties.rspace.resolve(font_size()), Au::zero());
    // Boolean attributes are ASCII case-insensitive.
    assert!(properties.stretchy);
    assert!(properties.largeop);

    let attributes = OperatorAttributes {
        form: Some("not-a-form"),
        ..Default::default()
    };
    let (form, _) = dictionary::resolve(Some('+'), OperatorForm::Postfix, &attributes);
    assert_eq!(form, OperatorForm::Postfix);
}

#[test]
fn dictionary_percentage_of_a_non_zero_dictionary_value() {
    let attributes = OperatorAttributes {
        lspace: Some("50%"),
        ..Default::default()
    };
    let (_, properties) = dictionary::resolve(Some('+'), OperatorForm::Infix, &attributes);
    // 50% of the `+` infix dictionary value of 4/18em.
    assert!(close(
        properties.lspace.resolve(font_size()),
        font_size().scale_by(2.0 / 18.0)
    ));
}

#[test]
fn default_operator_form_follows_the_position_among_siblings() {
    // First of several children of a grouping element: prefix.
    assert_eq!(
        dictionary::default_form(true, false, false),
        OperatorForm::Prefix
    );
    // Last of several children: postfix.
    assert_eq!(
        dictionary::default_form(false, true, false),
        OperatorForm::Postfix
    );
    // A script that is not the base: postfix.
    assert_eq!(
        dictionary::default_form(false, false, true),
        OperatorForm::Postfix
    );
    // Otherwise infix.
    assert_eq!(
        dictionary::default_form(false, false, false),
        OperatorForm::Infix
    );
}

#[test]
fn math_style_compact_detection() {
    assert!(dictionary::math_style_is_compact(MathStyle::Compact));
    assert!(!dictionary::math_style_is_compact(MathStyle::Normal));
}

#[test]
fn parse_math_length_attributes() {
    assert_eq!(
        parse_length("10px").map(|l| l.resolve(font_size())),
        Some(Au::from_px(10))
    );
    assert_eq!(
        parse_length("1.5em").map(|l| l.resolve(font_size())),
        Some(font_size().scale_by(1.5))
    );
    assert_eq!(
        parse_length("0").map(|l| l.resolve(font_size())),
        Some(Au::zero())
    );
    // Percentages need a containing block and are treated as absent.
    assert!(parse_length("50%").is_none());
    assert!(parse_length("not-a-length").is_none());
    assert!(matches!(parse_length("2em"), Some(MathLength::Em(2.0))));
}

#[test]
fn scripted_element_kinds() {
    assert!(MathElementKind::Sub.is_scripted());
    assert!(MathElementKind::MultiScripts.is_scripted());
    assert!(!MathElementKind::Fraction.is_scripted());
    assert!(!MathElementKind::Table.is_scripted());

    assert!(MathElementKind::Row.is_grouping_element());
    assert!(!MathElementKind::Fraction.is_grouping_element());

    assert!(MathElementKind::Token.is_token());
    assert!(!MathElementKind::Space.is_token());
}

#[test]
fn properties_without_spacing_keep_the_default_category() {
    // `OperatorProperties::DEFAULT` is the specification's fallback category.
    assert!(close(
        OperatorProperties::DEFAULT.lspace.resolve(font_size()),
        font_size().scale_by(0.2777777777777778)
    ));
    assert!(!OperatorProperties::DEFAULT.stretchy);
    assert!(!OperatorProperties::DEFAULT.separator);
}
