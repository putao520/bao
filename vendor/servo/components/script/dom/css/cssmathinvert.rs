/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssmathinvert>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSMathInvertMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::cssmathvalue::CSSMathValue;
use super::cssnumericvalue::CSSNumericValue;
use super::typed_om_values::rectify_numberish_rooted;
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSMathInvert {
    css_math_value: CSSMathValue,
    value: Dom<CSSNumericValue>,
}

impl CSSMathInvert {
    fn new_inherited(value: Dom<CSSNumericValue>) -> CSSMathInvert {
        CSSMathInvert {
            css_math_value: CSSMathValue::new_inherited(),
            value,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        value: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSMathInvert> {
        reflect_dom_object(
            cx,
            Box::new(CSSMathInvert::new_inherited(Dom::from_ref(&*value))),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSMathInvert> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSMathInvert::new_inherited(Dom::from_ref(&*value))),
            global,
            proto,
        )
    }

    pub(crate) fn value_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.value)
    }
}

impl CSSMathInvertMethods<crate::DomTypeHolder> for CSSMathInvert {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathinvert-cssmathinvert>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: DoubleOrCSSNumericValue,
    ) -> Fallible<DomRoot<CSSMathInvert>> {
        let value = rectify_numberish_rooted(cx, global, value);
        Ok(CSSMathInvert::new_with_proto(cx, global, proto, value))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathinvert-value>
    fn Value(&self) -> DomRoot<CSSNumericValue> {
        self.value_root()
    }
}
