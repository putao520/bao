/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csskeywordvalue>

use crate::dom::bindings::codegen::Bindings::CSSKeywordValueBinding::CSSKeywordValueMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::cssstylevalue::CSSStyleValue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSKeywordValue {
    css_style_value: CSSStyleValue,
    value: DomRefCell<String>,
}

impl CSSKeywordValue {
    fn new_inherited(value: String) -> CSSKeywordValue {
        CSSKeywordValue {
            css_style_value: CSSStyleValue::new_inherited(String::new(), None),
            value: DomRefCell::new(value),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        value: String,
    ) -> DomRoot<CSSKeywordValue> {
        reflect_dom_object(cx, Box::new(CSSKeywordValue::new_inherited(value)), global)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: String,
    ) -> DomRoot<CSSKeywordValue> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSKeywordValue::new_inherited(value)),
            global,
            proto,
        )
    }

    /// The keyword string (used by reification/serialization).
    pub(crate) fn value(&self) -> String {
        self.value.borrow().clone()
    }
}

impl CSSKeywordValueMethods<crate::DomTypeHolder> for CSSKeywordValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csskeywordvalue-csskeywordvalue>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: USVString,
    ) -> Fallible<DomRoot<CSSKeywordValue>> {
        if value.0.is_empty() {
            return Err(Error::Type(
                c"CSSKeywordValue cannot be constructed with an empty string".to_owned(),
            ));
        }
        Ok(CSSKeywordValue::new_with_proto(cx, global, proto, value.0))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csskeywordvalue-value>
    fn GetValue(&self) -> Fallible<USVString> {
        Ok(USVString(self.value()))
    }

    fn SetValue(&self, value: USVString) -> ErrorResult {
        if value.0.is_empty() {
            return Err(Error::Type(
                c"CSSKeywordValue cannot be set to an empty string".to_owned(),
            ));
        }
        *self.value.borrow_mut() = value.0;
        Ok(())
    }
}
