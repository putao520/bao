/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssskewy>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSSkewYMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::cssnumericvalue::CSSNumericValue;
use super::csstransformcomponent::CSSTransformComponent;
use super::typed_om_values::numeric_type_of;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSSkewY {
    css_transform_component: CSSTransformComponent,
    ay: DomRefCell<Dom<CSSNumericValue>>,
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).unwrap())
}

fn validate_angle(value: &CSSNumericValue) -> ErrorResult {
    if numeric_type_of(value).is_ok_and(|numeric_type| numeric_type.matches_angle()) {
        Ok(())
    } else {
        Err(type_error("Expected an <angle> value"))
    }
}

impl CSSSkewY {
    fn new_inherited(ay: Dom<CSSNumericValue>) -> CSSSkewY {
        CSSSkewY {
            css_transform_component: CSSTransformComponent::new_inherited(true),
            ay: DomRefCell::new(ay),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        ay: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkewY> {
        Self::new_with_proto(cx, global, None, ay)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ay: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkewY> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSSkewY::new_inherited(Dom::from_ref(&*ay))),
            global,
            proto,
        )
    }

    pub(crate) fn ay_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.ay.borrow())
    }
}

impl CSSSkewYMethods<crate::DomTypeHolder> for CSSSkewY {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskewy-cssskewy>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ay: &CSSNumericValue,
    ) -> Fallible<DomRoot<CSSSkewY>> {
        validate_angle(ay)?;
        Ok(CSSSkewY::new_with_proto(
            cx,
            global,
            proto,
            DomRoot::from_ref(ay),
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskewy-ay>
    fn GetAy(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.ay_root())
    }

    fn SetAy(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.ay.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }
}
