/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM numeric core.
//!
//! This module owns everything the `CSSNumericValue` family needs that is not
//! a DOM object:
//!
//! * the specification's "numeric type" (a map of base-type exponents plus an
//!   optional percent hint) and its algorithms
//!   (<https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-typed-om-numeric-type>),
//! * the unit model (which units exist, which family they belong to, and the
//!   absolute conversion ratios), used by `to()`/`toSum()` serialization,
//! * the "parse a CSSNumericValue" grammar
//!   (<https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-parse>)
//!   producing a `ParsedNumeric` tree,
//! * the parse-time simplification pass (calc nesting flattening, canonical
//!   unit folding and same-unit term merging), and
//! * the "create a sum value" algorithm
//!   (<https://drafts.css-houdini.org/css-typed-om-1/#create-a-sum-value>)
//!   consumed by `CSSNumericValue.to()` / `toSum()`.
//!
//! NOTE (reuse note): `style::typed_om` (stylo) carries a parallel internal
//! representation (`NumericType`, `NumericValue`, `SumValue`). It is *not*
//! reused here for two structural reasons: its `NumericType` keeps the
//! exponent array private behind no accessor (the `type()` dictionary needs
//! the raw exponents), and its `NoCalcNumeric` covers only
//! length/angle/time/number/percent (frequency/resolution/flex are TODO), so
//! `to()`/`toSum()` over `Hz`/`dpi`/`fr` would fail. The algorithms below are
//! transcribed step by step from the specification instead.

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::{
    CSSNumericBaseType, CSSNumericType,
};
use cssparser::{BasicParseError, ParseError, Parser as CssParser, ToCss, Token};

/// The number of base types tracked by [`NumericType`].
const NUMERIC_BASE_TYPE_COUNT: usize = 7;

/// A base type of the Typed OM numeric type map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum NumericBase {
    Length = 0,
    Angle = 1,
    Time = 2,
    Frequency = 3,
    Resolution = 4,
    Flex = 5,
    Percent = 6,
}

impl NumericBase {
    /// All base types, in specification order.
    pub(crate) const ALL: [NumericBase; NUMERIC_BASE_TYPE_COUNT] = [
        NumericBase::Length,
        NumericBase::Angle,
        NumericBase::Time,
        NumericBase::Frequency,
        NumericBase::Resolution,
        NumericBase::Flex,
        NumericBase::Percent,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn to_idl(self) -> CSSNumericBaseType {
        match self {
            NumericBase::Length => CSSNumericBaseType::Length,
            NumericBase::Angle => CSSNumericBaseType::Angle,
            NumericBase::Time => CSSNumericBaseType::Time,
            NumericBase::Frequency => CSSNumericBaseType::Frequency,
            NumericBase::Resolution => CSSNumericBaseType::Resolution,
            NumericBase::Flex => CSSNumericBaseType::Flex,
            NumericBase::Percent => CSSNumericBaseType::Percent,
        }
    }
}

/// The CSS Typed OM "numeric type": an exponent per base type plus an optional
/// percent hint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NumericType {
    exponents: [i32; NUMERIC_BASE_TYPE_COUNT],
    percent_hint: Option<NumericBase>,
}

impl NumericType {
    /// A numeric type whose exponent map is empty (`<number>`).
    pub(crate) fn number() -> Self {
        Self::default()
    }

