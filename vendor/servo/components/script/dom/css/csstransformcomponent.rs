/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csstransformcomponent>

use std::cell::Cell;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSTransformComponentMethods;
use dom_struct::dom_struct;
use euclid::Angle;
use euclid::default::Transform3D;
use js::context::JSContext;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use script_bindings::str::DOMString;

use super::cssmatrixcomponent::CSSMatrixComponent;
use super::cssnumericvalue::CSSNumericValue;
use super::cssperspective::CSSPerspective;
use super::cssrotate::CSSRotate;
use super::cssscale::CSSScale;
use super::cssskew::CSSSkew;
use super::cssskewx::CSSSkewX;
use super::cssskewy::CSSSkewY;
use super::csstranslate::CSSTranslate;
use super::typed_om_serialize::serialize_transform_component;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::dommatrix::DOMMatrix;

#[dom_struct]
pub(crate) struct CSSTransformComponent {
    reflector: Reflector,
    is2d: Cell<bool>,
}

fn type_error(message: &str) -> Error {
    Error::Type(std::ffi::CString::new(message).unwrap())
}

fn resolve(numeric: &CSSNumericValue, unit: &str) -> Fallible<f64> {
    super::typed_om_serialize::numeric_to_absolute(numeric, unit)
        .map_err(|_| type_error("Cannot resolve value to an absolute unit"))
}

fn normalize_axis(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        (0.0, 0.0, 0.0)
    } else {
        (x / length, y / length, z / length)
    }
}

impl CSSTransformComponent {
    pub(crate) fn new_inherited(is2d: bool) -> CSSTransformComponent {
        CSSTransformComponent {
            reflector: Reflector::new(),
            is2d: Cell::new(is2d),
        }
    }

    pub(crate) fn is_2d(&self) -> bool {
        self.is2d.get()
    }

    pub(crate) fn set_is_2d(&self, value: bool) {
        // Skew/skewX/skewY/perspective are inherently 2D/3D and ignore writes
        // (<https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformcomponent-is2d>).
        if self.is::<CSSSkew>() || self.is::<CSSSkewX>() || self.is::<CSSSkewY>() {
            return;
        }
        if self.is::<CSSPerspective>() {
            return;
        }
        self.is2d.set(value);
    }

    pub(crate) fn to_matrix_dispatch(&self, cx: &mut JSContext) -> Fallible<DomRoot<DOMMatrix>> {
        let global = self.global();
        let global = &global;
        if let Some(translate) = self.downcast::<CSSTranslate>() {
            let x = resolve(&translate.x_root(), "px")?;
            let y = resolve(&translate.y_root(), "px")?;
            if self.is_2d() {
                return Ok(DOMMatrix::new(
                    cx,
                    global,
                    true,
                    Transform3D::translation(x, y, 0.0),
                ));
            }
            let z = resolve(&translate.z_root(), "px")?;
            return Ok(DOMMatrix::new(
                cx,
                global,
                false,
                Transform3D::translation(x, y, z),
            ));
        }
        if let Some(rotate) = self.downcast::<CSSRotate>() {
            let angle = resolve(&rotate.angle_root(), "deg")?;
            if self.is_2d() {
                return Ok(DOMMatrix::new(
                    cx,
                    global,
                    true,
                    Transform3D::rotation(0.0, 0.0, 1.0, Angle::degrees(angle)),
                ));
            }
            let x = resolve(&rotate.x_root(), "number")?;
            let y = resolve(&rotate.y_root(), "number")?;
            let z = resolve(&rotate.z_root(), "number")?;
            let (x, y, z) = normalize_axis(x, y, z);
            return Ok(DOMMatrix::new(
                cx,
                global,
                false,
                Transform3D::rotation(x, y, z, Angle::degrees(angle)),
            ));
        }
        if let Some(scale) = self.downcast::<CSSScale>() {
            let x = resolve(&scale.x_root(), "number")?;
            let y = resolve(&scale.y_root(), "number")?;
            if self.is_2d() {
                return Ok(DOMMatrix::new(
                    cx,
                    global,
                    true,
                    Transform3D::scale(x, y, 1.0),
                ));
            }
            let z = resolve(&scale.z_root(), "number")?;
            return Ok(DOMMatrix::new(
                cx,
                global,
                false,
                Transform3D::scale(x, y, z),
            ));
        }
        if let Some(skew) = self.downcast::<CSSSkew>() {
            let ax = resolve(&skew.ax_root(), "deg")?;
            let ay = resolve(&skew.ay_root(), "deg")?;
            return Ok(DOMMatrix::new(
                cx,
                global,
                true,
                Transform3D::skew(Angle::degrees(ax), Angle::degrees(ay)),
            ));
        }
        if let Some(skew_x) = self.downcast::<CSSSkewX>() {
            let ax = resolve(&skew_x.ax_root(), "deg")?;
            return Ok(DOMMatrix::new(
                cx,
                global,
                true,
                Transform3D::skew(Angle::degrees(ax), Angle::degrees(0.0)),
            ));
        }
        if let Some(skew_y) = self.downcast::<CSSSkewY>() {
            let ay = resolve(&skew_y.ay_root(), "deg")?;
            return Ok(DOMMatrix::new(
                cx,
                global,
                true,
                Transform3D::skew(Angle::degrees(0.0), Angle::degrees(ay)),
            ));
        }
        if let Some(perspective) = self.downcast::<CSSPerspective>() {
            let numeric = perspective
                .length_as_numeric()
                .ok_or_else(|| type_error("perspective(none) has no matrix"))?;
            let length = resolve(&numeric, "px")?;
            // Spec matrix: m34 = -1/d, matching `Transform3D::perspective`
            // (<https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformcomponent-tomatrix>).
            return Ok(DOMMatrix::new(
                cx,
                global,
                false,
                Transform3D::perspective(length),
            ));
        }
        if let Some(matrix_component) = self.downcast::<CSSMatrixComponent>() {
            let matrix_root = matrix_component.matrix_root();
            let readonly = matrix_root.upcast::<crate::dom::dommatrixreadonly::DOMMatrixReadOnly>();
            let is_2d = readonly.is2D();
            let matrix = *readonly.matrix();
            if self.is_2d() && !is_2d {
                let flattened = Transform3D::new(
                    matrix.m11, matrix.m12, 0.0, 0.0, matrix.m21, matrix.m22, 0.0, 0.0, 0.0, 0.0,
                    1.0, 0.0, matrix.m41, matrix.m42, 0.0, 1.0,
                );
                return Ok(DOMMatrix::new(cx, global, true, flattened));
            }
            return Ok(DOMMatrix::new(cx, global, is_2d, matrix));
        }
        Err(type_error("Unsupported CSSTransformComponent"))
    }
}

impl CSSTransformComponentMethods<crate::DomTypeHolder> for CSSTransformComponent {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#CSSTransformComponent-stringification-behavior>
    fn Stringifier(&self) -> DOMString {
        DOMString::from(serialize_transform_component(self))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformcomponent-is2d>
    fn Is2D(&self) -> bool {
        self.is_2d()
    }

    fn SetIs2D(&self, value: bool) {
        self.set_is_2d(value);
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformcomponent-tomatrix>
    fn ToMatrix(&self, cx: &mut JSContext) -> Fallible<DomRoot<DOMMatrix>> {
        self.to_matrix_dispatch(cx)
    }
}
