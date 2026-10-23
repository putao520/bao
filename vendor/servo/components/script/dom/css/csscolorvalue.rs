/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csscolorvalue>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSColorValueMethods;
use crate::dom::bindings::codegen::UnionTypes::CSSColorValueOrCSSStyleValue;
use cssparser::Parser as CssParser;
use dom_struct::dom_struct;
use js::context::JSContext;
use style::color::{AbsoluteColor, ColorSpace};
use style::stylesheets::CssRuleType;
use style::values::specified::Color;
use style_traits::ParsingMode;

use super::csshsl::CSSHSL;
use super::csshwb::CSSHWB;
use super::csskeywordvalue::CSSKeywordValue;
use super::csslab::CSSLab;
use super::csslch::CSSLCH;
use super::cssnumericvalue::CSSNumericValue;
use super::cssoklab::CSSOKLab;
use super::cssoklch::CSSOKLCH;
use super::cssrgb::CSSRGB;
use super::cssstylevalue::CSSStyleValue;
use super::cssunitvalue::CSSUnitValue;
use crate::css::css::ANONYMOUS_CONTENT_URL_DATA;
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValueOrCSSKeywordValueOrString;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

/// A single color channel, stored already rectified to the channel's expected
/// representation (numbers stay numbers, percentages stay percentages, angles
/// become `<angle>` unit values).
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
pub(crate) enum ColorChannel {
    Numeric(Dom<CSSNumericValue>),
    Keyword(Dom<CSSKeywordValue>),
}

impl ColorChannel {
    pub(crate) fn numeric(&self) -> Option<DomRoot<CSSNumericValue>> {
        match self {
            ColorChannel::Numeric(value) => Some(DomRoot::from_ref(&**value)),
            ColorChannel::Keyword(_) => None,
        }
    }

    pub(crate) fn keyword(&self) -> Option<DomRoot<CSSKeywordValue>> {
        match self {
            ColorChannel::Keyword(value) => Some(DomRoot::from_ref(&**value)),
            ColorChannel::Numeric(_) => None,
        }
    }

    /// Serializes the channel for the color class `stringifier`.
    pub(crate) fn serialize(&self) -> String {
        match self {
            ColorChannel::Numeric(value) => {
                super::typed_om_serialize::serialize_numeric(value, false, false)
            },
            ColorChannel::Keyword(value) => {
                super::typed_om_serialize::serialize_keyword_value(value)
            },
        }
    }
}

/// The channel kind, which decides how a `CSSNumberish` argument is rectified
/// and which numeric types it accepts.
#[derive(Clone, Copy)]
pub(crate) enum ChannelKind {
    /// `rgb()` components: doubles become percentages; numbers and percentages
    /// are accepted.
    RgbComponent,
    /// `lab()`/`oklab()` a/b: doubles stay numbers; numbers and percentages are
    /// accepted.
    NumberComponent,
    /// Saturation/lightness/whiteness/blackness/alpha: doubles become
    /// percentages; only percentages are accepted.
    PercentComponent,
    /// Hue: doubles become degrees; only angles are accepted. A string branch
    /// (`undefined` in the union) becomes a `CSSKeywordValue`.
    AngleComponent,
}

#[dom_struct]
pub(crate) struct CSSColorValue {
    css_style_value: CSSStyleValue,
}

impl CSSColorValue {
    pub(crate) fn new_inherited() -> CSSColorValue {
        CSSColorValue {
            css_style_value: CSSStyleValue::new_inherited(String::new(), None),
        }
    }
}

fn numeric_type_matches(value: &CSSNumericValue, kind: ChannelKind) -> bool {
    let Ok(numeric_type) = super::typed_om_values::numeric_type_of(value) else {
        return false;
    };
    match kind {
        ChannelKind::RgbComponent | ChannelKind::NumberComponent => {
            numeric_type.matches_number() || numeric_type.matches_percentage()
        },
        ChannelKind::PercentComponent => numeric_type.matches_percentage(),
        ChannelKind::AngleComponent => numeric_type.matches_angle(),
    }
}

