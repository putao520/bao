/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssscale>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSScaleMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::cssnumericvalue::CSSNumericValue;
use super::csstransformcomponent::CSSTransformComponent;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_values::numeric_type_of;
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSScale {
    css_transform_component: CSSTransformComponent,
    x: DomRefCell<Dom<CSSNumericValue>>,
    y: DomRefCell<Dom<CSSNumericValue>>,
    z: DomRefCell<Dom<CSSNumericValue>>,
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).unwrap())
}

fn number_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DoubleOrCSSNumericValue,
) -> Fallible<DomRoot<CSSNumericValue>> {
    match value {
        DoubleOrCSSNumericValue::Double(value) => {
            let unit_value = CSSUnitValue::new(cx, global, *value, "number");
            Ok(DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>()))
        },
        DoubleOrCSSNumericValue::CSSNumericValue(value) => {
            if !numeric_type_of(&value).is_ok_and(|numeric_type| numeric_type.matches_number()) {
                return Err(type_error("Expected a <number> value"));
            }
            Ok(value)
        },
    }
}

fn number_unit(cx: &mut JSContext, global: &GlobalScope, value: f64) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, value, "number");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

impl CSSScale {
    fn new_inherited(
        x: Dom<CSSNumericValue>,
        y: Dom<CSSNumericValue>,
        z: Dom<CSSNumericValue>,
        is2d: bool,
    ) -> CSSScale {
        CSSScale {
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
    ) -> DomRoot<CSSScale> {
        Self::new_with_proto(cx, global, None, x, y, z)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: DomRoot<CSSNumericValue>,
        y: DomRoot<CSSNumericValue>,
        z: Option<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSScale> {
        let is2d = z.is_none();
        let z = z.unwrap_or_else(|| number_unit(cx, global, 1.0));
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSScale::new_inherited(
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

impl CSSScaleMethods<crate::DomTypeHolder> for CSSScale {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssscale-cssscale>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: DoubleOrCSSNumericValue,
        y: DoubleOrCSSNumericValue,
        z: Option<DoubleOrCSSNumericValue>,
    ) -> Fallible<DomRoot<CSSScale>> {
        let x = number_value(cx, global, x)?;
        let y = number_value(cx, global, y)?;
        let z = match z {
            Some(z) => Some(number_value(cx, global, z)?),
            None => None,
        };
        Ok(CSSScale::new_with_proto(cx, global, proto, x, y, z))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssscale-x>
    fn GetX(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(self.x_root()))
    }

    fn SetX(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        let value = number_value(cx, &global, value)?;
        *self.x.borrow_mut() = Dom::from_ref(&*value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssscale-y>
    fn GetY(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(self.y_root()))
    }

    fn SetY(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        let value = number_value(cx, &global, value)?;
        *self.y.borrow_mut() = Dom::from_ref(&*value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssscale-z>
    fn GetZ(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(self.z_root()))
    }

    fn SetZ(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        let value = number_value(cx, &global, value)?;
        *self.z.borrow_mut() = Dom::from_ref(&*value);
        Ok(())
    }
}
