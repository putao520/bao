/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csstranslate>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSTranslateMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::cssnumericvalue::CSSNumericValue;
use super::csstransformcomponent::CSSTransformComponent;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_values::numeric_type_of;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSTranslate {
    css_transform_component: CSSTransformComponent,
    x: DomRefCell<Dom<CSSNumericValue>>,
    y: DomRefCell<Dom<CSSNumericValue>>,
    z: DomRefCell<Dom<CSSNumericValue>>,
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).unwrap())
}

fn validate_length_percentage(value: &CSSNumericValue) -> ErrorResult {
    if numeric_type_of(value).is_ok_and(|numeric_type| numeric_type.matches_length_percentage()) {
        Ok(())
    } else {
        Err(type_error("Expected a <length-percentage> value"))
    }
}

fn validate_length(value: &CSSNumericValue) -> ErrorResult {
    if numeric_type_of(value).is_ok_and(|numeric_type| numeric_type.matches_length()) {
        Ok(())
    } else {
        Err(type_error("Expected a <length> value"))
    }
}

impl CSSTranslate {
    fn new_inherited(
        x: Dom<CSSNumericValue>,
        y: Dom<CSSNumericValue>,
        z: Dom<CSSNumericValue>,
        is2d: bool,
    ) -> CSSTranslate {
        CSSTranslate {
            css_transform_component: CSSTransformComponent::new_inherited(is2d),
            x: DomRefCell::new(x),
            y: DomRefCell::new(y),
            z: DomRefCell::new(z),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        x: DomRoot<CSSNumericValue>,
        y: DomRoot<CSSNumericValue>,
        z: Option<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSTranslate> {
        Self::new_with_proto(cx, global, None, x, y, z)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: DomRoot<CSSNumericValue>,
        y: DomRoot<CSSNumericValue>,
        z: Option<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSTranslate> {
        let is2d = z.is_none();
        let z = z.unwrap_or_else(|| {
            let unit_value = CSSUnitValue::new(cx, global, 0.0, "px");
            DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
        });
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSTranslate::new_inherited(
                Dom::from_ref(&*x),
                Dom::from_ref(&*y),
                Dom::from_ref(&*z),
                is2d,
            )),
            global,
            proto,
        )
    }

    pub(crate) fn x_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.x.borrow())
    }

    pub(crate) fn y_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.y.borrow())
    }

    pub(crate) fn z_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.z.borrow())
    }
}

impl CSSTranslateMethods<crate::DomTypeHolder> for CSSTranslate {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstranslate-csstranslate>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: &CSSNumericValue,
        y: &CSSNumericValue,
        z: Option<&CSSNumericValue>,
    ) -> Fallible<DomRoot<CSSTranslate>> {
        validate_length_percentage(x)?;
        validate_length_percentage(y)?;
        if let Some(z) = z {
            validate_length(z)?;
        }
        Ok(CSSTranslate::new_with_proto(
            cx,
            global,
            proto,
            DomRoot::from_ref(x),
            DomRoot::from_ref(y),
            z.map(DomRoot::from_ref),
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstranslate-x>
    fn GetX(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.x_root())
    }

    fn SetX(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_length_percentage(value)?;
        *self.x.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstranslate-y>
    fn GetY(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.y_root())
    }

    fn SetY(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_length_percentage(value)?;
        *self.y.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstranslate-z>
    fn GetZ(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.z_root())
    }

    fn SetZ(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_length(value)?;
        *self.z.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }
}
