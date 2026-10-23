/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssmatrixcomponent>

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::{
    CSSMatrixComponentMethods, CSSMatrixComponentOptions,
};
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::csstransformcomponent::CSSTransformComponent;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::dommatrix::DOMMatrix;
use crate::dom::dommatrixreadonly::DOMMatrixReadOnly;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSMatrixComponent {
    css_transform_component: CSSTransformComponent,
    matrix: DomRefCell<Dom<DOMMatrix>>,
}

impl CSSMatrixComponent {
    fn new_inherited(matrix: Dom<DOMMatrix>, is2d: bool) -> CSSMatrixComponent {
        CSSMatrixComponent {
            css_transform_component: CSSTransformComponent::new_inherited(is2d),
            matrix: DomRefCell::new(matrix),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        matrix: DomRoot<DOMMatrixReadOnly>,
        options: Option<&CSSMatrixComponentOptions>,
    ) -> DomRoot<CSSMatrixComponent> {
        Self::new_with_proto(cx, global, None, matrix, options)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        matrix: DomRoot<DOMMatrixReadOnly>,
        options: Option<&CSSMatrixComponentOptions>,
    ) -> DomRoot<CSSMatrixComponent> {
        let is2d = options
            .and_then(|options| options.is2D)
            .unwrap_or_else(|| matrix.is2D());
        let matrix = DOMMatrix::from_readonly(global, &matrix, cx);
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSMatrixComponent::new_inherited(
                Dom::from_ref(&*matrix),
                is2d,
            )),
            global,
            proto,
        )
    }

    pub(crate) fn matrix_root(&self) -> DomRoot<DOMMatrix> {
        DomRoot::from_ref(&*self.matrix.borrow())
    }
}

impl CSSMatrixComponentMethods<crate::DomTypeHolder> for CSSMatrixComponent {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmatrixcomponent-cssmatrixcomponent>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        matrix: &DOMMatrixReadOnly,
        options: &CSSMatrixComponentOptions,
    ) -> Fallible<DomRoot<CSSMatrixComponent>> {
        Ok(CSSMatrixComponent::new_with_proto(
            cx,
            global,
            proto,
            DomRoot::from_ref(matrix),
            Some(options),
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmatrixcomponent-matrix>
    fn GetMatrix(&self) -> Fallible<DomRoot<DOMMatrix>> {
        Ok({
            self.matrix_root()
        })
    }

    fn SetMatrix(&self, cx: &mut JSContext, value: &DOMMatrix) -> Fallible<()> {
        let global = self.global();
        let is_2d = value.upcast::<DOMMatrixReadOnly>().is2D();
        let matrix = DOMMatrix::new(
            cx,
            &global,
            is_2d,
            *value.upcast::<DOMMatrixReadOnly>().matrix(),
        );
        *self.matrix.borrow_mut() = Dom::from_ref(&*matrix);
        Ok(())
    }
}
