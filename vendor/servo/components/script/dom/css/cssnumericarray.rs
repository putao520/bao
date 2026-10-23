/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssnumericarray>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSNumericArrayMethods;
use dom_struct::dom_struct;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{Reflector, reflect_dom_object};

use super::cssnumericvalue::CSSNumericValue;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSNumericArray {
    reflector: Reflector,
    values: DomRefCell<Vec<Dom<CSSNumericValue>>>,
}

impl CSSNumericArray {
    fn new_inherited(values: Vec<Dom<CSSNumericValue>>) -> CSSNumericArray {
        CSSNumericArray {
            reflector: Reflector::new(),
            values: DomRefCell::new(values),
        }
    }

    pub(crate) fn new(
        cx: &mut js::context::JSContext,
        global: &GlobalScope,
        values: Vec<DomRoot<CSSNumericValue>>,
    ) -> DomRoot<CSSNumericArray> {
        let values = values.iter().map(|value| Dom::from_ref(&**value)).collect();
        reflect_dom_object(cx, Box::new(CSSNumericArray::new_inherited(values)), global)
    }
}

impl CSSNumericArrayMethods<crate::DomTypeHolder> for CSSNumericArray {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericarray-length>
    fn Length(&self) -> u32 {
        self.values.borrow().len() as u32
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssnumericarray-__getter__-index-index>
    fn IndexedGetter(&self, index: u32) -> Fallible<Option<DomRoot<CSSNumericValue>>> {
        Ok(self
            .values
            .borrow()
            .get(index as usize)
            .map(|value| DomRoot::from_ref(&**value)))
    }
}
