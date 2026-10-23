/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO fork self-build (REQ-BRW-051 C2): property-value reification.
//!
//! Turns CSS text into the typed object graph:
//!
//! * `CSSStyleValue.parse` / `parseAll`
//!   (<https://drafts.css-houdini.org/css-typed-om-1/#dom-cssstylevalue-parse>),
//! * the specified/computed `StylePropertyMap` read path,
//! * `<transform-list>` reification into `CSSTransformValue`.
//!
//! Property names are validated with the engine's own declaration parser, and
//! the *canonical specified serialization* is used as the reification source,
//! which is what gives property-aware normalisation for free (`width: 0`
//! becomes `0px`, list-valued properties keep their list shape, shorthands are
//! kept as base `CSSStyleValue`s).

use std::ffi::CString;

use cssparser::Parser as CssParser;
use euclid::default::Transform3D;
use js::context::JSContext;
use style::context::QuirksMode;
use style::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
    parse_one_declaration_into,
};
use style::stylesheets::{CssRuleType, Origin};
use style_traits::ParsingMode;

use super::csskeywordvalue::CSSKeywordValue;
use super::cssmatrixcomponent::CSSMatrixComponent;
use super::cssnumericvalue::CSSNumericValue;
use super::cssperspective::CSSPerspective;
use super::cssrotate::CSSRotate;
use super::cssscale::CSSScale;
use super::cssskew::CSSSkew;
use super::cssskewx::CSSSkewX;
use super::cssskewy::CSSSkewY;
use super::cssstylevalue::CSSStyleValue;
use super::csstransformcomponent::CSSTransformComponent;
use super::csstransformvalue::CSSTransformValue;
use super::csstranslate::CSSTranslate;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_numeric::{parse_css_numeric, parsed_numeric_type};
use super::typed_om_tokens::{
    build_unparsed_from_text, build_unparsed_value, segments_have_variable_reference,
    tokenize_unparsed,
};
use super::typed_om_values::{build_parsed_numeric, numeric_type_of};
use crate::css::css::ANONYMOUS_CONTENT_URL_DATA;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::dommatrixreadonly::DOMMatrixReadOnly;
use crate::dom::globalscope::GlobalScope;

fn type_error() -> Error {
    Error::Type(CString::new("Invalid property or value").unwrap())
}

/// CSS-wide keywords are always reified as `CSSKeywordValue`s.
const CSS_WIDE_KEYWORDS: &[&str] = &["initial", "inherit", "unset", "revert", "revert-layer"];

/// The CSS named colors (`<named-color>`), used to distinguish a keyword from
/// an unsupported `<color>` value: Typed OM level 1 does not reify colors, so
/// `color: red` must yield a base `CSSStyleValue` while `color: currentcolor`
/// yields a `CSSKeywordValue`.
const NAMED_COLORS: &[&str] = &[
    "aliceblue", "antiquewhite", "aqua", "aquamarine", "azure", "beige", "bisque", "black",
    "blanchedalmond", "blue", "blueviolet", "brown", "burlywood", "cadetblue", "chartreuse",
    "chocolate", "coral", "cornflowerblue", "cornsilk", "crimson", "cyan", "darkblue",
    "darkcyan", "darkgoldenrod", "darkgray", "darkgreen", "darkgrey", "darkkhaki",
    "darkmagenta", "darkolivegreen", "darkorange", "darkorchid", "darkred", "darksalmon",
    "darkseagreen", "darkslateblue", "darkslategray", "darkslategrey", "darkturquoise",
    "darkviolet", "deeppink", "deepskyblue", "dimgray", "dimgrey", "dodgerblue", "firebrick",
    "floralwhite", "forestgreen", "fuchsia", "gainsboro", "ghostwhite", "gold", "goldenrod",
    "gray", "green", "greenyellow", "grey", "honeydew", "hotpink", "indianred", "indigo",
    "ivory", "khaki", "lavender", "lavenderblush", "lawngreen", "lemonchiffon", "lightblue",
    "lightcoral", "lightcyan", "lightgoldenrodyellow", "lightgray", "lightgreen", "lightgrey",
    "lightpink", "lightsalmon", "lightseagreen", "lightskyblue", "lightslategray",
    "lightslategrey", "lightsteelblue", "lightyellow", "lime", "limegreen", "linen", "magenta",
    "maroon", "mediumaquamarine", "mediumblue", "mediumorchid", "mediumpurple",
    "mediumseagreen", "mediumslateblue", "mediumspringgreen", "mediumturquoise",
    "mediumvioletred", "midnightblue", "mintcream", "mistyrose", "moccasin", "navajowhite",
    "navy", "oldlace", "olive", "olivedrab", "orange", "orangered", "orchid", "palegoldenrod",
    "palegreen", "paleturquoise", "palevioletred", "papayawhip", "peachpuff", "peru", "pink",
    "plum", "powderblue", "purple", "rebeccapurple", "red", "rosybrown", "royalblue",
    "saddlebrown", "salmon", "sandybrown", "seagreen", "seashell", "sienna", "silver",
    "skyblue", "slateblue", "slategray", "slategrey", "snow", "springgreen", "steelblue", "tan",
    "teal", "thistle", "tomato", "turquoise", "violet", "wheat", "white", "whitesmoke",
    "yellow", "yellowgreen", "transparent",
];

