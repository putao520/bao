/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssskew>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSSkewMethods;
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
pub(crate) struct CSSSkew {
    css_transform_component: CSSTransformComponent,
    ax: DomRefCell<Dom<CSSNumericValue>>,
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

impl CSSSkew {
    fn new_inherited(ax: Dom<CSSNumericValue>, ay: Dom<CSSNumericValue>) -> CSSSkew {
        CSSSkew {
            css_transform_component: CSSTransformComponent::new_inherited(true),
            ax: DomRefCell::new(ax),
            ay: DomRefCell::new(ay),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        ax: DomRoot<CSSNumericValue>,
        ay: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkew> {
        Self::new_with_proto(cx, global, None, ax, ay)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ax: DomRoot<CSSNumericValue>,
        ay: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkew> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSSkew::new_inherited(
                Dom::from_ref(&*ax),
                Dom::from_ref(&*ay),
            )),
            global,
            proto,
        )
    }

    pub(crate) fn ax_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.ax.borrow())
    }

    pub(crate) fn ay_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.ay.borrow())
    }
}

impl CSSSkewMethods<crate::DomTypeHolder> for CSSSkew {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskew-cssskew>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ax: &CSSNumericValue,
        ay: &CSSNumericValue,
    ) -> Fallible<DomRoot<CSSSkew>> {
        validate_angle(ax)?;
        validate_angle(ay)?;
        Ok(CSSSkew::new_with_proto(
            cx,
            global,
            proto,
            DomRoot::from_ref(ax),
            DomRoot::from_ref(ay),
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskew-ax>
    fn GetAx(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.ax_root())
    }

    fn SetAx(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.ax.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskew-ay>
    fn GetAy(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.ay_root())
    }

    fn SetAy(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.ay.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }
}
