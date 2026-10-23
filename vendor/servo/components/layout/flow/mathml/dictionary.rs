/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The MathML Core operator dictionary, and the `<mo>` property resolution
//! algorithm.
//!
//! * <https://w3c.github.io/mathml-core/#operator-dictionary>
//! * <https://w3c.github.io/mathml-core/#dictionary-based-attributes>
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//!
//! The dictionary in MathML Core appendix A is a few thousand entries keyed by
//! `(Content, Form)`. This module implements the resolution *algorithm*
//! faithfully — attribute precedence, form determination, the
//! infix→postfix→prefix fallback chain, the `Default`/`ForceDefault`
//! categories, and the `0.2777777777777778em` default spacing — together with
//! the subset of entries consumed by the layout algorithms in this crate:
//! fences, separators, common binary operators and relations, arrows, accents,
//! large operators and invisible operators.
//!
//! Entries outside that subset resolve through the specification's `Default`
//! category instead of a hand-written row; extending the table is a pure data
//! change to `OPERATORS`.

use app_units::Au;
use malloc_size_of_derive::MallocSizeOf;
use style::values::computed::MathStyle;

/// The form of an operator, which selects its dictionary row.
///
/// <https://w3c.github.io/mathml-core/#dfn-form>
#[derive(Clone, Copy, Debug, Eq, MallocSizeOf, PartialEq)]
pub(crate) enum OperatorForm {
    Prefix,
    Infix,
    Postfix,
}

/// A resolved MathML length used for operator spacing.
///
/// Dictionary values are expressed in em; presentation attributes accept both
/// `<length>` (including `em` and `px`) and `<percentage>` values, so the
/// resolved value keeps the unit until the operator's font size is known.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub(crate) enum MathSpace {
    /// A multiple of the operator's own font size.
    Em(f32),
    /// An absolute length.
    Px(Au),
}

impl MathSpace {
    /// Resolve this space against the given font size.
    #[inline]
    pub(crate) fn resolve(self, font_size: Au) -> Au {
        match self {
            MathSpace::Em(factor) => font_size.scale_by(factor),
            MathSpace::Px(length) => length,
        }
    }
}

/// Dictionary-derived properties of an `<mo>` element.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub(crate) struct OperatorProperties {
    /// Space before the operator.
    pub lspace: MathSpace,
    /// Space after the operator.
    pub rspace: MathSpace,
    /// Whether the operator stretches along its stretch axis.
    pub stretchy: bool,
    /// Whether the operator is symmetric about the math axis.
    pub symmetric: bool,
    /// Whether the operator is a "large operator" in display style.
    pub largeop: bool,
    /// Whether the operator's limits move to the underscript/overscript
    /// positions in inline (compact) math style.
    pub movablelimits: bool,
    /// Whether the operator is a fence.
    pub fence: bool,
    /// Whether the operator is a separator.
    pub separator: bool,
}

impl OperatorProperties {
    /// The specification's `Default` / `ForceDefault` category, which is also
    /// the fallback for every operator that is not in the dictionary.
    ///
    /// <https://w3c.github.io/mathml-core/#dfn-default>
    pub(crate) const DEFAULT: Self = Self {
        lspace: MathSpace::Em(0.2777777777777778),
        rspace: MathSpace::Em(0.2777777777777778),
        stretchy: false,
        symmetric: false,
        largeop: false,
        movablelimits: false,
        fence: false,
        separator: false,
    };
}

/// A dictionary row for a single `(content, form)` pair.
struct DictionaryEntry {
    content: char,
    form: OperatorForm,
    properties: OperatorProperties,
}

/// 1/18em, the unit used by the operator dictionary spacing values.
const E: f32 = 1.0 / 18.0;

#[allow(clippy::too_many_arguments)]
const fn entry(
    content: char,
    form: OperatorForm,
    lspace: f32,
    rspace: f32,
    stretchy: bool,
    symmetric: bool,
    largeop: bool,
    movablelimits: bool,
    fence: bool,
    separator: bool,
) -> DictionaryEntry {
    DictionaryEntry {
        content,
        form,
        properties: OperatorProperties {
            lspace: MathSpace::Em(lspace),
            rspace: MathSpace::Em(rspace),
            stretchy,
            symmetric,
            largeop,
            movablelimits,
            fence,
            separator,
        },
    }
}