/// Properties whose values may be `<color>`s (for the keyword/unsupported
/// distinction above).
const COLOR_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "border-block-start-color",
    "border-block-end-color",
    "border-inline-start-color",
    "border-inline-end-color",
    "outline-color",
    "text-decoration-color",
    "text-emphasis-color",
    "caret-color",
    "column-rule-color",
    "accent-color",
    "fill",
    "stroke",
    "stop-color",
    "flood-color",
    "lighting-color",
];

fn is_color_property(property: &str) -> bool {
    COLOR_PROPERTIES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(property))
}

fn is_unsupported_color_keyword(property: &str, ident: &str) -> bool {
    if !is_color_property(property) {
        return false;
    }
    if ident.eq_ignore_ascii_case("currentcolor") {
        return false;
    }
    let lowercase = ident.to_ascii_lowercase();
    NAMED_COLORS.iter().any(|name| *name == lowercase)
}

fn is_css_wide_keyword(ident: &str) -> bool {
    CSS_WIDE_KEYWORDS
        .iter()
        .any(|keyword| keyword.eq_ignore_ascii_case(ident))
}

/// Validates `property: value` with the engine parser and returns the property
/// id together with the canonical specified serialization of the value.
fn validate_and_serialize(property: &str, value: &str) -> Result<(PropertyId, String), ()> {
    if property.is_empty() {
        return Err(());
    }
    let id = PropertyId::parse_enabled_for_all_content(property).map_err(|_| ())?;
    match id {
        PropertyId::Custom(_) => {
            if value.is_empty() {
                return Err(());
            }
            Ok((id, value.to_string()))
        },
        PropertyId::NonCustom(_) => {
            let mut declarations = SourcePropertyDeclaration::default();
            parse_one_declaration_into(
                &mut declarations,
                id.clone(),
                value,
                Origin::Author,
                &ANONYMOUS_CONTENT_URL_DATA,
                None,
                ParsingMode::DEFAULT,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            )
            .map_err(|_| ())?;
            let mut block = PropertyDeclarationBlock::new();
            block.extend(declarations.drain(), Importance::Normal);
            let mut serialized = String::new();
            block.property_value_to_css(&id, &mut serialized).map_err(|_| ())?;
            if serialized.is_empty() {
                return Err(());
            }
            Ok((id, serialized))
        },
    }
}

/// Checks that `property: value` is a valid declaration (used by the set/append
/// path of `StylePropertyMap`, where invalid input is a `TypeError`).
pub(crate) fn validate_property_value(property: &str, value: &str) -> Result<(), ()> {
    validate_and_serialize(property, value).map(|_| ())
}

