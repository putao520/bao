/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssmathclamp>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSMathClampMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::cssmathvalue::CSSMathValue;
use super::cssnumericvalue::CSSNumericValue;
use super::typed_om_values::{rectify_numberish_rooted, validate_addable};
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSMathClamp {
    css_math_value: CSSMathValue,
    lower: Dom<CSSNumericValue>,
    value: Dom<CSSNumericValue>,
    upper: Dom<CSSNumericValue>,
}

impl CSSMathClamp {
    fn new_inherited(
        lower: Dom<CSSNumericValue>,
        value: Dom<CSSNumericValue>,
        upper: Dom<CSSNumericValue>,
    ) -> CSSMathClamp {
        CSSMathClamp {
            css_math_value: CSSMathValue::new_inherited(),
            lower,
            value,
            upper,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        lower: DomRoot<CSSNumericValue>,
        value: DomRoot<CSSNumericValue>,
        upper: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSMathClamp> {
        reflect_dom_object(
            cx,
            Box::new(CSSMathClamp::new_inherited(
                Dom::from_ref(&*lower),
                Dom::from_ref(&*value),
                Dom::from_ref(&*upper),
            )),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        lower: DomRoot<CSSNumericValue>,
        value: DomRoot<CSSNumericValue>,
        upper: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSMathClamp> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSMathClamp::new_inherited(
                Dom::from_ref(&*lower),
                Dom::from_ref(&*value),
                Dom::from_ref(&*upper),
            )),
            global,
            proto,
        )
    }

    pub(crate) fn lower_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.lower)
    }

    pub(crate) fn value_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.value)
    }

    pub(crate) fn upper_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.upper)
    }
}

impl CSSMathClampMethods<crate::DomTypeHolder> for CSSMathClamp {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathclamp-cssmathclamp>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        lower: DoubleOrCSSNumericValue,
        value: DoubleOrCSSNumericValue,
        upper: DoubleOrCSSNumericValue,
    ) -> Fallible<DomRoot<CSSMathClamp>> {
        let lower = rectify_numberish_rooted(cx, global, lower);
        let value = rectify_numberish_rooted(cx, global, value);
        let upper = rectify_numberish_rooted(cx, global, upper);
        validate_addable(&[lower.clone(), value.clone(), upper.clone()])?;
        Ok(CSSMathClamp::new_with_proto(
            cx, global, proto, lower, value, upper,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathclamp-lower>
    fn Lower(&self) -> DomRoot<CSSNumericValue> {
        self.lower_root()
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathclamp-value>
    fn Value(&self) -> DomRoot<CSSNumericValue> {
        self.value_root()
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathclamp-upper>
    fn Upper(&self) -> DomRoot<CSSNumericValue> {
        self.upper_root()
    }
}