/// <https://drafts.css-houdini.org/css-typed-om-1/#rectify-a-numberish-value>
pub(crate) fn channel_from_union(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    kind: ChannelKind,
) -> Fallible<ColorChannel> {
    match value {
        DoubleOrCSSNumericValueOrCSSKeywordValueOrString::Double(value) => {
            let number = *value;
            let unit_value = match kind {
                ChannelKind::RgbComponent | ChannelKind::PercentComponent => {
                    CSSUnitValue::new(cx, global, number * 100.0, "percent")
                },
                ChannelKind::NumberComponent => CSSUnitValue::new(cx, global, number, "number"),
                ChannelKind::AngleComponent => CSSUnitValue::new(cx, global, number, "deg"),
            };
            Ok(ColorChannel::Numeric(Dom::from_ref(
                unit_value.upcast::<CSSNumericValue>(),
            )))
        },
        DoubleOrCSSNumericValueOrCSSKeywordValueOrString::CSSNumericValue(value) => {
            if !numeric_type_matches(&value, kind) {
                return Err(Error::Syntax(Some(
                    "Invalid type for color channel".to_string(),
                )));
            }
            Ok(ColorChannel::Numeric(Dom::from_ref(&*value)))
        },
        DoubleOrCSSNumericValueOrCSSKeywordValueOrString::CSSKeywordValue(value) => {
            if !matches!(kind, ChannelKind::AngleComponent) {
                return Err(Error::Syntax(Some(
                    "Invalid keyword for color channel".to_string(),
                )));
            }
            Ok(ColorChannel::Keyword(Dom::from_ref(&*value)))
        },
        DoubleOrCSSNumericValueOrCSSKeywordValueOrString::String(value) => {
            if !matches!(kind, ChannelKind::AngleComponent) {
                return Err(Error::Syntax(Some(
                    "Invalid string for color channel".to_string(),
                )));
            }
            let keyword = CSSKeywordValue::new(cx, global, value.str().to_string());
            Ok(ColorChannel::Keyword(Dom::from_ref(&*keyword)))
        },
    }
}

/// Builds the union value returned by a channel getter.
pub(crate) fn channel_to_union(
    channel: &ColorChannel,
) -> DoubleOrCSSNumericValueOrCSSKeywordValueOrString {
    match channel {
        ColorChannel::Numeric(value) => {
            DoubleOrCSSNumericValueOrCSSKeywordValueOrString::CSSNumericValue(DomRoot::from_ref(
                &**value,
            ))
        },
        ColorChannel::Keyword(value) => {
            DoubleOrCSSNumericValueOrCSSKeywordValueOrString::CSSKeywordValue(DomRoot::from_ref(
                &**value,
            ))
        },
    }
}

fn number_channel(cx: &mut JSContext, global: &GlobalScope, value: f64) -> ColorChannel {
    let unit_value = CSSUnitValue::new(cx, global, value, "number");
    ColorChannel::Numeric(Dom::from_ref(unit_value.upcast::<CSSNumericValue>()))
}

fn percent_channel(cx: &mut JSContext, global: &GlobalScope, value: f64) -> ColorChannel {
    let unit_value = CSSUnitValue::new(cx, global, value, "percent");
    ColorChannel::Numeric(Dom::from_ref(unit_value.upcast::<CSSNumericValue>()))
}

fn angle_channel(cx: &mut JSContext, global: &GlobalScope, value: f64) -> ColorChannel {
    let unit_value = CSSUnitValue::new(cx, global, value, "deg");
    ColorChannel::Numeric(Dom::from_ref(unit_value.upcast::<CSSNumericValue>()))
}

fn single_identifier(text: &str) -> Option<String> {
    let mut parser = CssParser::new(text);
    let ident = parser.expect_ident().ok()?.to_string();
    parser.expect_exhausted().ok()?;
    Some(ident)
}

/// The CSS system colors are reified as `CSSKeywordValue`s.
const SYSTEM_COLORS: &[&str] = &[
    "canvas",
    "canvastext",
    "linktext",
    "visitedtext",
    "activetext",
    "buttonface",
    "buttontext",
    "buttonborder",
    "field",
    "fieldtext",
    "highlight",
    "highlighttext",
    "selecteditem",
    "selecteditemtext",
    "mark",
    "marktext",
    "graytext",
    "accentcolor",
    "accentcolortext",
];

fn is_system_color(ident: &str) -> bool {
    let lowercase = ident.to_ascii_lowercase();
    SYSTEM_COLORS.iter().any(|name| *name == lowercase)
}

fn parse_absolute_color(text: &str) -> Result<AbsoluteColor, ()> {
    let context = crate::css::css::parser_context_for_anonymous_content(
        CssRuleType::Style,
        ParsingMode::DEFAULT,
        &ANONYMOUS_CONTENT_URL_DATA,
    );
    let mut input = CssParser::new(text);
    // `parse_and_compute` already parses the entire input (and swallows the
    // `ParseError` into `()`), so it is called directly instead of through
    // `Parser::parse_entirely`.
    let color = Color::parse_and_compute(&context, &mut input, None)?;
    Ok(color.resolve_to_absolute(&AbsoluteColor::BLACK))
}

