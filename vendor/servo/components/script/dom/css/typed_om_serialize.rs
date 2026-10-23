/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM serialization.
//!
//! Implements the `stringifier` behaviour of every value class:
//!
//! * `CSSUnitValue` / `CSSKeywordValue` / `CSSUnparsedValue` /
//!   `CSSVariableReferenceValue` (<https://drafts.css-houdini.org/css-typed-om-1/#serialization>),
//! * `CSSMathValue` with the nested / paren-less argument rules and the
//!   canonical (absolute unit folding + unit sort) form when every summand or
//!   factor is a `CSSUnitValue`,
//! * transform components and `CSSTransformValue`,
//! * color classes.

use cssparser::serialize_identifier;

use super::csshsl::CSSHSL;
use super::csshwb::CSSHWB;
use super::csskeywordvalue::CSSKeywordValue;
use super::csslab::CSSLab;
use super::csslch::CSSLCH;
use super::cssmathclamp::CSSMathClamp;
use super::cssmathinvert::CSSMathInvert;
use super::cssmathmax::CSSMathMax;
use super::cssmathmin::CSSMathMin;
use super::cssmathnegate::CSSMathNegate;
use super::cssmathproduct::CSSMathProduct;
use super::cssmathsum::CSSMathSum;
use super::cssmathvalue::CSSMathValue;
use super::cssmatrixcomponent::CSSMatrixComponent;
use super::cssnumericvalue::CSSNumericValue;
use super::cssoklab::CSSOKLab;
use super::cssoklch::CSSOKLCH;
use super::cssperspective::CSSPerspective;
use super::cssrgb::CSSRGB;
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
use super::cssunparsedvalue::{CSSUnparsedSegment, CSSUnparsedValue};
use super::cssvariablereferencevalue::CSSVariableReferenceValue;
use super::typed_om_numeric::{canonicalize_unit, convert_unit, serialize_number};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::dommatrixreadonly::DOMMatrixReadOnly;

fn escape_identifier(value: &str) -> String {
    let mut result = String::new();
    serialize_identifier(value, &mut result).unwrap();
    result
}

pub(crate) fn serialize_keyword_value(value: &CSSKeywordValue) -> String {
    escape_identifier(&value.value())
}

fn serialize_unit_value(value: &CSSUnitValue) -> String {
    let unit = value.unit_str().to_ascii_lowercase();
    let number = serialize_number(value.value());
    if unit.eq_ignore_ascii_case("number") {
        number
    } else if unit.eq_ignore_ascii_case("percent") {
        format!("{number}%")
    } else {
        format!("{number}{unit}")
    }
}

fn serialize_keyword(value: &CSSKeywordValue) -> String {
    serialize_keyword_value(value)
}

fn open_math(nested: bool, paren_less: bool) -> String {
    if paren_less {
        String::new()
    } else if nested {
        "(".to_string()
    } else {
        "calc(".to_string()
    }
}

fn close_math(_nested: bool, paren_less: bool) -> &'static str {
    if paren_less { "" } else { ")" }
}

/// When every term is a `CSSUnitValue`, canonicalises absolute units and
/// sorts by unit name (`number` first), which is what the CSS-values math
/// serialization algorithm mandates and what the WPT serialization tests pin
/// (`calc(1em + 1px)`, `calc(2 * 90deg)`).
fn canonical_unit_terms(values: &[DomRoot<CSSNumericValue>]) -> Option<Vec<String>> {
    let mut terms: Vec<(f64, String)> = Vec::with_capacity(values.len());
    for value in values {
        let unit_value = value.downcast::<CSSUnitValue>()?;
        let unit = unit_value.unit_str().to_ascii_lowercase();
        let (numeric, unit) =
            canonicalize_unit(unit_value.value(), &unit).unwrap_or((unit_value.value(), unit));
        terms.push((numeric, unit));
    }
    terms.sort_by(|left, right| {
        let left_key = (if left.1 == "number" { 0 } else { 1 }, left.1.clone());
        let right_key = (if right.1 == "number" { 0 } else { 1 }, right.1.clone());
        left_key.cmp(&right_key)
    });
    Some(
        terms
            .into_iter()
            .map(|(value, unit)| {
                if unit == "number" {
                    serialize_number(value)
                } else if unit == "percent" {
                    format!("{}%", serialize_number(value))
                } else {
                    format!("{}{}", serialize_number(value), unit)
                }
            })
            .collect(),
    )
}

