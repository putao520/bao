/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssrotate>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSRotateMethods;
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
pub(crate) struct CSSRotate {
    css_transform_component: CSSTransformComponent,
    x: DomRefCell<Dom<CSSNumericValue>>,
    y: DomRefCell<Dom<CSSNumericValue>>,
    z: DomRefCell<Dom<CSSNumericValue>>,
    angle: DomRefCell<Dom<CSSNumericValue>>,
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

/// Rectifies a `CSSNumberish` into a `CSSNumericValue` without type
/// validation. Used by the non-throwing `CSSRotate` x/y/z setters; the
/// `<number>` check belongs to `toMatrix()` per the Typed OM spec.
fn rectify_numberish(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DoubleOrCSSNumericValue,
) -> DomRoot<CSSNumericValue> {
    match value {
        DoubleOrCSSNumericValue::Double(value) => axis_value(cx, global, *value),
        DoubleOrCSSNumericValue::CSSNumericValue(value) => value,
    }
}

fn axis_value(cx: &mut JSContext, global: &GlobalScope, value: f64) -> DomRoot<CSSNumericValue> {
    let unit_value = CSSUnitValue::new(cx, global, value, "number");
    DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>())
}

impl CSSRotate {
    fn new_inherited(
        x: Dom<CSSNumericValue>,
        y: Dom<CSSNumericValue>,
        z: Dom<CSSNumericValue>,
        angle: Dom<CSSNumericValue>,
        is2d: bool,
    ) -> CSSRotate {
        CSSRotate {
            css_transform_component: CSSTransformComponent::new_inherited(is2d),
            x: DomRefCell::new(x),
            y: DomRefCell::new(y),
            z: DomRefCell::new(z),
            angle: DomRefCell::new(angle),
        }
    }

    pub(crate) fn new_from_angle(
        cx: &mut JSContext,
        global: &GlobalScope,
        angle: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSRotate> {
        let x = axis_value(cx, global, 0.0);
        let y = axis_value(cx, global, 0.0);
        let z = axis_value(cx, global, 1.0);
        Self::new_with_proto(cx, global, None, x, y, z, angle, true)
    }

    pub(crate) fn new_from_axis(
        cx: &mut JSContext,
        global: &GlobalScope,
        x: DomRoot<CSSNumericValue>,
        y: DomRoot<CSSNumericValue>,
        z: DomRoot<CSSNumericValue>,
        angle: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSRotate> {
        Self::new_with_proto(cx, global, None, x, y, z, angle, false)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: DomRoot<CSSNumericValue>,
        y: DomRoot<CSSNumericValue>,
        z: DomRoot<CSSNumericValue>,
        angle: DomRoot<CSSNumericValue>,
        is2d: bool,
    ) -> DomRoot<CSSRotate> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSRotate::new_inherited(
                Dom::from_ref(&*x),
                Dom::from_ref(&*y),
                Dom::from_ref(&*z),
                Dom::from_ref(&*angle),
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

    pub(crate) fn angle_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.angle.borrow())
    }
}

impl CSSRotateMethods<crate::DomTypeHolder> for CSSRotate {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-cssrotate>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        angle: &CSSNumericValue,
    ) -> Fallible<DomRoot<CSSRotate>> {
        validate_angle(angle)?;
        let x = axis_value(cx, global, 0.0);
        let y = axis_value(cx, global, 0.0);
        let z = axis_value(cx, global, 1.0);
        Ok(CSSRotate::new_with_proto(
            cx,
            global,
            proto,
            x,
            y,
            z,
            DomRoot::from_ref(angle),
            true,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-cssrotate-x-y-z-angle>
    fn Constructor_(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        x: DoubleOrCSSNumericValue,
        y: DoubleOrCSSNumericValue,
        z: DoubleOrCSSNumericValue,
        angle: &CSSNumericValue,
    ) -> Fallible<DomRoot<CSSRotate>> {
        validate_angle(angle)?;
        let x = number_value(cx, global, x)?;
        let y = number_value(cx, global, y)?;
        let z = number_value(cx, global, z)?;
        Ok(CSSRotate::new_with_proto(
            cx,
            global,
            proto,
            x,
            y,
            z,
            DomRoot::from_ref(angle),
            false,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-x>
    fn X(&self) -> DoubleOrCSSNumericValue {
        DoubleOrCSSNumericValue::CSSNumericValue(self.x_root())
    }

    fn SetX(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) {
        let global = self.global();
        let value = rectify_numberish(cx, &global, value);
        *self.x.borrow_mut() = Dom::from_ref(&*value);
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-y>
    fn Y(&self) -> DoubleOrCSSNumericValue {
        DoubleOrCSSNumericValue::CSSNumericValue(self.y_root())
    }

    fn SetY(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) {
        let global = self.global();
        let value = rectify_numberish(cx, &global, value);
        *self.y.borrow_mut() = Dom::from_ref(&*value);
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-z>
    fn Z(&self) -> DoubleOrCSSNumericValue {
        DoubleOrCSSNumericValue::CSSNumericValue(self.z_root())
    }

    fn SetZ(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) {
        let global = self.global();
        let value = rectify_numberish(cx, &global, value);
        *self.z.borrow_mut() = Dom::from_ref(&*value);
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrotate-angle>
    fn GetAngle(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.angle_root())
    }

    fn SetAngle(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.angle.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }
}
