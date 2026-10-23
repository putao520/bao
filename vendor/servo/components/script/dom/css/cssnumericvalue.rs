/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#numericvalue>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::{
    CSSNumericType, CSSNumericValueMethods,
};
use dom_struct::dom_struct;
use js::context::JSContext;

use super::cssmathsum::CSSMathSum;
use super::cssstylevalue::CSSStyleValue;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_values::{
    ArithmeticOp, arithmetic, numeric_equals_numberish, numeric_type_dictionary,
    parse_numeric_from_string, rectify_numberish_vec, to_sum, to_unit,
};
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSNumericValue {
    css_style_value: CSSStyleValue,
}

impl CSSNumericValue {
    pub(crate) fn new_inherited() -> CSSNumericValue {
        CSSNumericValue {
            css_style_value: CSSStyleValue::new_inherited(String::new(), None),
        }
    }
}

impl CSSNumericValueMethods<crate::DomTypeHolder> for CSSNumericValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-add>
    fn Add(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Add)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-sub>
    fn Sub(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Sub)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-mul>
    fn Mul(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Mul)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-div>
    fn Div(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Div)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-min>
    fn Min(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Min)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-max>
    fn Max(
        &self,
        cx: &mut JSContext,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        let global = self.global();
        let values = rectify_numberish_vec(cx, &global, values);
        arithmetic(cx, &global, self, values, ArithmeticOp::Max)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-equals>
    fn Equals(&self, values: Vec<DoubleOrCSSNumericValue>) -> bool {
        values
            .iter()
            .all(|value| numeric_equals_numberish(self, value))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-to>
    fn To(&self, cx: &mut JSContext, unit: USVString) -> Fallible<DomRoot<CSSUnitValue>> {
        let global = self.global();
        to_unit(cx, &global, self, unit)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-tosum>
    fn ToSum(&self, cx: &mut JSContext, units: Vec<USVString>) -> Fallible<DomRoot<CSSMathSum>> {
        let global = self.global();
        to_sum(cx, &global, self, units)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-type>
    fn Type(&self) -> Fallible<CSSNumericType> {
        numeric_type_dictionary(self)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericvalue-parse>
    fn Parse(
        cx: &mut JSContext,
        global: &GlobalScope,
        css_text: USVString,
    ) -> Fallible<DomRoot<CSSNumericValue>> {
        parse_numeric_from_string(cx, global, &css_text.0)
    }
}