fn single_identifier(text: &str) -> Option<String> {
    let mut parser = CssParser::new(text);
    let ident = parser.expect_ident().ok()?.to_string();
    parser.expect_exhausted().ok()?;
    Some(ident)
}

/// Splits CSS text on top-level commas (paren/bracket/brace depth aware).
pub(crate) fn split_top_level_commas(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in text.chars() {
        if let Some(active_quote) = quote {
            current.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => {
                quote = Some(character);
                current.push(character);
            },
            '(' | '[' | '{' => {
                depth += 1;
                current.push(character);
            },
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(character);
            },
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current = String::new();
            },
            _ => current.push(character),
        }
    }
    result.push(current.trim().to_string());
    result
}

fn base_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
    property: &str,
    associated: bool,
) -> DomRoot<CSSStyleValue> {
    if associated {
        CSSStyleValue::new_with_property(cx, global, text.to_string(), property)
    } else {
        CSSStyleValue::new(cx, global, text.to_string())
    }
}

fn unparsed_root(cx: &mut JSContext, global: &GlobalScope, text: &str) -> DomRoot<CSSStyleValue> {
    let unparsed = build_unparsed_from_text(cx, global, text);
    DomRoot::from_ref(unparsed.upcast::<CSSStyleValue>())
}

fn keyword_root(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: String,
) -> DomRoot<CSSStyleValue> {
    let keyword = CSSKeywordValue::new(cx, global, value);
    DomRoot::from_ref(keyword.upcast::<CSSStyleValue>())
}