fn build_color(
    cx: &mut JSContext,
    global: &GlobalScope,
    absolute: &AbsoluteColor,
) -> Option<DomRoot<CSSColorValue>> {
    let components = absolute.components.to_array();
    let alpha = (absolute.alpha as f64) * 100.0;
    match absolute.color_space {
        ColorSpace::Srgb => {
            let r = number_channel(cx, global, (components[0] as f64) * 255.0);
            let g = number_channel(cx, global, (components[1] as f64) * 255.0);
            let b = number_channel(cx, global, (components[2] as f64) * 255.0);
            let a = percent_channel(cx, global, alpha);
            let rgb = CSSRGB::new(cx, global, r, g, b, a);
            Some(DomRoot::from_ref(rgb.upcast::<CSSColorValue>()))
        },
        ColorSpace::Hsl => {
            let h = angle_channel(cx, global, components[0] as f64);
            let s = percent_channel(cx, global, components[1] as f64);
            let l = percent_channel(cx, global, components[2] as f64);
            let a = percent_channel(cx, global, alpha);
            let hsl = CSSHSL::new(cx, global, h, s, l, a);
            Some(DomRoot::from_ref(hsl.upcast::<CSSColorValue>()))
        },
        ColorSpace::Hwb => {
            let h = CSSUnitValue::new(cx, global, components[0] as f64, "deg");
            let w = CSSUnitValue::new(cx, global, components[1] as f64, "percent");
            let b = CSSUnitValue::new(cx, global, components[2] as f64, "percent");
            let a = CSSUnitValue::new(cx, global, alpha, "percent");
            let hwb = CSSHWB::new(
                cx,
                global,
                DomRoot::from_ref(h.upcast::<CSSNumericValue>()),
                DomRoot::from_ref(w.upcast::<CSSNumericValue>()),
                DomRoot::from_ref(b.upcast::<CSSNumericValue>()),
                DomRoot::from_ref(a.upcast::<CSSNumericValue>()),
            );
            Some(DomRoot::from_ref(hwb.upcast::<CSSColorValue>()))
        },
        ColorSpace::Lab => {
            let l = percent_channel(cx, global, components[0] as f64);
            let a = number_channel(cx, global, components[1] as f64);
            let b = number_channel(cx, global, components[2] as f64);
            let alpha_channel = percent_channel(cx, global, alpha);
            let lab = CSSLab::new(cx, global, l, a, b, alpha_channel);
            Some(DomRoot::from_ref(lab.upcast::<CSSColorValue>()))
        },
        ColorSpace::Lch => {
            let l = percent_channel(cx, global, components[0] as f64);
            let c = percent_channel(cx, global, (components[1] as f64) / 150.0 * 100.0);
            let h = angle_channel(cx, global, components[2] as f64);
            let alpha_channel = percent_channel(cx, global, alpha);
            let lch = CSSLCH::new(cx, global, l, c, h, alpha_channel);
            Some(DomRoot::from_ref(lch.upcast::<CSSColorValue>()))
        },
        ColorSpace::Oklab => {
            let l = percent_channel(cx, global, (components[0] as f64) * 100.0);
            let a = number_channel(cx, global, components[1] as f64);
            let b = number_channel(cx, global, components[2] as f64);
            let alpha_channel = percent_channel(cx, global, alpha);
            let oklab = CSSOKLab::new(cx, global, l, a, b, alpha_channel);
            Some(DomRoot::from_ref(oklab.upcast::<CSSColorValue>()))
        },
        ColorSpace::Oklch => {
            let l = percent_channel(cx, global, (components[0] as f64) * 100.0);
            let c = percent_channel(cx, global, (components[1] as f64) / 0.4 * 100.0);
            let h = angle_channel(cx, global, components[2] as f64);
            let alpha_channel = percent_channel(cx, global, alpha);
            let oklch = CSSOKLCH::new(cx, global, l, c, h, alpha_channel);
            Some(DomRoot::from_ref(oklch.upcast::<CSSColorValue>()))
        },
        _ => None,
    }
}

impl CSSColorValueMethods<crate::DomTypeHolder> for CSSColorValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csscolorvalue-parse>
    fn Parse(
        cx: &mut JSContext,
        global: &GlobalScope,
        css_text: USVString,
    ) -> Fallible<CSSColorValueOrCSSStyleValue> {
        let text = css_text.0;
        if let Some(ident) = single_identifier(&text) {
            if is_system_color(&ident) {
                let keyword = CSSKeywordValue::new(cx, global, ident.to_ascii_lowercase());
                return Ok(CSSColorValueOrCSSStyleValue::CSSStyleValue(
                    DomRoot::from_ref(keyword.upcast::<CSSStyleValue>()),
                ));
            }
        }
        let absolute = parse_absolute_color(&text)
            .map_err(|_| Error::Syntax(Some("Invalid color value".to_string())))?;
        let color = build_color(cx, global, &absolute)
            .ok_or_else(|| Error::Syntax(Some("Unsupported color space".to_string())))?;
        Ok(CSSColorValueOrCSSStyleValue::CSSColorValue(color))
    }
}
