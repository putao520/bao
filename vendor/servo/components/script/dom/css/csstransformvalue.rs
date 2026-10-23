/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#transformvalue>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSTransformValueMethods;
use dom_struct::dom_struct;
use euclid::default::Transform3D;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::cssstylevalue::CSSStyleValue;
use super::csstransformcomponent::CSSTransformComponent;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::dommatrix::DOMMatrix;
use crate::dom::dommatrixreadonly::DOMMatrixReadOnly;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSTransformValue {
    css_style_value: CSSStyleValue,
    components: DomRefCell<Vec<Dom<CSSTransformComponent>>>,
}

impl CSSTransformValue {
    fn new_inherited(components: Vec<Dom<CSSTransformComponent>>) -> CSSTransformValue {
        CSSTransformValue {
            css_style_value: CSSStyleValue::new_inherited(String::new(), None),
            components: DomRefCell::new(components),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        components: Vec<DomRoot<CSSTransformComponent>>,
    ) -> DomRoot<CSSTransformValue> {
        Self::new_with_proto(cx, global, None, components)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        components: Vec<DomRoot<CSSTransformComponent>>,
    ) -> DomRoot<CSSTransformValue> {
        let components = components
            .iter()
            .map(|component| Dom::from_ref(&**component))
            .collect();
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSTransformValue::new_inherited(components)),
            global,
            proto,
        )
    }

    pub(crate) fn components(&self) -> Vec<DomRoot<CSSTransformComponent>> {
        self.components
            .borrow()
            .iter()
            .map(|component| DomRoot::from_ref(&**component))
            .collect()
    }

    pub(crate) fn is_2d(&self) -> bool {
        self.components
            .borrow()
            .iter()
            .all(|component| component.is_2d())
    }
}

impl CSSTransformValueMethods<crate::DomTypeHolder> for CSSTransformValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-csstransformvalue>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        transforms: Vec<DomRoot<CSSTransformComponent>>,
    ) -> Fallible<DomRoot<CSSTransformValue>> {
        if transforms.is_empty() {
            return Err(Error::Type(
                CString::new("CSSTransformValue requires at least one component").unwrap(),
            ));
        }
        Ok(CSSTransformValue::new_with_proto(
            cx, global, proto, transforms,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-length>
    fn Length(&self) -> u32 {
        self.components.borrow().len() as u32
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-__getter__-index>
    fn IndexedGetter(&self, index: u32) -> Fallible<Option<DomRoot<CSSTransformComponent>>> {
        Ok(self
            .components
            .borrow()
            .get(index as usize)
            .map(|component| DomRoot::from_ref(&**component)))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-__setter__-index-val>
    fn IndexedSetter(&self, index: u32, value: &CSSTransformComponent) -> ErrorResult {
        let mut components = self.components.borrow_mut();
        if index as usize >= components.len() {
            components.push(Dom::from_ref(value));
            return Ok(());
        }
        components[index as usize] = Dom::from_ref(value);
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-is2d>
    fn Is2D(&self) -> bool {
        self.is_2d()
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csstransformvalue-tomatrix>
    fn ToMatrix(&self, cx: &mut JSContext) -> Fallible<DomRoot<DOMMatrix>> {
        let global = self.global();
        let components = self.components();
        let mut matrix = Transform3D::identity();
        for component in &components {
            let component_matrix = component.to_matrix_dispatch(cx)?;
            let component_matrix = *component_matrix.upcast::<DOMMatrixReadOnly>().matrix();
            matrix = component_matrix.then(&matrix);
        }
        Ok(DOMMatrix::new(cx, &global, self.is_2d(), matrix))
    }
}
