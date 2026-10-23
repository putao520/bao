/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csshwb>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSHWBMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::csscolorvalue::CSSColorValue;
use super::cssnumericvalue::CSSNumericValue;
use super::cssunitvalue::CSSUnitValue;
use super::typed_om_serialize::serialize_numeric;
use super::typed_om_values::numeric_type_of;
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSHWB {
    css_color_value: CSSColorValue,
    h: DomRefCell<Dom<CSSNumericValue>>,
    w: DomRefCell<Dom<CSSNumericValue>>,
    b: DomRefCell<Dom<CSSNumericValue>>,
    alpha: DomRefCell<Dom<CSSNumericValue>>,
}

fn syntax_error(message: &str) -> Error {
    Error::Syntax(Some(message.to_string()))
}

fn validate_angle(value: &CSSNumericValue) -> Fallible<()> {
    if numeric_type_of(value).is_ok_and(|numeric_type| numeric_type.matches_angle()) {
        Ok(())
    } else {
        Err(syntax_error("CSSHWB hue must be an <angle>"))
    }
}

fn percent_numberish(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: DoubleOrCSSNumericValue,
) -> Fallible<DomRoot<CSSNumericValue>> {
    match value {
        DoubleOrCSSNumericValue::Double(value) => {
            let unit_value = CSSUnitValue::new(cx, global, *value * 100.0, "percent");
            Ok(DomRoot::from_ref(unit_value.upcast::<CSSNumericValue>()))
        },
        DoubleOrCSSNumericValue::CSSNumericValue(value) => {
            if !numeric_type_of(&value).is_ok_and(|numeric_type| numeric_type.matches_percentage())
            {
                return Err(syntax_error("CSSHWB components must be <percentage>"));
            }
            Ok(value)
        },
    }
}

impl CSSHWB {
    fn new_inherited(
        h: Dom<CSSNumericValue>,
        w: Dom<CSSNumericValue>,
        b: Dom<CSSNumericValue>,
        alpha: Dom<CSSNumericValue>,
    ) -> CSSHWB {
        CSSHWB {
            css_color_value: CSSColorValue::new_inherited(),
            h: DomRefCell::new(h),
            w: DomRefCell::new(w),
            b: DomRefCell::new(b),
            alpha: DomRefCell::new(alpha),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        h: DomRoot<CSSNumericValue>,
        w: DomRoot<CSSNumericValue>,
        b: DomRoot<CSSNumericValue>,
        alpha: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSHWB> {
        reflect_dom_object(
            cx,
            Box::new(CSSHWB::new_inherited(
                Dom::from_ref(&*h),
                Dom::from_ref(&*w),
                Dom::from_ref(&*b),
                Dom::from_ref(&*alpha),
            )),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        h: DomRoot<CSSNumericValue>,
        w: DomRoot<CSSNumericValue>,
        b: DomRoot<CSSNumericValue>,
        alpha: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSHWB> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSHWB::new_inherited(
                Dom::from_ref(&*h),
                Dom::from_ref(&*w),
                Dom::from_ref(&*b),
                Dom::from_ref(&*alpha),
            )),
            global,
            proto,
        )
    }

    pub(crate) fn h_root(&self) -> DomRoot<CSSNumericValue> {
        DomRoot::from_ref(&*self.h.borrow())
    }

    pub(crate) fn h_serialized(&self) -> String {
        serialize_numeric(&self.h.borrow(), false, false)
    }

    pub(crate) fn w_serialized(&self) -> String {
        serialize_numeric(&self.w.borrow(), false, false)
    }

    pub(crate) fn b_serialized(&self) -> String {
        serialize_numeric(&self.b.borrow(), false, false)
    }

    pub(crate) fn alpha_serialized(&self) -> String {
        serialize_numeric(&self.alpha.borrow(), false, false)
    }
}

impl CSSHWBMethods<crate::DomTypeHolder> for CSSHWB {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshwb-csshwb>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        h: &CSSNumericValue,
        w: DoubleOrCSSNumericValue,
        b: DoubleOrCSSNumericValue,
        alpha: DoubleOrCSSNumericValue,
    ) -> Fallible<DomRoot<CSSHWB>> {
        validate_angle(h)?;
        let h = DomRoot::from_ref(h);
        let w = percent_numberish(cx, global, w)?;
        let b = percent_numberish(cx, global, b)?;
        let alpha = percent_numberish(cx, global, alpha)?;
        Ok(CSSHWB::new_with_proto(cx, global, proto, h, w, b, alpha))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshwb-h>
    fn GetH(&self) -> Fallible<DomRoot<CSSNumericValue>> {
        Ok(self.h_root())
    }

    fn SetH(&self, value: &CSSNumericValue) -> ErrorResult {
        validate_angle(value)?;
        *self.h.borrow_mut() = Dom::from_ref(value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshwb-w>
    fn GetW(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(DomRoot::from_ref(
            &*self.w.borrow(),
        )))
    }

    fn SetW(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        *self.w.borrow_mut() = Dom::from_ref(&*percent_numberish(cx, &global, value)?);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshwb-b>
    fn GetB(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(DomRoot::from_ref(
            &*self.b.borrow(),
        )))
    }

    fn SetB(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        *self.b.borrow_mut() = Dom::from_ref(&*percent_numberish(cx, &global, value)?);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshwb-alpha>
    fn GetAlpha(&self) -> Fallible<DoubleOrCSSNumericValue> {
        Ok(DoubleOrCSSNumericValue::CSSNumericValue(DomRoot::from_ref(
            &*self.alpha.borrow(),
        )))
    }

    fn SetAlpha(&self, cx: &mut JSContext, value: DoubleOrCSSNumericValue) -> ErrorResult {
        let global = self.global();
        *self.alpha.borrow_mut() = Dom::from_ref(&*percent_numberish(cx, &global, value)?);
        Ok(())
    }
}
