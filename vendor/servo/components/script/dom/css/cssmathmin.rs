/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssmathmin>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSMathMinMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::cssmathvalue::CSSMathValue;
use super::cssnumericarray::CSSNumericArray;
use super::cssnumericvalue::CSSNumericValue;
use super::typed_om_values::{rectify_numberish_vec, validate_addable};
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSMathMin {
    css_math_value: CSSMathValue,
    values: DomRefCell<Vec<Dom<CSSNumericValue>>>,
}

impl CSSMathMin {
    fn new_inherited(values: Vec<Dom<CSSNumericValue>>) -> CSSMathMin {
        CSSMathMin {
            css_math_value: CSSMathValue::new_inherited(),
            values: DomRefCell::new(values),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        values: Vec<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSMathMin> {
        let values = values.iter().map(|value| Dom::from_ref(&**value)).collect();
        reflect_dom_object(cx, Box::new(CSSMathMin::new_inherited(values)), global)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        values: Vec<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSMathMin> {
        let values = values.iter().map(|value| Dom::from_ref(&**value)).collect();
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSMathMin::new_inherited(values)),
            global,
            proto,
        )
    }

    pub(crate) fn values(&self) -> Vec<DomRoot<CSSNumericValue>> {
        self.values
            .borrow()
            .iter()
            .map(|value| DomRoot::from_ref(&**value))
            .collect()
    }
}

impl CSSMathMinMethods<crate::DomTypeHolder> for CSSMathMin {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathmin-cssmathmin>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        values: Vec<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSMathMin>> {
        let values = rectify_numberish_vec(cx, global, values);
        if values.is_empty() {
            return Err(Error::Syntax(Some(
                "CSSMathMin requires at least one value".into(),
            )));
        }
        validate_addable(&values)?;
        Ok(CSSMathMin::new_with_proto(cx, global, proto, values))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathmin-values>
    fn Values(&self, cx: &mut JSContext) -> DomRoot<CSSNumericArray> {
        let global = self.global();
        CSSNumericArray::new(cx, &global, self.values())
    }
}
