/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssskewx>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSSkewXMethods;
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
pub(crate) struct CSSSkewX {
    css_transform_component: CSSTransformComponent,
    ax: DomRefCell<Dom<CSSNumericValue>>,
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

impl CSSSkewX {
    fn new_inherited(ax: Dom<CSSNumericValue>) -> CSSSkewX {
        CSSSkewX {
            css_transform_component: CSSTransformComponent::new_inherited(true),
            ax: DomRefCell::new(ax),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        ax: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkewX> {
        Self::new_with_proto(cx, global, None, ax)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ax: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSSkewX> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSSkewX::new_inherited(Dom::from_ref(&*ax))),
            global,
            proto,
        )
    }

    pub(crate) fn ax_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.ax.borrow())
    }
}

impl CSSSkewXMethods<crate::DomTypeHolder> for CSSSkewX {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskewx-cssskewx>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        ax: &CSSNumericValue,
    ) -> Fallible<DomRoot<CSSSkewX>> {
        validate_angle(ax)?;
        Ok(CSSSkewX::new_with_proto(
            cx,
            global,
            proto,
            DomRoot::from_ref(ax),
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssskewx-ax>
    fn GetAx(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.ax_root())
    }

    fn SetAx(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.ax.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }
}