fn serialize_sum(sum: &CSSMathSum, nested: bool, paren_less: bool) -> String {
    let values = sum.values();
    let mut result = open_math(nested, paren_less);
    if let Some(terms) = canonical_unit_terms(&values) {
        result.push_str(&terms.join(" + "));
    } else {
        for (index, value) in values.iter().enumerate() {
            if index == 0 {
                result.push_str(&serialize_numeric(value, true, false));
                continue;
            }
            if let Some(negate) = value.downcast::<CSSMathNegate>() {
                result.push_str(" - ");
                result.push_str(&serialize_numeric(&negate.value_root(), true, false));
            } else {
                result.push_str(" + ");
                result.push_str(&serialize_numeric(value, true, false));
            }
        }
    }
    result.push_str(close_math(nested, paren_less));
    result
}

fn serialize_product(product: &CSSMathProduct, nested: bool, paren_less: bool) -> String {
    let values = product.values();
    let mut result = open_math(nested, paren_less);
    if let Some(terms) = canonical_unit_terms(&values) {
        result.push_str(&terms.join(" * "));
    } else {
        for (index, value) in values.iter().enumerate() {
            if index == 0 {
                result.push_str(&serialize_numeric(value, true, false));
                continue;
            }
            if let Some(invert) = value.downcast::<CSSMathInvert>() {
                result.push_str(" / ");
                result.push_str(&serialize_numeric(&invert.value_root(), true, false));
            } else {
                result.push_str(" * ");
                result.push_str(&serialize_numeric(value, true, false));
            }
        }
    }
    result.push_str(close_math(nested, paren_less));
    result
}

fn serialize_negate(negate: &CSSMathNegate, nested: bool, paren_less: bool) -> String {
    let mut result = open_math(nested, paren_less);
    result.push('-');
    result.push_str(&serialize_numeric(&negate.value_root(), true, false));
    result.push_str(close_math(nested, paren_less));
    result
}

fn serialize_invert(invert: &CSSMathInvert, nested: bool, paren_less: bool) -> String {
    let mut result = open_math(nested, paren_less);
    result.push_str("1 / ");
    result.push_str(&serialize_numeric(&invert.value_root(), true, false));
    result.push_str(close_math(nested, paren_less));
    result
}

fn serialize_min_max(values: &[DomRoot<CSSNumericValue>], is_min: bool) -> String {
    let mut result = String::from(if is_min { "min(" } else { "max(" });
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            result.push_str(", ");
        }
        result.push_str(&serialize_numeric(value, true, true));
    }
    result.push(')');
    result
}

fn serialize_clamp(clamp: &CSSMathClamp) -> String {
    format!(
        "clamp({}, {}, {})",
        serialize_numeric(&clamp.lower_root(), true, true),
        serialize_numeric(&clamp.value_root(), true, true),
        serialize_numeric(&clamp.upper_root(), true, true),
    )
}

/// Serializes any `CSSNumericValue` with the calc nesting flags.
pub(crate) fn serialize_numeric(value: &CSSNumericValue, nested: bool, paren_less: bool) -> String {
    if let Some(unit_value) = value.downcast::<CSSUnitValue>() {
        return serialize_unit_value(unit_value);
    }
    if let Some(sum) = value.downcast::<CSSMathSum>() {
        return serialize_sum(sum, nested, paren_less);
    }
    if let Some(product) = value.downcast::<CSSMathProduct>() {
        return serialize_product(product, nested, paren_less);
    }
    if let Some(negate) = value.downcast::<CSSMathNegate>() {
        return serialize_negate(negate, nested, paren_less);
    }
    if let Some(invert) = value.downcast::<CSSMathInvert>() {
        return serialize_invert(invert, nested, paren_less);
    }
    if let Some(min) = value.downcast::<CSSMathMin>() {
        return serialize_min_max(&min.values(), true);
    }
    if let Some(max) = value.downcast::<CSSMathMax>() {
        return serialize_min_max(&max.values(), false);
    }
    if let Some(clamp) = value.downcast::<CSSMathClamp>() {
        return serialize_clamp(clamp);
    }
    String::new()
}

fn serialize_math_value(value: &CSSMathValue) -> String {
    serialize_numeric(value.upcast::<CSSNumericValue>(), false, false)
}

