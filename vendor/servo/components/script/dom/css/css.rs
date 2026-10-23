/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use cssparser::{Parser, serialize_identifier};
use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::codegen::GenericBindings::CSSBinding::PropertyDefinition;
use script_bindings::num::Finite;
use script_bindings::reflector::Reflector;
use style::stylesheets::supports_rule::{Declaration, parse_condition_or_declaration};
use style::stylesheets::{CssRuleType, UrlExtraData};
use style::stylist::RegisterCustomPropertyResult;
use style_traits::ParsingMode;

use super::cssunitvalue::CSSUnitValue;
use crate::css::css::parser_context_for_anonymous_content;
use crate::dom::bindings::codegen::Bindings::CSSBinding::CSSMethods;
use crate::dom::bindings::codegen::Bindings::WindowBinding::Window_Binding::WindowMethods;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;
use crate::dom::worklet::Worklet;

/// The numeric factory functions of the CSS namespace
/// (<https://drafts.css-houdini.org/css-typed-om-1/#numeric-factory>).
macro_rules! numeric_factories {
    ($($method:ident => $unit:literal),* $(,)?) => {
        $(
            fn $method(
                cx: &mut JSContext,
                global: &GlobalScope,
                value: Finite<f64>,
            ) -> DomRoot<CSSUnitValue> {
                CSSUnitValue::new(cx, global, *value, $unit)
            }
        )*
    };
}

#[dom_struct]
#[expect(clippy::upper_case_acronyms)]
pub(crate) struct CSS {
    reflector_: Reflector,
}

impl CSSMethods<crate::DomTypeHolder> for CSS {
    /// <https://drafts.csswg.org/cssom/#the-css.escape()-method>
    fn Escape(_: &GlobalScope, ident: DOMString) -> Fallible<DOMString> {
        let mut escaped = String::new();
        serialize_identifier(&ident.str(), &mut escaped).unwrap();
        Ok(DOMString::from(escaped))
    }

    /// <https://drafts.csswg.org/css-conditional/#dom-css-supports>
    fn Supports(global: &GlobalScope, property: DOMString, value: DOMString) -> bool {
        let mut decl = String::new();
        serialize_identifier(&property.str(), &mut decl).unwrap();
        decl.push_str(": ");
        decl.push_str(&value.str());
        let decl = Declaration(decl);
        let url_data = UrlExtraData(global.get_url().get_arc());
        let mut context = parser_context_for_anonymous_content(
            CssRuleType::Style,
            ParsingMode::DEFAULT,
            &url_data,
        );
        decl.eval(&mut context)
    }

    /// <https://drafts.csswg.org/css-conditional/#dom-css-supports>
    fn Supports_(global: &GlobalScope, condition: DOMString) -> bool {
        let condition = condition.str();
        let mut input = Parser::new(&condition);
        let cond = match parse_condition_or_declaration(&mut input) {
            Ok(c) => c,
            Err(..) => return false,
        };

        let url_data = UrlExtraData(global.get_url().get_arc());
        let mut context = parser_context_for_anonymous_content(
            CssRuleType::Style,
            ParsingMode::DEFAULT,
            &url_data,
        );
        cond.eval(&mut context)
    }

    /// <https://drafts.css-houdini.org/css-paint-api-1/#paint-worklet>
    fn PaintWorklet(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<Worklet> {
        global.as_window().paint_worklet(cx)
    }

    /// <https://drafts.css-houdini.org/css-properties-values-api/#the-registerproperty-function>
    fn RegisterProperty(
        global: &GlobalScope,
        property_definition: &PropertyDefinition,
    ) -> Fallible<()> {
        use RegisterCustomPropertyResult::*;
        let window = global.as_window();
        let result = window.layout_mut().stylist_mut().register_custom_property(
            &UrlExtraData(window.get_url().get_arc()),
            &property_definition.name.str(),
            &property_definition.syntax.str(),
            property_definition.inherits,
            property_definition
                .initialValue
                .as_ref()
                .map(|value| value.str())
                .as_deref(),
        );
        Err(match result {
            SuccessfullyRegistered => return Ok(()),
            InvalidName |
            InvalidSyntax |
            InvalidInitialValue |
            NoInitialValue |
            InitialValueNotComputationallyIndependent => Error::Syntax(Some(
                "CSS property value is not computationally independent".into(),
            )),
            AlreadyRegistered => {
                Error::InvalidModification(Some("CSS property is already registered".into()))
            },
        })
    }

    numeric_factories! {
        Number => "number",
        Percent => "percent",
        Cap => "cap",
        Ch => "ch",
        Em => "em",
        Ex => "ex",
        Ic => "ic",
        Lh => "lh",
        Rcap => "rcap",
        Rch => "rch",
        Rem => "rem",
        Rex => "rex",
        Ric => "ric",
        Rlh => "rlh",
        Vw => "vw",
        Vh => "vh",
        Vi => "vi",
        Vb => "vb",
        Vmin => "vmin",
        Vmax => "vmax",
        Svw => "svw",
        Svh => "svh",
        Svi => "svi",
        Svb => "svb",
        Svmin => "svmin",
        Svmax => "svmax",
        Lvw => "lvw",
        Lvh => "lvh",
        Lvi => "lvi",
        Lvb => "lvb",
        Lvmin => "lvmin",
        Lvmax => "lvmax",
        Dvw => "dvw",
        Dvh => "dvh",
        Dvi => "dvi",
        Dvb => "dvb",
        Dvmin => "dvmin",
        Dvmax => "dvmax",
        Cqw => "cqw",
        Cqh => "cqh",
        Cqi => "cqi",
        Cqb => "cqb",
        Cqmin => "cqmin",
        Cqmax => "cqmax",
        Cm => "cm",
        Mm => "mm",
        Q => "q",
        In => "in",
        Pt => "pt",
        Pc => "pc",
        Px => "px",
        Deg => "deg",
        Grad => "grad",
        Rad => "rad",
        Turn => "turn",
        S => "s",
        Ms => "ms",
        Hz => "hz",
        KHz => "khz",
        Dpi => "dpi",
        Dpcm => "dpcm",
        Dppx => "dppx",
        Fr => "fr",
    }
}
