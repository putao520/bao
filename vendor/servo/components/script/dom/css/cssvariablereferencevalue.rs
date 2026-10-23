/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssvariablereferencevalue>

use crate::dom::bindings::codegen::Bindings::CSSUnparsedValueBinding::CSSVariableReferenceValueMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{Reflector, reflect_dom_object, reflect_dom_object_with_proto};
use style::custom_properties;

use super::cssunparsedvalue::CSSUnparsedValue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::root::{DomRoot, MutNullableDom};
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSVariableReferenceValue {
    reflector: Reflector,
    variable: DomRefCell<String>,
    fallback: MutNullableDom<CSSUnparsedValue>,
}

fn validate_variable_name(variable: &str) -> bool {
    custom_properties::parse_name(variable).is_ok()
}

impl CSSVariableReferenceValue {
    fn new_inherited(
        variable: String,
        fallback: Option<DomRoot<CSSUnparsedValue>>,
    ) -> CSSVariableReferenceValue {
        CSSVariableReferenceValue {
            reflector: Reflector::new(),
            variable: DomRefCell::new(variable),
            fallback: MutNullableDom::new(fallback.as_deref()),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        variable: String,
        fallback: Option<DomRoot<CSSUnparsedValue>>,
    ) -> DomRoot<CSSVariableReferenceValue> {
        reflect_dom_object(
            cx,
            Box::new(CSSVariableReferenceValue::new_inherited(variable, fallback)),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        variable: String,
        fallback: Option<DomRoot<CSSUnparsedValue>>,
    ) -> DomRoot<CSSVariableReferenceValue> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSVariableReferenceValue::new_inherited(variable, fallback)),
            global,
            proto,
        )
    }

    pub(crate) fn variable(&self) -> String {
        self.variable.borrow().clone()
    }

    pub(crate) fn fallback_root(&self) -> Option<DomRoot<CSSUnparsedValue>> {
        self.fallback.get()
    }
}

impl CSSVariableReferenceValueMethods<crate::DomTypeHolder> for CSSVariableReferenceValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssvariablereferencevalue-cssvariablereferencevalue>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        variable: USVString,
        fallback: Option<&CSSUnparsedValue>,
    ) -> Fallible<DomRoot<CSSVariableReferenceValue>> {
        if !validate_variable_name(&variable.0) {
            return Err(Error::Type(
                c"Invalid custom property name for CSSVariableReferenceValue".to_owned(),
            ));
        }
        let fallback = fallback.map(DomRoot::from_ref);
        Ok(CSSVariableReferenceValue::new_with_proto(
            cx, global, proto, variable.0, fallback,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssvariablereferencevalue-variable>
    fn GetVariable(&self) -> Fallible<USVString> {
        Ok(USVString(self.variable()))
    }

    fn SetVariable(&self, value: USVString) -> ErrorResult {
        if !validate_variable_name(&value.0) {
            return Err(Error::Type(
                c"Invalid custom property name for CSSVariableReferenceValue".to_owned(),
            ));
        }
        *self.variable.borrow_mut() = value.0;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssvariablereferencevalue-fallback>
    fn GetFallback(&self) -> Option<DomRoot<CSSUnparsedValue>> {
        self.fallback_root()
    }
}
