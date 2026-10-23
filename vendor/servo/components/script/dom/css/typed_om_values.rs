/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO fork self-build (REQ-BRW-051 C2): `CSSNumericValue` DOM bridge.
//!
//! Converts between the `CSSNumericValue` object tree and the pure-data
//! `ParsedNumeric` representation in [`super::typed_om_numeric`], and
//! implements the specification's arithmetic / conversion algorithms on top of
//! the DOM objects:
//!
//! * type computation and validation for `add`/`sub`/`mul`/`div`/`min`/`max`,
//! * "create a CSSMathSum/Product/Min/Max" simplification,
//! * `to()`, `toSum()`, `type()`, `equals()`, and the `CSSNumericValue.parse`
//!   builder.

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSNumericType;
use js::context::JSContext;

use super::cssmathclamp::CSSMathClamp;
use super::cssmathinvert::CSSMathInvert;
use super::cssmathmax::CSSMathMax;
use super::cssmathmin::CSSMathMin;
use super::cssmathnegate::CSSMathNegate;
use super::cssmathproduct::CSSMathProduct;
use super::cssmathsum::CSSMathSum;
use super::cssnumericvalue::CSSNumericValue;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_numeric::{
    NumericType, ParsedNumeric, parse_css_numeric, parsed_numeric_type, simplify, sum_value,
    sum_value_item_to_unit,
};
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).expect("error message must not contain NUL"))
}

fn syntax_error(message: &str) -> Error {
    Error::Syntax(Some(message.to_string()))
}

/// Converts a numeric DOM object into the pure-data parsed representation.
pub(crate) fn numeric_to_parsed(value: &CSSNumericValue) -> Result<ParsedNumeric, ()> {
    if let Some(unit_value) = value.downcast::<CSSUnitValue>() {
        return Ok(ParsedNumeric::Unit {
            value: unit_value.value(),
            unit: unit_value.unit_str().to_string(),
        });
    }
    if let Some(math_sum) = value.downcast::<CSSMathSum>() {
        let mut values = Vec::new();
        for child in math_sum.values() {
            values.push(numeric_to_parsed(&child)?);
        }
        return Ok(ParsedNumeric::Sum(values));
    }
    if let Some(math_product) = value.downcast::<CSSMathProduct>() {
        let mut values = Vec::new();
        for child in math_product.values() {
            values.push(numeric_to_parsed(&child)?);
        }
        return Ok(ParsedNumeric::Product(values));
    }
    if let Some(math_negate) = value.downcast::<CSSMathNegate>() {
        return Ok(ParsedNumeric::Negate(Box::new(numeric_to_parsed(
            &math_negate.value_root(),
        )?)));
    }
    if let Some(math_invert) = value.downcast::<CSSMathInvert>() {
        return Ok(ParsedNumeric::Invert(Box::new(numeric_to_parsed(
            &math_invert.value_root(),
        )?)));
    }
    if let Some(math_min) = value.downcast::<CSSMathMin>() {
        let mut values = Vec::new();
        for child in math_min.values() {
            values.push(numeric_to_parsed(&child)?);
        }
        return Ok(ParsedNumeric::Min(values));
    }
    if let Some(math_max) = value.downcast::<CSSMathMax>() {
        let mut values = Vec::new();
        for child in math_max.values() {
            values.push(numeric_to_parsed(&child)?);
        }
        return Ok(ParsedNumeric::Max(values));
    }
    if let Some(math_clamp) = value.downcast::<CSSMathClamp>() {
        return Ok(ParsedNumeric::Clamp {
            lower: Box::new(numeric_to_parsed(&math_clamp.lower_root())?),
            value: Box::new(numeric_to_parsed(&math_clamp.value_root())?),
            upper: Box::new(numeric_to_parsed(&math_clamp.upper_root())?),
        });
    }
    Err(())
}

/// Computes the numeric type of a DOM numeric value.
pub(crate) fn numeric_type_of(value: &CSSNumericValue) -> Result<NumericType, ()> {
    let parsed = numeric_to_parsed(value)?;
    parsed_numeric_type(&parsed)
}

/// The `CSSNumericType` dictionary returned by `CSSNumericValue.type()`.
pub(crate) fn numeric_type_dictionary(value: &CSSNumericValue) -> Fallible<CSSNumericType> {
    let numeric_type = numeric_type_of(value).map_err(|_| type_error("Invalid CSSNumericValue"))?;
    Ok(numeric_type.to_dictionary())
}