fn serialize_unparsed(value: &CSSUnparsedValue) -> String {
    value.with_segments(|segments| {
        let mut result = String::new();
        for segment in segments {
            let chunk = match segment {
                CSSUnparsedSegment::Text(text) => text.clone(),
                CSSUnparsedSegment::VariableReference(reference) => {
                    serialize_variable_reference(reference)
                },
            };
            if super::typed_om_tokens::needs_separator(&result, &chunk) {
                result.push_str("/**/");
            }
            result.push_str(&chunk);
        }
        result
    })
}

fn serialize_variable_reference(value: &CSSVariableReferenceValue) -> String {
    let mut result = String::from("var(");
    result.push_str(&value.variable());
    if let Some(fallback) = value.fallback_root() {
        result.push(',');
        result.push_str(&serialize_unparsed(&fallback));
    }
    result.push(')');
    result
}

fn serialize_transform_value(value: &CSSTransformValue) -> String {
    let components = value.components();
    let mut result = String::new();
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            result.push(' ');
        }
        result.push_str(&serialize_transform_component(component));
    }
    result
}

pub(super) fn serialize_transform_component(value: &CSSTransformComponent) -> String {
    if let Some(translate) = value.downcast::<CSSTranslate>() {
        let x = serialize_numeric(&translate.x_root(), false, false);
        let y = serialize_numeric(&translate.y_root(), false, false);
        if value.is_2d() {
            return format!("translate({x}, {y})");
        }
        let z = serialize_numeric(&translate.z_root(), false, false);
        return format!("translate3d({x}, {y}, {z})");
    }
    if let Some(rotate) = value.downcast::<CSSRotate>() {
        let angle = serialize_numeric(&rotate.angle_root(), false, false);
        if value.is_2d() {
            return format!("rotate({angle})");
        }
        let x = serialize_numeric(&rotate.x_root(), false, false);
        let y = serialize_numeric(&rotate.y_root(), false, false);
        let z = serialize_numeric(&rotate.z_root(), false, false);
        return format!("rotate3d({x}, {y}, {z}, {angle})");
    }
    if let Some(scale) = value.downcast::<CSSScale>() {
        let x = serialize_numeric(&scale.x_root(), false, false);
        let y = serialize_numeric(&scale.y_root(), false, false);
        if !value.is_2d() {
            let z = serialize_numeric(&scale.z_root(), false, false);
            return format!("scale3d({x}, {y}, {z})");
        }
        if let (Some(x_value), Some(y_value)) = (
            scale.x_root().downcast::<CSSUnitValue>(),
            scale.y_root().downcast::<CSSUnitValue>(),
        ) {
            if x_value.unit_str() == y_value.unit_str() && x_value.value() == y_value.value() {
                return format!("scale({x})");
            }
        }
        return format!("scale({x}, {y})");
    }
    if let Some(skew) = value.downcast::<CSSSkew>() {
        let ax = serialize_numeric(&skew.ax_root(), false, false);
        if let Some(ay_value) = skew.ay_root().downcast::<CSSUnitValue>() {
            if ay_value.value() == 0.0 {
                return format!("skew({ax})");
            }
        }
        let ay = serialize_numeric(&skew.ay_root(), false, false);
        return format!("skew({ax}, {ay})");
    }
    if let Some(skew_x) = value.downcast::<CSSSkewX>() {
        return format!(
            "skewX({})",
            serialize_numeric(&skew_x.ax_root(), false, false)
        );
    }
    if let Some(skew_y) = value.downcast::<CSSSkewY>() {
        return format!(
            "skewY({})",
            serialize_numeric(&skew_y.ay_root(), false, false)
        );
    }
    if let Some(perspective) = value.downcast::<CSSPerspective>() {
        let numeric = perspective.length_as_numeric();
        let mut body = match &numeric {
            Some(numeric) => serialize_numeric(numeric, false, false),
            None => match perspective.length_as_keyword() {
                Some(keyword) => escape_identifier(&keyword.value()),
                None => String::new(),
            },
        };
        if let Some(unit_value) = numeric
            .as_ref()
            .and_then(|numeric| numeric.downcast::<CSSUnitValue>())
        {
            if unit_value.value() < 0.0 {
                body = format!("calc({body})");
            }
        }
        return format!("perspective({body})");
    }
    if let Some(matrix_component) = value.downcast::<CSSMatrixComponent>() {
        return serialize_matrix(&matrix_component.matrix_root());
    }
    String::new()
}