    fn with_base_type(base: NumericBase) -> Self {
        let mut result = Self::default();
        result.exponents[base.index()] = 1;
        result
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#create-a-type-from-a-string>
    pub(crate) fn from_unit(unit: &str) -> Result<Self, ()> {
        let unit = unit.to_ascii_lowercase();
        match unit.as_str() {
            "number" => Ok(Self::number()),
            "percent" => Ok(Self::with_base_type(NumericBase::Percent)),
            _ => match unit_base_type(&unit) {
                Some(base) => Ok(Self::with_base_type(base)),
                None => Err(()),
            },
        }
    }

    pub(crate) fn exponent(&self, base: NumericBase) -> i32 {
        self.exponents[base.index()]
    }

    fn set_exponent(&mut self, base: NumericBase, value: i32) {
        self.exponents[base.index()] = value;
    }

    fn add_exponent(&mut self, base: NumericBase, delta: i32) {
        self.exponents[base.index()] += delta;
    }

    fn non_zero_count(&self) -> usize {
        self.exponents.iter().filter(|e| **e != 0).count()
    }

    fn has_no_non_zero_entries(&self) -> bool {
        self.non_zero_count() == 0
    }

    fn only_non_zero_entry_is(&self, base: NumericBase, exponent: i32) -> bool {
        self.non_zero_count() == 1 && self.exponent(base) == exponent
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-invert-a-type>
    pub(crate) fn invert(&mut self) {
        for exponent in self.exponents.iter_mut() {
            *exponent = -*exponent;
        }
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#apply-the-percent-hint>
    pub(crate) fn apply_percent_hint(&mut self, hint: NumericBase) {
        if self.percent_hint.is_some() {
            return;
        }
        self.percent_hint = Some(hint);
        if hint != NumericBase::Percent {
            let percent = self.exponent(NumericBase::Percent);
            if percent != 0 {
                self.add_exponent(hint, percent);
                self.set_exponent(NumericBase::Percent, 0);
            }
        }
    }

    fn has_null_percent_hint(&self) -> bool {
        self.percent_hint.is_none()
    }

    /// Matches `<length>` (percentages disallowed).
    pub(crate) fn matches_length(&self) -> bool {
        self.only_non_zero_entry_is(NumericBase::Length, 1) && self.has_null_percent_hint()
    }

    /// Matches `<length-percentage>`.
    pub(crate) fn matches_length_percentage(&self) -> bool {
        (self.only_non_zero_entry_is(NumericBase::Length, 1)
            && (self.has_null_percent_hint() || self.percent_hint == Some(NumericBase::Length)))
            || self.matches_percentage()
    }

    /// Matches `<angle>` (percentages disallowed).
    pub(crate) fn matches_angle(&self) -> bool {
        self.only_non_zero_entry_is(NumericBase::Angle, 1) && self.has_null_percent_hint()
    }

    /// Matches `<percentage>`.
    pub(crate) fn matches_percentage(&self) -> bool {
        self.only_non_zero_entry_is(NumericBase::Percent, 1)
            && (self.has_null_percent_hint() || self.percent_hint == Some(NumericBase::Percent))
    }

    /// Matches `<number>`.
    pub(crate) fn matches_number(&self) -> bool {
        self.has_no_non_zero_entries() && self.has_null_percent_hint()
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-add-two-types>
    pub(crate) fn add_two_types(type1: &NumericType, type2: &NumericType) -> Result<Self, ()> {
        let mut type1 = *type1;
        let mut type2 = *type2;

        // Step 2.
        match (type1.percent_hint, type2.percent_hint) {
            (Some(hint1), Some(hint2)) if hint1 != hint2 => return Err(()),
            (Some(hint), None) => type2.apply_percent_hint(hint),
            (None, Some(hint)) => type1.apply_percent_hint(hint),
            _ => {},
        }

        // Step 3.
        if type1.exponents == type2.exponents {
            return Ok(type1);
        }

        let type1_has_percent = type1.exponent(NumericBase::Percent) != 0;
        let type2_has_percent = type2.exponent(NumericBase::Percent) != 0;
        let type1_has_other = NumericBase::ALL
            .iter()
            .any(|base| *base != NumericBase::Percent && type1.exponent(*base) != 0);
        let type2_has_other = NumericBase::ALL
            .iter()
            .any(|base| *base != NumericBase::Percent && type2.exponent(*base) != 0);

        if (type1_has_percent || type2_has_percent) && (type1_has_other || type2_has_other) {
            // Step 3.1-3.3: provisionally apply each non-percent hint.
            for hint in NumericBase::ALL {
                if hint == NumericBase::Percent {
                    continue;
                }
                let mut candidate1 = type1;
                let mut candidate2 = type2;
                candidate1.apply_percent_hint(hint);
                candidate2.apply_percent_hint(hint);
                if candidate1.exponents == candidate2.exponents {
                    return Ok(candidate1);
                }
            }
            return Err(());
        }

        Err(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-multiply-two-types>
    pub(crate) fn multiply_two_types(type1: &NumericType, type2: &NumericType) -> Result<Self, ()> {
        let mut type1 = *type1;
        let mut type2 = *type2;

        match (type1.percent_hint, type2.percent_hint) {
            (Some(hint1), Some(hint2)) if hint1 != hint2 => return Err(()),
            (Some(hint), None) => type2.apply_percent_hint(hint),
            (None, Some(hint)) => type1.apply_percent_hint(hint),
            _ => {},
        }

        for base in NumericBase::ALL {
            let power = type2.exponent(base);
            if power != 0 {
                type1.add_exponent(base, power);
            }
        }

        Ok(type1)
    }

    pub(crate) fn add_types<'a, I>(types: I) -> Result<Self, ()>
    where
        I: Iterator<Item = &'a NumericType>,
    {
        let mut result: Option<NumericType> = None;
        for next in types {
            result = Some(match result {
                None => *next,
                Some(current) => Self::add_two_types(&current, next)?,
            });
        }
        result.ok_or(())
    }

    pub(crate) fn multiply_types<'a, I>(types: I) -> Result<Self, ()>
    where
        I: Iterator<Item = &'a NumericType>,
    {
        let mut result: Option<NumericType> = None;
        for next in types {
            result = Some(match result {
                None => *next,
                Some(current) => Self::multiply_two_types(&current, next)?,
            });
        }
        result.ok_or(())
    }

    /// Produces the `CSSNumericType` dictionary exposed by `type()`. Only
    /// non-zero exponents are present; absent members read as `undefined`,
    /// which is what the specification's dictionary semantics require.
    pub(crate) fn to_dictionary(&self) -> CSSNumericType {
        let mut dict = CSSNumericType::default();
        for base in NumericBase::ALL {
            let exponent = self.exponent(base);
            if exponent == 0 {
                continue;
            }
            match base {
                NumericBase::Length => dict.length = Some(exponent),
                NumericBase::Angle => dict.angle = Some(exponent),
                NumericBase::Time => dict.time = Some(exponent),
                NumericBase::Frequency => dict.frequency = Some(exponent),
                NumericBase::Resolution => dict.resolution = Some(exponent),
                NumericBase::Flex => dict.flex = Some(exponent),
                NumericBase::Percent => dict.percent = Some(exponent),
            }
        }
        dict.percentHint = self.percent_hint.map(|base| base.to_idl());
        dict
    }
}

/// Relative font/viewport/container length units. They have no absolute ratio
/// outside of a computed context.
const RELATIVE_LENGTH_UNITS: &[&str] = &[
    "cap", "ch", "em", "ex", "ic", "lh", "rcap", "rch", "rem", "rex", "ric", "rlh", "vw", "vh",
    "vi", "vb", "vmin", "vmax", "svw", "svh", "svi", "svb", "svmin", "svmax", "lvw", "lvh", "lvi",
    "lvb", "lvmin", "lvmax", "dvw", "dvh", "dvi", "dvb", "dvmin", "dvmax", "cqw", "cqh", "cqi",
    "cqb", "cqmin", "cqmax",
];

/// Absolute length units and their value in the canonical unit (`px`).
const ABSOLUTE_LENGTH_UNITS: &[(&str, f64)] = &[
    ("px", 1.0),
    ("cm", 96.0 / 2.54),
    ("mm", 96.0 / 25.4),
    ("q", 96.0 / 101.6),
    ("in", 96.0),
    ("pt", 96.0 / 72.0),
    ("pc", 16.0),
];

/// Angle units and their value in the canonical unit (`deg`).
const ANGLE_UNITS: &[(&str, f64)] = &[
    ("deg", 1.0),
    ("grad", 0.9),
    ("rad", 180.0 / std::f64::consts::PI),
    ("turn", 360.0),
];

/// Time units and their value in the canonical unit (`s`).
const TIME_UNITS: &[(&str, f64)] = &[("s", 1.0), ("ms", 0.001)];

/// Frequency units and their value in the canonical unit (`Hz`).
const FREQUENCY_UNITS: &[(&str, f64)] = &[("hz", 1.0), ("khz", 1000.0)];

/// Resolution units and their value in the canonical unit (`dppx`).
const RESOLUTION_UNITS: &[(&str, f64)] =
    &[("dppx", 1.0), ("dpi", 1.0 / 96.0), ("dpcm", 2.54 / 96.0)];

fn lookup(table: &[(&str, f64)], unit: &str) -> Option<f64> {
    table
        .iter()
        .find(|(name, _)| *name == unit)
        .map(|(_, factor)| *factor)
}

/// Returns the base type a unit belongs to, or `None` if it is not a valid
/// Typed OM unit.
fn unit_base_type(unit: &str) -> Option<NumericBase> {
    if lookup(ABSOLUTE_LENGTH_UNITS, unit).is_some() || RELATIVE_LENGTH_UNITS.contains(&unit) {
        return Some(NumericBase::Length);
    }
    if lookup(ANGLE_UNITS, unit).is_some() {
        return Some(NumericBase::Angle);
    }
    if lookup(TIME_UNITS, unit).is_some() {
        return Some(NumericBase::Time);
    }
    if lookup(FREQUENCY_UNITS, unit).is_some() {
        return Some(NumericBase::Frequency);
    }
    if lookup(RESOLUTION_UNITS, unit).is_some() {
        return Some(NumericBase::Resolution);
    }
    if unit == "fr" {
        return Some(NumericBase::Flex);
    }
    None
}

fn canonical_unit(base: NumericBase) -> &'static str {
    match base {
        NumericBase::Length => "px",
        NumericBase::Angle => "deg",
        NumericBase::Time => "s",
        NumericBase::Frequency => "hz",
        NumericBase::Resolution => "dppx",
        NumericBase::Flex => "fr",
        NumericBase::Percent => "percent",
    }
}

fn unit_factor(unit: &str) -> Option<f64> {
    lookup(ABSOLUTE_LENGTH_UNITS, unit)
        .or_else(|| lookup(ANGLE_UNITS, unit))
        .or_else(|| lookup(TIME_UNITS, unit))
        .or_else(|| lookup(FREQUENCY_UNITS, unit))
        .or_else(|| lookup(RESOLUTION_UNITS, unit))
}

/// Converts `value` from `from` to `to`, returning `None` if the units are not
/// compatible (or the source is a relative unit).
pub(crate) fn convert_unit(value: f64, from: &str, to: &str) -> Option<f64> {
    let from = from.to_ascii_lowercase();
    let to = to.to_ascii_lowercase();
    if from == to {
        return Some(value);
    }
    if from == "number" || to == "number" || from == "percent" || to == "percent" {
        return None;
    }
    let from_base = unit_base_type(&from)?;
    let to_base = unit_base_type(&to)?;
    if from_base != to_base {
        return None;
    }
    let from_factor = unit_factor(&from)?;
    let to_factor = unit_factor(&to)?;
    Some(value * from_factor / to_factor)
}

/// Canonicalises `value`/`unit` when the unit is an absolute one (the "create a
/// sum value" step). Returns `None` for relative units, `number` and `percent`.
pub(crate) fn canonicalize_unit(value: f64, unit: &str) -> Option<(f64, String)> {
    let unit = unit.to_ascii_lowercase();
    let base = unit_base_type(&unit)?;
    let factor = unit_factor(&unit)?;
    Some((value * factor, canonical_unit(base).to_string()))
}

/// Serializes a CSS `<number>` (shortest round-trippable form).
pub(crate) fn serialize_number(value: f64) -> String {
    let mut result = String::new();
    value.to_css(&mut result).unwrap();
    result
}

/// The parsed representation of a CSS numeric expression produced by
/// [`parse_css_numeric`]. This mirrors the calc grammar and is converted to
/// DOM objects by `typed_om_values::build_parsed_numeric`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ParsedNumeric {
    Unit {
        value: f64,
        unit: String,
    },
    Sum(Vec<ParsedNumeric>),
    Product(Vec<ParsedNumeric>),
    Negate(Box<ParsedNumeric>),
    Invert(Box<ParsedNumeric>),
    Min(Vec<ParsedNumeric>),
    Max(Vec<ParsedNumeric>),
    Clamp {
        lower: Box<ParsedNumeric>,
        value: Box<ParsedNumeric>,
        upper: Box<ParsedNumeric>,
    },
}

impl ParsedNumeric {
    fn unit(value: f64, unit: impl Into<String>) -> ParsedNumeric {
        ParsedNumeric::Unit {
            value,
            unit: unit.into(),
        }
    }
}

fn numeric_token(input: &mut CssParser) -> Result<(f64, String), BasicParseError> {
    let token = input.next()?.clone();
    match token {
        Token::Number { value, .. } => Ok((value as f64, "number".to_string())),
        Token::Percentage { unit_value, .. } => {
            Ok(((unit_value as f64) * 100.0, "percent".to_string()))
        },
        Token::Dimension { value, unit, .. } => {
            let unit = unit.to_ascii_lowercase();
            if NumericType::from_unit(&unit).is_err() {
                return Err(BasicParseError::unexpected_token());
            }
            Ok((value as f64, unit))
        },
        _ => Err(BasicParseError::unexpected_token()),
    }
}

fn parse_function_arguments(input: &mut CssParser) -> Result<Vec<ParsedNumeric>, ParseError<()>> {
    let mut arguments = vec![parse_sum(input)?];
    while input.try_parse(|input| input.expect_comma()).is_ok() {
        arguments.push(parse_sum(input)?);
    }
    Ok(arguments)
}

fn parse_value(input: &mut CssParser) -> Result<ParsedNumeric, ParseError<()>> {
    if let Ok((value, unit)) = input.try_parse(|input| numeric_token(input)) {
        return Ok(ParsedNumeric::unit(value, unit));
    }

    let token = input.next().map_err(ParseError::from)?.clone();
    match token {
        Token::ParenthesisBlock => input.parse_nested_block(|nested| parse_sum(nested)),
        Token::Function(name) => {
            let name = name.to_ascii_lowercase();
            match name.as_str() {
                "calc" => {
                    let inner = input.parse_nested_block(|nested| parse_sum(nested))?;
                    Ok(ParsedNumeric::Sum(vec![inner]))
                },
                "min" => {
                    Ok(ParsedNumeric::Min(input.parse_nested_block(|nested| {
                        parse_function_arguments(nested)
                    })?))
                },
                "max" => {
                    Ok(ParsedNumeric::Max(input.parse_nested_block(|nested| {
                        parse_function_arguments(nested)
                    })?))
                },
                "clamp" => {
                    let arguments =
                        input.parse_nested_block(|nested| parse_function_arguments(nested))?;
                    if arguments.len() != 3 {
                        return Err(ParseError::custom(()));
                    }
                    let mut arguments = arguments.into_iter();
                    Ok(ParsedNumeric::Clamp {
                        lower: Box::new(arguments.next().unwrap()),
                        value: Box::new(arguments.next().unwrap()),
                        upper: Box::new(arguments.next().unwrap()),
                    })
                },
                _ => Err(ParseError::custom(())),
            }
        },
        _ => Err(ParseError::custom(())),
    }
}

fn parse_product(input: &mut CssParser) -> Result<ParsedNumeric, ParseError<()>> {
    let mut factors = vec![parse_value(input)?];
    loop {
        input.skip_whitespace();
        if input.try_parse(|input| input.expect_delim('*')).is_ok() {
            factors.push(parse_value(input)?);
        } else if input.try_parse(|input| input.expect_delim('/')).is_ok() {
            factors.push(ParsedNumeric::Invert(Box::new(parse_value(input)?)));
        } else {
            break;
        }
    }
    if factors.len() == 1 {
        Ok(factors.pop().unwrap())
    } else {
        Ok(ParsedNumeric::Product(factors))
    }
}

fn parse_sum(input: &mut CssParser) -> Result<ParsedNumeric, ParseError<()>> {
    let mut terms = vec![parse_product(input)?];
    loop {
        input.skip_whitespace();
        if input.try_parse(|input| input.expect_delim('+')).is_ok() {
            terms.push(parse_product(input)?);
        } else if input.try_parse(|input| input.expect_delim('-')).is_ok() {
            terms.push(ParsedNumeric::Negate(Box::new(parse_product(input)?)));
        } else {
            break;
        }
    }
    if terms.len() == 1 {
        Ok(terms.pop().unwrap())
    } else {
        Ok(ParsedNumeric::Sum(terms))
    }
}

fn parse_top_level(input: &mut CssParser) -> Result<ParsedNumeric, ParseError<()>> {
    input.skip_whitespace();
    if let Ok((value, unit)) = input.try_parse(|input| numeric_token(input)) {
        input.expect_exhausted().map_err(ParseError::from)?;
        return Ok(ParsedNumeric::unit(value, unit));
    }

    let token = input.next().map_err(ParseError::from)?.clone();
    let parsed = match token {
        Token::Function(name) => {
            let name = name.to_ascii_lowercase();
            match name.as_str() {
                "calc" => {
                    let inner = input.parse_nested_block(|nested| parse_sum(nested))?;
                    ParsedNumeric::Sum(vec![inner])
                },
                "min" => ParsedNumeric::Min(
                    input.parse_nested_block(|nested| parse_function_arguments(nested))?,
                ),
                "max" => ParsedNumeric::Max(
                    input.parse_nested_block(|nested| parse_function_arguments(nested))?,
                ),
                "clamp" => {
                    let arguments =
                        input.parse_nested_block(|nested| parse_function_arguments(nested))?;
                    if arguments.len() != 3 {
                        return Err(ParseError::custom(()));
                    }
                    let mut arguments = arguments.into_iter();
                    ParsedNumeric::Clamp {
                        lower: Box::new(arguments.next().unwrap()),
                        value: Box::new(arguments.next().unwrap()),
                        upper: Box::new(arguments.next().unwrap()),
                    }
                },
                _ => return Err(ParseError::custom(())),
            }
        },
        _ => return Err(ParseError::custom(())),
    };
    input.expect_exhausted().map_err(ParseError::from)?;
    Ok(parsed)
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue-parse>
pub(crate) fn parse_css_numeric(input: &str) -> Result<ParsedNumeric, ()> {
    let mut parser = CssParser::new(input);
    parser
        .parse_entirely(|parser| parse_top_level(parser))
        .map_err(|_| ())
}

/// Adds two `ParsedNumeric` unit terms into `terms`, merging identical units.
fn merge_unit_term(terms: &mut Vec<ParsedNumeric>, value: f64, unit: String) {
    for term in terms.iter_mut() {
        if let ParsedNumeric::Unit {
            value: existing_value,
            unit: existing_unit,
        } = term
        {
            if *existing_unit == unit {
                *existing_value += value;
                return;
            }
        }
    }
    terms.push(ParsedNumeric::unit(value, unit));
}

fn collapse_all_unit_product(values: &[ParsedNumeric]) -> Option<ParsedNumeric> {
    let mut product = 1.0;
    let mut single_unit: Option<String> = None;
    for value in values {
        let ParsedNumeric::Unit { value, unit } = value else {
            return None;
        };
        product *= *value;
        let unit = unit.to_ascii_lowercase();
        if unit == "number" {
            continue;
        }
        if single_unit.is_some() {
            return None;
        }
        single_unit = Some(unit);
    }
    Some(ParsedNumeric::unit(
        product,
        single_unit.unwrap_or_else(|| "number".to_string()),
    ))
}

/// Parse-time simplification: flattens nested `calc()` sums, folds absolute
/// units into their canonical unit, merges plain unit summands and collapses
/// all-unit products (matching the "create a CSSMathSum/Product" behaviour the
/// WPT parse tests pin).
pub(crate) fn simplify(node: ParsedNumeric) -> ParsedNumeric {
    match node {
        ParsedNumeric::Unit { .. } => node,
        ParsedNumeric::Sum(children) => {
            let mut flattened = Vec::new();
            for child in children {
                match simplify(child) {
                    ParsedNumeric::Sum(inner) => flattened.extend(inner),
                    other => flattened.push(other),
                }
            }
            let mut merged: Vec<ParsedNumeric> = Vec::new();
            for term in flattened {
                match term {
                    ParsedNumeric::Unit { value, unit } => {
                        let (value, unit) = canonicalize_unit(value, &unit)
                            .unwrap_or((value, unit.to_ascii_lowercase()));
                        merge_unit_term(&mut merged, value, unit);
                    },
                    other => merged.push(other),
                }
            }
            ParsedNumeric::Sum(merged)
        },
        ParsedNumeric::Product(children) => {
            let children: Vec<ParsedNumeric> = children.into_iter().map(simplify).collect();
            match collapse_all_unit_product(&children) {
                Some(collapsed) => collapsed,
                None => ParsedNumeric::Product(children),
            }
        },
        ParsedNumeric::Negate(inner) => ParsedNumeric::Negate(Box::new(simplify(*inner))),
        ParsedNumeric::Invert(inner) => ParsedNumeric::Invert(Box::new(simplify(*inner))),
        ParsedNumeric::Min(children) => {
            ParsedNumeric::Min(children.into_iter().map(simplify).collect())
        },
        ParsedNumeric::Max(children) => {
            ParsedNumeric::Max(children.into_iter().map(simplify).collect())
        },
        ParsedNumeric::Clamp {
            lower,
            value,
            upper,
        } => ParsedNumeric::Clamp {
            lower: Box::new(simplify(*lower)),
            value: Box::new(simplify(*value)),
            upper: Box::new(simplify(*upper)),
        },
    }
}

/// Computes the numeric type of a parsed tree, validating add/multiply
/// compatibility at the same time. Used by the parse entry points, where a
/// failure surfaces as a `SyntaxError`.
pub(crate) fn parsed_numeric_type(node: &ParsedNumeric) -> Result<NumericType, ()> {
    match node {
        ParsedNumeric::Unit { unit, .. } => NumericType::from_unit(unit),
        ParsedNumeric::Sum(children)
        | ParsedNumeric::Min(children)
        | ParsedNumeric::Max(children) => {
            let mut types = Vec::with_capacity(children.len());
            for child in children {
                types.push(parsed_numeric_type(child)?);
            }
            NumericType::add_types(types.iter())
        },
        ParsedNumeric::Product(children) => {
            let mut types = Vec::with_capacity(children.len());
            for child in children {
                types.push(parsed_numeric_type(child)?);
            }
            NumericType::multiply_types(types.iter())
        },
        ParsedNumeric::Negate(inner) => parsed_numeric_type(inner),
        ParsedNumeric::Invert(inner) => {
            let mut numeric_type = parsed_numeric_type(inner)?;
            numeric_type.invert();
            Ok(numeric_type)
        },
        ParsedNumeric::Clamp {
            lower,
            value,
            upper,
        } => {
            let types = [
                parsed_numeric_type(lower)?,
                parsed_numeric_type(value)?,
                parsed_numeric_type(upper)?,
            ];
            NumericType::add_types(types.iter())
        },
    }
}

/// One entry of a "sum value": a scalar plus a unit map (unit → exponent).
#[derive(Clone, Debug)]
pub(crate) struct SumValueItem {
    pub(crate) value: f64,
    pub(crate) units: Vec<(String, i32)>,
}

fn normalized_unit_map(mut units: Vec<(String, i32)>) -> Vec<(String, i32)> {
    units.sort_by(|a, b| a.0.cmp(&b.0));
    units
}

fn product_of_unit_maps(left: &[(String, i32)], right: &[(String, i32)]) -> Vec<(String, i32)> {
    let mut result: Vec<(String, i32)> = left.to_vec();
    for (unit, power) in right {
        if let Some(entry) = result.iter_mut().find(|(name, _)| name == unit) {
            entry.1 += *power;
        } else {
            result.push((unit.clone(), *power));
        }
    }
    result.retain(|(_, power)| *power != 0);
    normalized_unit_map(result)
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#create-a-sum-value>
pub(crate) fn sum_value(node: &ParsedNumeric) -> Result<Vec<SumValueItem>, ()> {
    match node {
        ParsedNumeric::Unit { value, unit } => {
            let unit = unit.to_ascii_lowercase();
            let (value, unit) = canonicalize_unit(*value, &unit).unwrap_or((*value, unit));
            if unit == "number" {
                return Ok(vec![SumValueItem {
                    value,
                    units: Vec::new(),
                }]);
            }
            Ok(vec![SumValueItem {
                value,
                units: vec![(unit, 1)],
            }])
        },
        ParsedNumeric::Sum(children) => {
            let mut values: Vec<SumValueItem> = Vec::new();
            for child in children {
                let child_value = sum_value(child)?;
                for item in child_value {
                    if let Some(existing) = values
                        .iter_mut()
                        .find(|existing| existing.units == item.units)
                    {
                        existing.value += item.value;
                    } else {
                        values.push(item);
                    }
                }
            }
            // Step 3: the unit maps must be addable.
            let mut item_types = Vec::with_capacity(values.len());
            for item in &values {
                let mut item_type = NumericType::number();
                for (unit, power) in &item.units {
                    let mut unit_type = NumericType::from_unit(unit)?;
                    for base in NumericBase::ALL {
                        unit_type.set_exponent(base, unit_type.exponent(base) * *power);
                    }
                    item_type = NumericType::add_two_types(&item_type, &unit_type)?;
                }
                item_types.push(item_type);
            }
            NumericType::add_types(item_types.iter())?;
            Ok(values)
        },
        ParsedNumeric::Product(children) => {
            let mut values = vec![SumValueItem {
                value: 1.0,
                units: Vec::new(),
            }];
            for child in children {
                let new_values = sum_value(child)?;
                let mut temp = Vec::new();
                for left in &values {
                    for right in &new_values {
                        temp.push(SumValueItem {
                            value: left.value * right.value,
                            units: product_of_unit_maps(&left.units, &right.units),
                        });
                    }
                }
                values = temp;
            }
            Ok(values)
        },
        ParsedNumeric::Negate(inner) => {
            let mut values = sum_value(inner)?;
            for item in &mut values {
                item.value = -item.value;
            }
            Ok(values)
        },
        ParsedNumeric::Invert(inner) => {
            let mut values = sum_value(inner)?;
            if values.len() != 1 {
                return Err(());
            }
            let item = &mut values[0];
            item.value = 1.0 / item.value;
            for (_, power) in &mut item.units {
                *power = -*power;
            }
            Ok(values)
        },
        ParsedNumeric::Min(children) | ParsedNumeric::Max(children) => {
            let is_min = matches!(node, ParsedNumeric::Min(_));
            let mut arguments = Vec::with_capacity(children.len());
            for child in children {
                let values = sum_value(child)?;
                if values.len() > 1 {
                    return Err(());
                }
                arguments.push(values.into_iter().next().unwrap());
            }
            let first_units = arguments[0].units.clone();
            if !arguments.iter().all(|item| item.units == first_units) {
                return Err(());
            }
            let result = if is_min {
                arguments
                    .into_iter()
                    .min_by(|a, b| a.value.total_cmp(&b.value))
                    .unwrap()
            } else {
                arguments
                    .into_iter()
                    .max_by(|a, b| a.value.total_cmp(&b.value))
                    .unwrap()
            };
            Ok(vec![result])
        },
        ParsedNumeric::Clamp {
            lower,
            value,
            upper,
        } => {
            let lower = sum_value(lower)?;
            let value = sum_value(value)?;
            let upper = sum_value(upper)?;
            if lower.len() > 1 || value.len() > 1 || upper.len() > 1 {
                return Err(());
            }
            let lower = lower.into_iter().next().unwrap();
            let mut value_item = value.into_iter().next().unwrap();
            let upper = upper.into_iter().next().unwrap();
            if lower.units != value_item.units || lower.units != upper.units {
                return Err(());
            }
            value_item.value = value_item.value.max(lower.value).min(upper.value);
            Ok(vec![value_item])
        },
    }
}

/// A single sum-value item as a CSS unit value: `(value, unit)`. Items with a
/// compound unit map (or an exponent different from 1) cannot be expressed.
pub(crate) fn sum_value_item_to_unit(item: &SumValueItem) -> Result<(f64, String), ()> {
    if item.units.len() > 1 {
        return Err(());
    }
    if item.units.is_empty() {
        return Ok((item.value, "number".to_string()));
    }
    let (unit, power) = &item.units[0];
    if *power != 1 {
        return Err(());
    }
    Ok((item.value, unit.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(value: f64, unit: &str) -> ParsedNumeric {
        ParsedNumeric::Unit {
            value,
            unit: unit.to_string(),
        }
    }

    #[test]
    fn parse_plain_number() {
        let parsed = parse_css_numeric("42").expect("parse 42");
        assert_eq!(parsed, unit(42.0, "number"));
    }

    #[test]
    fn parse_dimension_and_percentage() {
        let parsed = parse_css_numeric("1.5px").expect("parse 1.5px");
        assert_eq!(parsed, unit(1.5, "px"));
        // A CSS percentage token is stored as its `percent`-unit value.
        let parsed = parse_css_numeric("50%").expect("parse 50%");
        assert_eq!(parsed, unit(5000.0 / 100.0, "percent"));
    }

    #[test]
    fn parse_calc_normalizes_to_sum() {
        let parsed = parse_css_numeric("calc(1px + 2px)").expect("parse calc");
        match simplify(parsed) {
            ParsedNumeric::Sum(terms) => {
                assert_eq!(terms, vec![unit(3.0, "px")]);
            },
            other => panic!("expected simplified sum, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_unknown_unit_and_garbage() {
        assert!(parse_css_numeric("1bogusunit").is_err());
        assert!(parse_css_numeric("foo").is_err());
        assert!(parse_css_numeric("").is_err());
        assert!(parse_css_numeric("1px 2px").is_err());
    }

    #[test]
    fn parse_min_max_clamp_shapes() {
        let parsed = parse_css_numeric("clamp(1px, 2px, 3px)").expect("parse clamp");
        match parsed {
            ParsedNumeric::Clamp {
                lower,
                value,
                upper,
            } => {
                assert_eq!(*lower, unit(1.0, "px"));
                assert_eq!(*value, unit(2.0, "px"));
                assert_eq!(*upper, unit(3.0, "px"));
            },
            other => panic!("expected clamp, got {other:?}"),
        }
        assert!(matches!(
            parse_css_numeric("min(1px, 2em)"),
            Ok(ParsedNumeric::Min(_))
        ));
        assert!(matches!(
            parse_css_numeric("max(1px)"),
            Ok(ParsedNumeric::Max(_))
        ));
        assert!(parse_css_numeric("clamp(1px, 2px)").is_err());
    }

    #[test]
    fn type_arithmetic_units() {
        let length = NumericType::from_unit("px").unwrap();
        assert!(length.matches_length());
        assert!(!length.matches_angle());

        // px * s / s has a length exponent of 1 again (via multiply_two_types
        // and an inverted factor's negative exponent).
        let px = NumericType::from_unit("px").unwrap();
        let seconds = NumericType::from_unit("s").unwrap();
        let product = NumericType::multiply_two_types(&px, &seconds).unwrap();
        let inverse = NumericType::from_unit("s").unwrap();
        let divided = NumericType::multiply_two_types(&product, &inverse).unwrap();
        assert!(divided.matches_length());
    }

    #[test]
    fn convert_unit_chain() {
        // 1in == 96px; 2.54cm == 1in; time/frequency/resolution conversions.
        assert_eq!(convert_unit(1.0, "in", "px"), Some(96.0));
        assert_eq!(convert_unit(2.54, "cm", "in"), Some(1.0));
        assert_eq!(convert_unit(1.0, "s", "ms"), Some(1000.0));
        assert_eq!(convert_unit(1.0, "khz", "hz"), Some(1000.0));
        assert_eq!(convert_unit(96.0, "dpi", "dppx"), Some(1.0));
        // Percent and number are not convertible into each other.
        assert_eq!(convert_unit(1.0, "percent", "number"), None);
        assert_eq!(convert_unit(1.0, "px", "deg"), None);
    }

    #[test]
    fn sum_value_item_extraction() {
        let parsed = parse_css_numeric("1in + 24px").expect("parse sum");
        let items = sum_value(&simplify(parsed)).expect("sum value");
        assert_eq!(items.len(), 1);
        let (value, unit) = sum_value_item_to_unit(&items[0]).expect("single unit");
        assert_eq!(unit, "px");
        assert_eq!(value, 120.0);
    }

    #[test]
    fn serialize_number_roundtrip() {
        assert_eq!(serialize_number(0.0), "0");
        assert_eq!(serialize_number(-3.0), "-3");
    }
}