/// Rectifies a numberish value and returns a rooted `CSSNumericValue`.
pub(crate) fn rectify_numberish_rooted(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DoubleOrCSSNumericValue,
) -> DomRoot<CSSNumericValue> {
    match value {
        DoubleOrCSSNumericValue::Double(value) => {
            let unit_value = CSSUnitValue::new(cx, global, *value, "number");
            DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
        },
        DoubleOrCSSNumericValue::CSSNumericValue(value) => value,
    }
}

/// The arithmetic operations exposed by `CSSNumericValue`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArithmeticOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

fn create_sum(
    cx: &mut JSContext,
    global: &GlobalScope,
    values: Vec<DomRoot<CSSNumericValue>>,
) -> DomRoot<CSSNumericValue> {
    // "create a CSSMathSum": if all values are CSSUnitValue with the same unit,
    // return a CSSUnitValue with the sum.
    let mut all_same_unit = true;
    let mut total = 0.0;
    let mut unit: Option<String> = None;
    for value in &values {
        let Some(unit_value) = value.downcast::<CSSUnitValue>() else {
            all_same_unit = false;
            break;
        };
        let value_unit = unit_value.unit_str().to_ascii_lowercase();
        match &unit {
            Some(existing) if *existing != value_unit => {
                all_same_unit = false;
                break;
            },
            None => unit = Some(value_unit),
            _ => {},
        }
        total += unit_value.value();
    }
    if all_same_unit {
        let unit_value = CSSUnitValue::new(cx, global, total, unit.as_deref().unwrap_or("number"));
        return DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>());
    }
    let math_sum = CSSMathSum::new(cx, global, values);
    DomRoot::from_ref(math_sum.upcast::<CSSNumericValue>())
}

fn create_product(
    cx: &mut JSContext,
    global: &GlobalScope,
    values: Vec<DomRoot<CSSNumericValue>>,
) -> DomRoot<CSSNumericValue> {
    // "create a CSSMathProduct": all CSSUnitValue with at most one non-number
    // unit collapses to a CSSUnitValue.
    let mut product = 1.0;
    let mut single_unit: Option<String> = None;
    let mut all_units = true;
    for value in &values {
        let Some(unit_value) = value.downcast::<CSSUnitValue>() else {
            all_units = false;
            break;
        };
        product *= unit_value.value();
        let value_unit = unit_value.unit_str().to_ascii_lowercase();
        if value_unit == "number" {
            continue;
        }
        if single_unit.is_some() {
            all_units = false;
            break;
        }
        single_unit = Some(value_unit);
    }
    if all_units {
        let unit_value = CSSUnitValue::new(
            cx,
            global,
            product,
            single_unit.as_deref().unwrap_or("number"),
        );
        return DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>());
    }
    let math_product = CSSMathProduct::new(cx, global, values);
    DomRoot::from_ref(math_product.upcast::<CSSNumericValue>())
}

fn create_min_max(
    cx: &mut JSContext,
    global: &GlobalScope,
    values: Vec<DomRoot<CSSNumericValue>>,
    is_min: bool,
) -> DomRoot<CSSNumericValue> {
    // "create a CSSMathMin/Max": all CSSUnitValue with the same unit collapses
    // to the min/max CSSUnitValue.
    let mut all_same_unit = true;
    let mut extremum: Option<f64> = None;
    let mut unit: Option<String> = None;
    for value in &values {
        let Some(unit_value) = value.downcast::<CSSUnitValue>() else {
            all_same_unit = false;
            break;
        };
        let value_unit = unit_value.unit_str().to_ascii_lowercase();
        match &unit {
            Some(existing) if *existing != value_unit => {
                all_same_unit = false;
                break;
            },
            None => unit = Some(value_unit),
            _ => {},
        }
        let current = unit_value.value();
        extremum = Some(match extremum {
            None => current,
            Some(previous) => {
                if is_min {
                    previous.min(current)
                } else {
                    previous.max(current)
                }
            },
        });
    }
    if all_same_unit {
        let unit_value = CSSUnitValue::new(
            cx,
            global,
            extremum.unwrap_or(0.0),
            unit.as_deref().unwrap_or("number"),
        );
        return DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>());
    }
    if is_min {
        let math_min = CSSMathMin::new(cx, global, values);
        DomRoot::from_ref(math_min.upcast::<CSSNumericValue>())
    } else {
        let math_max = CSSMathMax::new(cx, global, values);
        DomRoot::from_ref(math_max.upcast::<CSSNumericValue>())
    }
}