fn serialize_matrix(matrix: &DomRoot<crate::dom::dommatrix::DOMMatrix>) -> String {
    let readonly = matrix.upcast::<DOMMatrixReadOnly>();
    let matrix = readonly.matrix();
    if readonly.is2D() {
        format!(
            "matrix({}, {}, {}, {}, {}, {})",
            serialize_number(matrix.m11),
            serialize_number(matrix.m12),
            serialize_number(matrix.m21),
            serialize_number(matrix.m22),
            serialize_number(matrix.m41),
            serialize_number(matrix.m42),
        )
    } else {
        format!(
            "matrix3d({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            serialize_number(matrix.m11),
            serialize_number(matrix.m12),
            serialize_number(matrix.m13),
            serialize_number(matrix.m14),
            serialize_number(matrix.m21),
            serialize_number(matrix.m22),
            serialize_number(matrix.m23),
            serialize_number(matrix.m24),
            serialize_number(matrix.m31),
            serialize_number(matrix.m32),
            serialize_number(matrix.m33),
            serialize_number(matrix.m34),
            serialize_number(matrix.m41),
            serialize_number(matrix.m42),
            serialize_number(matrix.m43),
            serialize_number(matrix.m44),
        )
    }
}

fn serialize_color(value: &CSSStyleValue) -> Option<String> {
    if let Some(rgb) = value.downcast::<CSSRGB>() {
        return Some(format!(
            "rgb({}, {}, {})",
            rgb.r_serialized(),
            rgb.g_serialized(),
            rgb.b_serialized(),
        ));
    }
    if let Some(hsl) = value.downcast::<CSSHSL>() {
        return Some(format!(
            "hsl({}, {}, {})",
            hsl.h_serialized(),
            hsl.s_serialized(),
            hsl.l_serialized(),
        ));
    }
    if let Some(hwb) = value.downcast::<CSSHWB>() {
        return Some(format!(
            "hwb({}, {}, {})",
            hwb.h_serialized(),
            hwb.w_serialized(),
            hwb.b_serialized(),
        ));
    }
    if let Some(lab) = value.downcast::<CSSLab>() {
        return Some(format!(
            "lab({}, {}, {})",
            lab.l_serialized(),
            lab.a_serialized(),
            lab.b_serialized(),
        ));
    }
    if let Some(lch) = value.downcast::<CSSLCH>() {
        return Some(format!(
            "lch({}, {}, {})",
            lch.l_serialized(),
            lch.c_serialized(),
            lch.h_serialized(),
        ));
    }
    if let Some(oklab) = value.downcast::<CSSOKLab>() {
        return Some(format!(
            "oklab({}, {}, {})",
            oklab.l_serialized(),
            oklab.a_serialized(),
            oklab.b_serialized(),
        ));
    }
    if let Some(oklch) = value.downcast::<CSSOKLCH>() {
        return Some(format!(
            "oklch({}, {}, {})",
            oklch.l_serialized(),
            oklch.c_serialized(),
            oklch.h_serialized(),
        ));
    }
    None
}

/// `CSSStyleValue::stringifier` implementation for every subclass.
pub(crate) fn serialize_style_value(value: &CSSStyleValue) -> String {
    if let Some(keyword) = value.downcast::<CSSKeywordValue>() {
        return serialize_keyword(keyword);
    }
    if let Some(unit_value) = value.downcast::<CSSUnitValue>() {
        return serialize_unit_value(unit_value);
    }
    if let Some(math_value) = value.downcast::<CSSMathValue>() {
        return serialize_math_value(math_value);
    }
    if let Some(unparsed) = value.downcast::<CSSUnparsedValue>() {
        return serialize_unparsed(unparsed);
    }

    if let Some(transform_value) = value.downcast::<CSSTransformValue>() {
        return serialize_transform_value(transform_value);
    }

    if let Some(color) = serialize_color(value) {
        return color;
    }
    value.serialized_text().to_string()
}

/// Helper used by `toMatrix()` implementations: converts a numeric value to a
/// single unit value in the requested canonical family.
pub(crate) fn numeric_to_absolute(value: &CSSNumericValue, unit: &str) -> Result<f64, ()> {
    let parsed = super::typed_om_values::numeric_to_parsed(value)?;
    let sum = super::typed_om_numeric::sum_value(&parsed)?;
    if sum.len() != 1 {
        return Err(());
    }
    let (item_value, item_unit) = super::typed_om_numeric::sum_value_item_to_unit(&sum[0])?;
    convert_unit(item_value, &item_unit, unit).ok_or(())
}