fn numeric_root(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSStyleValue>> {
    let parsed = parse_css_numeric(text).ok()?;
    parsed_numeric_type(&parsed).ok()?;
    let numeric = build_parsed_numeric(cx, global, &parsed);
    Some(DomRoot::from_ref(numeric.upcast::<CSSStyleValue>()))
}

/// Reifies CSS text for `property`. `associated` records the property on base
/// values (the `parse()`/`get()` paths), while the `parseAll` elements share it.
fn reify(
    cx: &mut JSContext,
    global: &GlobalScope,
    id: &PropertyId,
    property: &str,
    text: &str,
    associated: bool,
) -> DomRoot<CSSStyleValue> {
    // Custom properties are always unparsed token streams.
    if matches!(id, PropertyId::Custom(_)) {
        return unparsed_root(cx, global, text);
    }

    // var()/env()-style values cannot be reified, even for known properties.
    if let Ok(segments) = tokenize_unparsed(text) {
        if segments_have_variable_reference(&segments) {
            let unparsed = build_unparsed_value(cx, global, &segments);
            return DomRoot::from_ref(unparsed.upcast::<CSSStyleValue>());
        }
    }

    // Keyword values (CSS-wide keywords included).
    if let Some(ident) = single_identifier(text) {
        if !is_unsupported_color_keyword(property, &ident) {
            return keyword_root(cx, global, ident);
        }
    }

    // Shorthands do not have a typed representation in level 1.
    if id.as_shorthand().is_ok() {
        return base_value(cx, global, text, property, associated);
    }

    if property.eq_ignore_ascii_case("transform") {
        if let Some(transform) = parse_transform_value(cx, global, text) {
            return DomRoot::from_ref(transform.upcast::<CSSStyleValue>());
        }
    }

    if let Some(numeric) = numeric_root(cx, global, text) {
        return numeric;
    }

    base_value(cx, global, text, property, associated)
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssstylevalue-parse>
pub(crate) fn parse_property_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    property: &str,
    css_text: &str,
) -> Fallible<DomRoot<CSSStyleValue>> {
    let (id, serialized) = validate_and_serialize(property, css_text).map_err(|_| type_error())?;
    Ok(reify(cx, global, &id, property, &serialized, true))
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssstylevalue-parseall>
pub(crate) fn parse_property_value_all(
    cx: &mut JSContext,
    global: &GlobalScope,
    property: &str,
    css_text: &str,
) -> Fallible<Vec<DomRoot<CSSStyleValue>>> {
    let (id, serialized) = validate_and_serialize(property, css_text).map_err(|_| type_error())?;
    if matches!(id, PropertyId::Custom(_)) {
        return Ok(vec![reify(cx, global, &id, property, &serialized, true)]);
    }
    if id.as_shorthand().is_ok() {
        return Ok(vec![reify(cx, global, &id, property, &serialized, true)]);
    }
    let mut result = Vec::new();
    for part in split_top_level_commas(&serialized) {
        result.push(reify(cx, global, &id, property, &part, true));
    }
    Ok(result)
}

/// Reifies engine-produced CSS text (specified or computed declaration value)
/// without re-validating the declaration.
pub(crate) fn reify_engine_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    property: &str,
    text: &str,
) -> DomRoot<CSSStyleValue> {
    match PropertyId::parse_enabled_for_all_content(property) {
        Ok(id) => reify(cx, global, &id, property, text, true),
        Err(()) => base_value(cx, global, text, property, true),
    }
}

// ---------------------------------------------------------------------------
// `<transform-list>` reification
// ---------------------------------------------------------------------------

fn take_block_text(input: &mut CssParser) -> String {
    let start = input.position();
    loop {
        if input.next_including_whitespace_and_comments().is_err() {
            break;
        }
    }
    input.slice_from(start).to_string()
}

fn parse_numeric_argument(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSNumericValue>> {
    let parsed = parse_css_numeric(text).ok()?;
    parsed_numeric_type(&parsed).ok()?;
    Some(build_parsed_numeric(cx, global, &parsed))
}

fn number_argument(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSNumericValue>> {
    let numeric = parse_numeric_argument(cx, global, text)?;
    if !numeric_type_of(&numeric).ok()?.matches_number() {
        return None;
    }
    Some(numeric)
}

fn length_argument(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSNumericValue>> {
    let numeric = parse_numeric_argument(cx, global, text)?;
    if !numeric_type_of(&numeric).ok()?.matches_length() {
        return None;
    }
    Some(numeric)
}

fn length_percentage_argument(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSNumericValue>> {
    let numeric = parse_numeric_argument(cx, global, text)?;
    if !numeric_type_of(&numeric).ok()?.matches_length_percentage() {
        return None;
    }
    Some(numeric)
}

fn angle_argument(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSNumericValue>> {
    let numeric = parse_numeric_argument(cx, global, text)?;
    if !numeric_type_of(&numeric).ok()?.matches_angle() {
        return None;
    }
    Some(numeric)
}

fn zero_number(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, 0.0, "number");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

fn one_number(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, 1.0, "number");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

fn zero_length(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, 0.0, "px");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

fn zero_angle(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, 0.0, "deg");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

fn numeric_as_number(value: &CSSNumericValue) -> Option<f64> {
    let unit_value = value.downcast::<CSSUnitValue>()?;
    if !unit_value.unit_str().eq_ignore_ascii_case("number") {
        return None;
    }
    Some(unit_value.value())
}

fn matrix_from_numbers(
    cx: &mut JSContext,
    global: &GlobalScope,
    numbers: &[f64],
) -> Option<DomRoot<DOMMatrixReadOnly>> {
    if numbers.len() == 6 {
        let matrix = Transform3D::new(
            numbers[0],
            numbers[1],
            0.0,
            0.0,
            numbers[2],
            numbers[3],
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            numbers[4],
            numbers[5],
            0.0,
            1.0,
        );
        return Some(DOMMatrixReadOnly::new(cx, global, true, matrix));
    }
    if numbers.len() == 16 {
        let matrix = Transform3D::new(
            numbers[0],
            numbers[1],
            numbers[2],
            numbers[3],
            numbers[4],
            numbers[5],
            numbers[6],
            numbers[7],
            numbers[8],
            numbers[9],
            numbers[10],
            numbers[11],
            numbers[12],
            numbers[13],
            numbers[14],
            numbers[15],
        );
        return Some(DOMMatrixReadOnly::new(cx, global, false, matrix));
    }
    None
}

fn parse_transform_function(
    cx: &mut JSContext,
    global: &GlobalScope,
    name: &str,
    arguments: &[String],
) -> Option<DomRoot<CSSTransformComponent>> {
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "translate" => {
            let x = length_percentage_argument(cx, global, arguments.first()?)?;
            let y = match arguments.get(1) {
                Some(argument) => length_percentage_argument(cx, global, argument)?,
                None => zero_length(cx, global),
            };
            let z = match arguments.get(2) {
                Some(argument) => Some(length_argument(cx, global, argument)?),
                None => None,
            };
            let translate = CSSTranslate::new(cx, global, x, y, z);
            Some(DomRoot::from_ref(translate.upcast::<CSSTransformComponent>()))
        },
        "translatex" => {
            let x = length_percentage_argument(cx, global, arguments.first()?)?;
            let zero = zero_length(cx, global);
            let translate = CSSTranslate::new(cx, global, x, zero, None);
            Some(DomRoot::from_ref(translate.upcast::<CSSTransformComponent>()))
        },
        "translatey" => {
            let y = length_percentage_argument(cx, global, arguments.first()?)?;
            let zero = zero_length(cx, global);
            let translate = CSSTranslate::new(cx, global, zero, y, None);
            Some(DomRoot::from_ref(translate.upcast::<CSSTransformComponent>()))
        },
        "translatez" => {
            let z = length_argument(cx, global, arguments.first()?)?;
            let zero_x = zero_length(cx, global);
            let zero_y = zero_length(cx, global);
            let translate = CSSTranslate::new(cx, global, zero_x, zero_y, Some(z));
            Some(DomRoot::from_ref(translate.upcast::<CSSTransformComponent>()))
        },
        "translate3d" => {
            if arguments.len() != 3 {
                return None;
            }
            let x = length_percentage_argument(cx, global, &arguments[0])?;
            let y = length_percentage_argument(cx, global, &arguments[1])?;
            let z = length_argument(cx, global, &arguments[2])?;
            let translate = CSSTranslate::new(cx, global, x, y, Some(z));
            Some(DomRoot::from_ref(translate.upcast::<CSSTransformComponent>()))
        },
        "scale" => {
            let x = number_argument(cx, global, arguments.first()?)?;
            let y = match arguments.get(1) {
                Some(argument) => number_argument(cx, global, argument)?,
                None => x.clone(),
            };
            let z = match arguments.get(2) {
                Some(argument) => Some(number_argument(cx, global, argument)?),
                None => None,
            };
            let scale = CSSScale::new(cx, global, x, y, z);
            Some(DomRoot::from_ref(scale.upcast::<CSSTransformComponent>()))
        },
        "scalex" => {
            let x = number_argument(cx, global, arguments.first()?)?;
            let one = one_number(cx, global);
            let scale = CSSScale::new(cx, global, x, one, None);
            Some(DomRoot::from_ref(scale.upcast::<CSSTransformComponent>()))
        },
        "scaley" => {
            let y = number_argument(cx, global, arguments.first()?)?;
            let one = one_number(cx, global);
            let scale = CSSScale::new(cx, global, one, y, None);
            Some(DomRoot::from_ref(scale.upcast::<CSSTransformComponent>()))
        },
        "scalez" => {
            let z = number_argument(cx, global, arguments.first()?)?;
            let one_x = one_number(cx, global);
            let one_y = one_number(cx, global);
            let scale = CSSScale::new(cx, global, one_x, one_y, Some(z));
            Some(DomRoot::from_ref(scale.upcast::<CSSTransformComponent>()))
        },
        "scale3d" => {
            if arguments.len() != 3 {
                return None;
            }
            let x = number_argument(cx, global, &arguments[0])?;
            let y = number_argument(cx, global, &arguments[1])?;
            let z = number_argument(cx, global, &arguments[2])?;
            let scale = CSSScale::new(cx, global, x, y, Some(z));
            Some(DomRoot::from_ref(scale.upcast::<CSSTransformComponent>()))
        },
        "rotate" => {
            let angle = angle_argument(cx, global, arguments.first()?)?;
            let rotate = CSSRotate::new_from_angle(cx, global, angle);
            Some(DomRoot::from_ref(rotate.upcast::<CSSTransformComponent>()))
        },
        "rotatex" | "rotatey" | "rotatez" => {
            let angle = angle_argument(cx, global, arguments.first()?)?;
            let axis = match name.as_str() {
                "rotatex" => (
                    one_number(cx, global),
                    zero_number(cx, global),
                    zero_number(cx, global),
                ),
                "rotatey" => (
                    zero_number(cx, global),
                    one_number(cx, global),
                    zero_number(cx, global),
                ),
                _ => (
                    zero_number(cx, global),
                    zero_number(cx, global),
                    one_number(cx, global),
                ),
            };
            let rotate =
                CSSRotate::new_from_axis(cx, global, axis.0, axis.1, axis.2, angle);
            Some(DomRoot::from_ref(rotate.upcast::<CSSTransformComponent>()))
        },
        "rotate3d" => {
            if arguments.len() != 4 {
                return None;
            }
            let x = number_argument(cx, global, &arguments[0])?;
            let y = number_argument(cx, global, &arguments[1])?;
            let z = number_argument(cx, global, &arguments[2])?;
            let angle = angle_argument(cx, global, &arguments[3])?;
            let rotate = CSSRotate::new_from_axis(cx, global, x, y, z, angle);
            Some(DomRoot::from_ref(rotate.upcast::<CSSTransformComponent>()))
        },
        "skew" => {
            let ax = angle_argument(cx, global, arguments.first()?)?;
            let ay = match arguments.get(1) {
                Some(argument) => angle_argument(cx, global, argument)?,
                None => zero_angle(cx, global),
            };
            let skew = CSSSkew::new(cx, global, ax, ay);
            Some(DomRoot::from_ref(skew.upcast::<CSSTransformComponent>()))
        },
        "skewx" => {
            let ax = angle_argument(cx, global, arguments.first()?)?;
            let skew = CSSSkewX::new(cx, global, ax);
            Some(DomRoot::from_ref(skew.upcast::<CSSTransformComponent>()))
        },
        "skewy" => {
            let ay = angle_argument(cx, global, arguments.first()?)?;
            let skew = CSSSkewY::new(cx, global, ay);
            Some(DomRoot::from_ref(skew.upcast::<CSSTransformComponent>()))
        },
        "perspective" => {
            let length = length_argument(cx, global, arguments.first()?)?;
            let perspective = CSSPerspective::new(cx, global, length);
            Some(DomRoot::from_ref(perspective.upcast::<CSSTransformComponent>()))
        },
        "matrix" | "matrix3d" => {
            let mut numbers = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let numeric = number_argument(cx, global, argument)?;
                numbers.push(numeric_as_number(&numeric)?);
            }
            let matrix = matrix_from_numbers(cx, global, &numbers)?;
            let component = CSSMatrixComponent::new(cx, global, matrix, None);
            Some(DomRoot::from_ref(component.upcast::<CSSTransformComponent>()))
        },
        _ => None,
    }
}

/// Reifies a `<transform-list>` into a `CSSTransformValue`.
pub(crate) fn parse_transform_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Option<DomRoot<CSSTransformValue>> {
    let mut input = CssParser::new(text);
    let mut components: Vec<DomRoot<CSSTransformComponent>> = Vec::new();
    loop {
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        match token {
            cssparser::Token::Function(name) => {
                let name = name.to_string();
                let raw = input
                    .parse_nested_block(|nested| -> Result<String, cssparser::ParseError<()>> {
                        Ok(take_block_text(nested))
                    })
                    .ok()?;
                let arguments: Vec<String> = if raw.trim().is_empty() {
                    Vec::new()
                } else {
                    split_top_level_commas(&raw)
                };
                let component = parse_transform_function(cx, global, &name, &arguments)?;
                components.push(component);
            },
            _ => return None,
        }
    }
    if components.is_empty() {
        return None;
    }
    Some(CSSTransformValue::new(cx, global, components))
}