fn negate_operand(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DomRoot<CSSNumericValue>,
) -> DomRoot<CSSNumericValue> {
    if let Some(unit_value) = value.downcast::<CSSUnitValue>() {
        let negated = CSSUnitValue::new(
            cx,
            global,
            -unit_value.value(),
            &unit_value.unit_str().to_ascii_lowercase(),
        );
        return DomRoot::from_ref(negated.upcast::<CSSNumericValue>());
    }
    if let Some(math_negate) = value.downcast::<CSSMathNegate>() {
        return math_negate.value_root();
    }
    let math_negate = CSSMathNegate::new(cx, global, value);
    DomRoot::from_ref(math_negate.upcast::<CSSNumericValue>())
}

fn invert_operand(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DomRoot<CSSNumericValue>,
) -> Fallible<DomRoot<CSSNumericValue>> {
    if let Some(unit_value) = value.downcast::<CSSUnitValue>() {
        if unit_value.unit_str().eq_ignore_ascii_case("number") {
            if unit_value.value() == 0.0 {
                return Err(Error::Range(c"Division by zero".to_owned()));
            }
            let inverted = CSSUnitValue::new(cx, global, 1.0 / unit_value.value(), "number");
            return Ok(DomRoot::from_ref(inverted.upcast::<CSSNumericValue>()));
        }
    } else if let Some(math_invert) = value.downcast::<CSSMathInvert>() {
        return Ok(math_invert.value_root());
    }
    let math_invert = CSSMathInvert::new(cx, global, value);
    Ok(DomRoot::from_ref(math_invert.upcast::<CSSNumericValue>()))
}

/// Implements `add`/`sub`/`mul`/`div`/`min`/`max`.
pub(crate) fn arithmetic(
    cx: &mut JSContext,
    global: &GlobalScope,
    this_numeric: &CSSNumericValue,
    values: Vec<DomRoot<CSSNumericValue>>,
    op: ArithmeticOp,
) -> Fallible<DomRoot<CSSNumericValue>> {
    if values.is_empty() {
        return Ok(DomRoot::from_ref(this_numeric));
    }

    // Step: validate the combined numeric types.
    let mut types =
        vec![numeric_type_of(this_numeric).map_err(|_| type_error("Invalid CSSNumericValue"))?];
    for value in &values {
        let mut value_type =
            numeric_type_of(value).map_err(|_| type_error("Invalid CSSNumericValue"))?;
        if op == ArithmeticOp::Div {
            value_type.invert();
        }
        types.push(value_type);
    }
    match op {
        ArithmeticOp::Add | ArithmeticOp::Sub | ArithmeticOp::Min | ArithmeticOp::Max => {
            NumericType::add_types(types.iter())
                .map_err(|_| type_error("Cannot add these numeric types"))?;
        },
        ArithmeticOp::Mul | ArithmeticOp::Div => {
            NumericType::multiply_types(types.iter())
                .map_err(|_| type_error("Cannot multiply these numeric types"))?;
        },
    }

    let mut operands = Vec::with_capacity(values.len() + 1);
    operands.push(DomRoot::from_ref(this_numeric));

    match op {
        ArithmeticOp::Add => {
            operands.extend(values);
            Ok(create_sum(cx, global, operands))
        },
        ArithmeticOp::Sub => {
            for value in values {
                operands.push(negate_operand(cx, global, value));
            }
            Ok(create_sum(cx, global, operands))
        },
        ArithmeticOp::Mul => {
            operands.extend(values);
            Ok(create_product(cx, global, operands))
        },
        ArithmeticOp::Div => {
            for value in values {
                operands.push(invert_operand(cx, global, value)?);
            }
            Ok(create_product(cx, global, operands))
        },
        ArithmeticOp::Min | ArithmeticOp::Max => {
            operands.extend(values);
            Ok(create_min_max(
                cx,
                global,
                operands,
                op == ArithmeticOp::Min,
            ))
        },
    }
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-to>
pub(crate) fn to_unit(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: &CSSNumericValue,
    unit: USVString,
) -> Fallible<DomRoot<CSSUnitValue>> {
    let unit_str = unit.0.to_ascii_lowercase();
    if NumericType::from_unit(&unit_str).is_err() {
        return Err(syntax_error("Invalid unit"));
    }
    let parsed = numeric_to_parsed(value).map_err(|_| type_error("Invalid CSSNumericValue"))?;
    let sum = sum_value(&parsed).map_err(|_| type_error("Cannot convert value to a sum"))?;
    if sum.len() != 1 {
        return Err(type_error("Cannot convert value with more than one unit"));
    }
    let (item_value, item_unit) =
        sum_value_item_to_unit(&sum[0]).map_err(|_| type_error("Cannot convert compound unit"))?;
    let converted =
        crate::dom::css::typed_om_numeric::convert_unit(item_value, &item_unit, &unit_str)
            .ok_or_else(|| type_error("Incompatible units"))?;
    Ok(CSSUnitValue::new(cx, global, converted, &unit_str))
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-tosum>
pub(crate) fn to_sum(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: &CSSNumericValue,
    units: Vec<USVString>,
) -> Fallible<DomRoot<CSSMathSum>> {
    let mut requested_units = Vec::with_capacity(units.len());
    for unit in units {
        let unit_str = unit.0.to_ascii_lowercase();
        if NumericType::from_unit(&unit_str).is_err() {
            return Err(syntax_error("Invalid unit"));
        }
        requested_units.push(unit_str);
    }

    let parsed = numeric_to_parsed(value).map_err(|_| type_error("Invalid CSSNumericValue"))?;
    let sum = sum_value(&parsed).map_err(|_| type_error("Cannot convert value to a sum"))?;

    let mut values = Vec::with_capacity(sum.len());
    for item in &sum {
        values.push(
            sum_value_item_to_unit(item).map_err(|_| type_error("Cannot convert compound unit"))?,
        );
    }

    let mut converted_values: Vec<DomRoot<CSSNumericValue>> = Vec::new();
    if requested_units.is_empty() {
        values.sort_by(|left, right| left.1.cmp(&right.1));
        for (item_value, item_unit) in values {
            let unit_value = CSSUnitValue::new(cx, global, item_value, &item_unit);
            converted_values.push(DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>()));
        }
    } else {
        for unit in &requested_units {
            let mut accumulated = 0.0;
            let mut index = 0;
            while index < values.len() {
                if let Some(converted) = crate::dom::css::typed_om_numeric::convert_unit(
                    values[index].0,
                    &values[index].1,
                    unit,
                ) {
                    accumulated += converted;
                    values.remove(index);
                } else {
                    index += 1;
                }
            }
            let unit_value = CSSUnitValue::new(cx, global, accumulated, unit);
            converted_values.push(DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>()));
        }
        if !values.is_empty() {
            return Err(type_error(
                "Cannot convert all values to the requested units",
            ));
        }
    }

    Ok(CSSMathSum::new(cx, global, converted_values))
}