/// The hand-maintained subset of the MathML Core operator dictionary.
static OPERATORS: &[DictionaryEntry] = &[
    // Fences (categories F and G): stretchy, symmetric, no spacing.
    entry(
        '(',
        OperatorForm::Prefix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        ')',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '[',
        OperatorForm::Prefix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        ']',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '{',
        OperatorForm::Prefix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '\u{27E8}',
        OperatorForm::Prefix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '\u{27E9}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '|',
        OperatorForm::Prefix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    entry(
        '|',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        true,
        false,
        false,
        true,
        false,
    ),
    // Binary operators and relations.
    entry(
        '+',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '+',
        OperatorForm::Prefix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2212}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2212}',
        OperatorForm::Prefix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{00B1}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2213}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{00D7}',
        OperatorForm::Infix,
        3.0 * E,
        3.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{00F7}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{22C5}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2219}',
        OperatorForm::Infix,
        4.0 * E,
        4.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '=',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '<',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '>',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2264}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2265}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2260}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2248}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2208}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        ':',
        OperatorForm::Infix,
        0.0,
        3.0 * E,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    // Separators (`,` and `;` are the only separator entries in the
    // specification's special table besides U+2063).
    entry(
        ',',
        OperatorForm::Infix,
        0.0,
        3.0 * E,
        false,
        false,
        false,
        false,
        false,
        true,
    ),
    entry(
        ';',
        OperatorForm::Infix,
        0.0,
        3.0 * E,
        false,
        false,
        false,
        false,
        false,
        true,
    ),
    // Prefix and postfix operators without spacing.
    entry(
        '!',
        OperatorForm::Prefix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '!',
        OperatorForm::Postfix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '/',
        OperatorForm::Infix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{0060}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    // Arrows: stretchy infix operators.
    entry(
        '\u{2192}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2190}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2194}',
        OperatorForm::Infix,
        5.0 * E,
        5.0 * E,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    // Accents: stretchy postfix operators.
    entry(
        '\u{02C6}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{00AF}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{02DC}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{20D7}',
        OperatorForm::Postfix,
        0.0,
        0.0,
        true,
        false,
        false,
        false,
        false,
        false,
    ),
    // Large operators: categories H (integrals, symmetric + largeop) and
    // J (n-ary operators, symmetric + largeop + movablelimits).
    entry(
        '\u{2211}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        true,
        false,
        false,
    ),
    entry(
        '\u{220F}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        true,
        false,
        false,
    ),
    entry(
        '\u{2210}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        true,
        false,
        false,
    ),
    entry(
        '\u{22C3}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        true,
        false,
        false,
    ),
    entry(
        '\u{22C2}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        true,
        false,
        false,
    ),
    entry(
        '\u{222B}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        false,
        false,
        false,
    ),
    entry(
        '\u{222C}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        false,
        false,
        false,
    ),
    entry(
        '\u{222D}',
        OperatorForm::Prefix,
        3.0 * E,
        3.0 * E,
        false,
        true,
        true,
        false,
        false,
        false,
    ),
    // Invisible operators: no spacing.
    entry(
        '\u{2061}',
        OperatorForm::Infix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2062}',
        OperatorForm::Infix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
    entry(
        '\u{2063}',
        OperatorForm::Infix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        true,
    ),
    entry(
        '\u{2064}',
        OperatorForm::Infix,
        0.0,
        0.0,
        false,
        false,
        false,
        false,
        false,
        false,
    ),
];

/// Look up `content` for `form`, then walk the specification's fallback chain.
///
/// <https://w3c.github.io/mathml-core/#dictionary-based-attributes>
fn lookup_in_dictionary(content: char, form: OperatorForm) -> Option<OperatorProperties> {
    if let Some(row) = OPERATORS
        .iter()
        .find(|row| row.content == content && row.form == form)
    {
        return Some(row.properties);
    }

    // Step 4 of the property resolution algorithm: when the category was not
    // explicitly specified, try `infix`, then `postfix`, then `prefix`.
    for fallback in [
        OperatorForm::Infix,
        OperatorForm::Postfix,
        OperatorForm::Prefix,
    ] {
        if let Some(row) = OPERATORS
            .iter()
            .find(|row| row.content == content && row.form == fallback)
        {
            return Some(row.properties);
        }
    }

    None
}

/// Parse the `form` attribute of an `<mo>`.
///
/// <https://w3c.github.io/mathml-core/#dfn-form>
pub(crate) fn parse_form(value: &str) -> Option<OperatorForm> {
    match value.trim().to_ascii_lowercase().as_str() {
        "prefix" => Some(OperatorForm::Prefix),
        "infix" => Some(OperatorForm::Infix),
        "postfix" => Some(OperatorForm::Postfix),
        _ => None,
    }
}

/// Determine the default operator form for a `<mo>` whose `form` attribute is
/// missing or invalid.
///
/// <https://w3c.github.io/mathml-core/#dfn-form>
pub(crate) fn default_form(
    is_first_in_flow_child: bool,
    is_last_in_flow_child: bool,
    in_script_position: bool,
) -> OperatorForm {
    if is_first_in_flow_child && !is_last_in_flow_child {
        return OperatorForm::Prefix;
    }
    if is_last_in_flow_child && !is_first_in_flow_child {
        return OperatorForm::Postfix;
    }
    if in_script_position {
        return OperatorForm::Postfix;
    }
    OperatorForm::Infix
}

/// Parse an `<mo>` boolean attribute.
///
/// <https://w3c.github.io/mathml-core/#dfn-boolean>
pub(crate) fn parse_boolean_attribute(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a `<length-percentage>` presentation attribute for `lspace`/`rspace`.
///
/// Percentages resolve against the dictionary value, as required by
/// <https://w3c.github.io/mathml-core/#dfn-lspace>.
pub(crate) fn parse_space_attribute(value: &str, dictionary_value: MathSpace) -> Option<MathSpace> {
    let value = value.trim();
    if let Some(percentage) = value.strip_suffix('%') {
        let percentage = percentage.trim().parse::<f32>().ok()?;
        let factor = match dictionary_value {
            MathSpace::Em(factor) => factor,
            // A percentage of an absolute dictionary value is not expressible
            // in em; the dictionary values are always em, so this branch is
            // unreachable for real dictionary rows.
            MathSpace::Px(_) => return None,
        };
        return Some(MathSpace::Em(factor * percentage / 100.0));
    }
    if let Some(em) = value.strip_suffix("em") {
        return Some(MathSpace::Em(em.trim().parse::<f32>().ok()?));
    }
    if let Some(px) = value.strip_suffix("px") {
        return Some(MathSpace::Px(Au::from_f32_px(
            px.trim().parse::<f32>().ok()?,
        )));
    }
    if let Some(rem) = value.strip_suffix("rem") {
        return Some(MathSpace::Px(Au::from_f32_px(
            rem.trim().parse::<f32>().ok()? * 16.0,
        )));
    }
    // Unitless zero is accepted by the HTML/CSS parsers for lengths.
    let unitless = value.parse::<f32>().ok()?;
    (unitless == 0.0).then_some(MathSpace::Em(0.0))
}

/// The `<mo>` presentation attributes that participate in the dictionary-based
/// property resolution, as read from the DOM at box construction time.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OperatorAttributes<'a> {
    pub form: Option<&'a str>,
    pub lspace: Option<&'a str>,
    pub rspace: Option<&'a str>,
    pub stretchy: Option<&'a str>,
    pub symmetric: Option<&'a str>,
    pub largeop: Option<&'a str>,
    pub movablelimits: Option<&'a str>,
    pub fence: Option<&'a str>,
    pub separator: Option<&'a str>,
}

/// Resolve the dictionary-derived and attribute-overridden properties of an
/// `<mo>` element.
///
/// `content` is the operator's content when it consists of a single character;
/// empty or multi-character content uses the `Default` category.
///
/// <https://w3c.github.io/mathml-core/#dictionary-based-attributes>
pub(crate) fn resolve(
    content: Option<char>,
    form: OperatorForm,
    attributes: &OperatorAttributes<'_>,
) -> (OperatorForm, OperatorProperties) {
    let form = attributes.form.and_then(parse_form).unwrap_or(form);

    // `ForceDefault` operators: infix `|` (U+007C) and infix `∼` (U+223C).
    // Both use the default spacing and carry no category flags, but their
    // `separator`/`fence` attributes still apply.
    let force_default =
        matches!(content, Some('|') | Some('\u{223C}')) && form == OperatorForm::Infix;

    let mut properties = if force_default {
        OperatorProperties::DEFAULT
    } else {
        content
            .and_then(|content| lookup_in_dictionary(content, form))
            .unwrap_or(OperatorProperties::DEFAULT)
    };

    if let Some(lspace) = attributes
        .lspace
        .and_then(|value| parse_space_attribute(value, properties.lspace))
    {
        properties.lspace = lspace;
    }
    if let Some(rspace) = attributes
        .rspace
        .and_then(|value| parse_space_attribute(value, properties.rspace))
    {
        properties.rspace = rspace;
    }

    let boolean_override = |attribute: Option<&str>, property: &mut bool| {
        if let Some(value) = attribute.and_then(parse_boolean_attribute) {
            *property = value;
        }
    };
    boolean_override(attributes.stretchy, &mut properties.stretchy);
    boolean_override(attributes.symmetric, &mut properties.symmetric);
    boolean_override(attributes.largeop, &mut properties.largeop);
    boolean_override(attributes.movablelimits, &mut properties.movablelimits);
    boolean_override(attributes.fence, &mut properties.fence);
    boolean_override(attributes.separator, &mut properties.separator);

    (form, properties)
}

/// Whether the computed `math-style` is `compact`.
#[inline]
pub(crate) fn math_style_is_compact(math_style: MathStyle) -> bool {
    matches!(math_style, MathStyle::Compact)
}