/// Builds DOM numeric objects from a simplified parsed tree (used by
/// `CSSNumericValue.parse` and by the property reification path).
pub(crate) fn build_parsed_numeric(
    cx: &mut JSContext,
    global: &GlobalScope,
    parsed: &ParsedNumeric,
) -> DomRoot<CSSNumericValue> {
    match parsed {
        ParsedNumeric::Unit { value, unit } => {
            let unit_value = CSSUnitValue::new(cx, global, *value, &unit.to_ascii_lowercase());
            DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Sum(values) => {
            let values = values
                .iter()
                .map(|value| build_parsed_numeric(cx, global, value))
                .collect();
            let math_sum = CSSMathSum::new(cx, global, values);
            DomRoot::from_ref(math_sum.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Product(values) => {
            let values = values
                .iter()
                .map(|value| build_parsed_numeric(cx, global, value))
                .collect();
            let math_product = CSSMathProduct::new(cx, global, values);
            DomRoot::from_ref(math_product.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Negate(value) => {
            let value = build_parsed_numeric(cx, global, value);
            let math_negate = CSSMathNegate::new(cx, global, value);
            DomRoot::from_ref(math_negate.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Invert(value) => {
            let value = build_parsed_numeric(cx, global, value);
            let math_invert = CSSMathInvert::new(cx, global, value);
            DomRoot::from_ref(math_invert.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Min(values) => {
            let values = values
                .iter()
                .map(|value| build_parsed_numeric(cx, global, value))
                .collect();
            let math_min = CSSMathMin::new(cx, global, values);
            DomRoot::from_ref(math_min.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Max(values) => {
            let values = values
                .iter()
                .map(|value| build_parsed_numeric(cx, global, value))
                .collect();
            let math_max = CSSMathMax::new(cx, global, values);
            DomRoot::from_ref(math_max.upcast::<CSSNumericValue>())
        },
        ParsedNumeric::Clamp {
            lower,
            value,
            upper,
        } => {
            let lower = build_parsed_numeric(cx, global, lower);
            let value = build_parsed_numeric(cx, global, value);
            let upper = build_parsed_numeric(cx, global, upper);
            let math_clamp = CSSMathClamp::new(cx, global, lower, value, upper);
            DomRoot::from_ref(math_clamp.upcast::<CSSNumericValue>())
        },
    }
}

/// Structural equality used by `CSSNumericValue.equals()`.
pub(crate) fn numeric_equals(left: &CSSNumericValue, right: &CSSNumericValue) -> bool {
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSUnitValue>(),
        right.downcast::<CSSUnitValue>(),
    ) {
        return left.unit_str().eq_ignore_ascii_case(right.unit_str())
            && left.value() == right.value();
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathSum>(),
        right.downcast::<CSSMathSum>(),
    ) {
        return numeric_list_equals(&left.values(), &right.values());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathProduct>(),
        right.downcast::<CSSMathProduct>(),
    ) {
        return numeric_list_equals(&left.values(), &right.values());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathMin>(),
        right.downcast::<CSSMathMin>(),
    ) {
        return numeric_list_equals(&left.values(), &right.values());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathMax>(),
        right.downcast::<CSSMathMax>(),
    ) {
        return numeric_list_equals(&left.values(), &right.values());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathNegate>(),
        right.downcast::<CSSMathNegate>(),
    ) {
        return numeric_equals(&left.value_root(), &right.value_root());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathInvert>(),
        right.downcast::<CSSMathInvert>(),
    ) {
        return numeric_equals(&left.value_root(), &right.value_root());
    }
    if let (Some(left), Some(right)) = (
        left.downcast::<CSSMathClamp>(),
        right.downcast::<CSSMathClamp>(),
    ) {
        return numeric_equals(&left.lower_root(), &right.lower_root())
            && numeric_equals(&left.value_root(), &right.value_root())
            && numeric_equals(&left.upper_root(), &right.upper_root());
    }
    false
}

fn numeric_list_equals(
    left: &[DomRoot<CSSNumericValue>],
    right: &[DomRoot<CSSNumericValue>],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| numeric_equals(left, right))
}

/// `equals()` against a `CSSNumberish` argument.
pub(crate) fn numeric_equals_numberish(
    left: &CSSNumericValue,
    right: &DoubleOrCSSNumericValue,
) -> bool {
    match right {
        DoubleOrCSSNumericValue::Double(value) => {
            left.downcast::<CSSUnitValue>().is_some_and(|unit_value| {
                unit_value.unit_str().eq_ignore_ascii_case("number")
                    && unit_value.value() == **value
            })
        },
        DoubleOrCSSNumericValue::CSSNumericValue(value) => numeric_equals(left, value),
    }
}

/// Rectifies `CSSNumberish` arguments for constructors/variadic methods.
pub(crate) fn rectify_numberish_vec(
    cx: &mut JSContext,
    global: &GlobalScope,
    values: Vec<DoubleOrCSSNumericValue>,
) -> Vec<DomRoot<CSSNumericValue>> {
    values
        .into_iter()
        .map(|value| rectify_numberish_rooted(cx, global, value))
        .collect()
}

/// Converts an `Option<CSSNumberish>` (optional argument, no default).
pub(crate) fn rectify_optional_numberish(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: Option<DoubleOrCSSNumericValue>,
) -> Option<DomRoot<CSSNumericValue>> {
    value.map(|value| rectify_numberish_rooted(cx, global, value))
}

/// Validates that the values' types can be added (constructor checks).
pub(crate) fn validate_addable(values: &[DomRoot<CSSNumericValue>]) -> Fallible<()> {
    let mut types = Vec::with_capacity(values.len());
    for value in values {
        types.push(numeric_type_of(value).map_err(|_| type_error("Invalid CSSNumericValue"))?);
    }
    NumericType::add_types(types.iter())
        .map_err(|_| type_error("Cannot add these numeric types"))?;
    Ok(())
}

/// Validates that the values' types can be multiplied (constructor checks).
pub(crate) fn validate_multipliable(values: &[DomRoot<CSSNumericValue>]) -> Fallible<()> {
    let mut types = Vec::with_capacity(values.len());
    for value in values {
        types.push(numeric_type_of(value).map_err(|_| type_error("Invalid CSSNumericValue"))?);
    }
    NumericType::multiply_types(types.iter())
        .map_err(|_| type_error("Cannot multiply these numeric types"))?;
    Ok(())
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-parse>
pub(crate) fn parse_numeric_from_string(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> Fallible<DomRoot<CSSNumericValue>> {
    let parsed = parse_css_numeric(text).map_err(|_| syntax_error("Invalid CSSNumericValue"))?;
    let parsed = simplify(parsed);
    parsed_numeric_type(&parsed).map_err(|_| syntax_error("Invalid CSSNumericValue"))?;
    Ok(build_parsed_numeric(cx, global, &parsed))
}
