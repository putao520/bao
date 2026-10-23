/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssunitvalue>

use std::cell::Cell;

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::CSSUnitValueMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::num::Finite;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};
use stylo_atoms::Atom;

use super::cssnumericvalue::CSSNumericValue;
use super::typed_om_numeric::NumericType;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSUnitValue {
    css_numeric_value: CSSNumericValue,
    value: Cell<f64>,
    /// `Atom` is an interned string (string_cache), not JS-managed.
    #[no_trace]
    #[ignore_malloc_size_of = "interned string (string_cache)"]
    unit: Atom,
}

impl CSSUnitValue {
    fn new_inherited(value: f64, unit: &str) -> CSSUnitValue {
        CSSUnitValue {
            css_numeric_value: CSSNumericValue::new_inherited(),
            value: Cell::new(value),
            unit: Atom::from(unit.to_ascii_lowercase()),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        value: f64,
        unit: &str,
    ) -> DomRoot<CSSUnitValue> {
        reflect_dom_object(
            cx,
            Box::new(CSSUnitValue::new_inherited(value, unit)),
            global,
        )
    }

    pub(crate) fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: f64,
        unit: &str,
    ) -> DomRoot<CSSUnitValue> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSUnitValue::new_inherited(value, unit)),
            global,
            proto,
        )
    }

    pub(crate) fn value(&self) -> f64 {
        self.value.get()
    }

    pub(crate) fn unit_str(&self) -> &str {
        &*self.unit
    }
}

impl CSSUnitValueMethods<crate::DomTypeHolder> for CSSUnitValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunitvalue-cssunitvalue>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        value: Finite<f64>,
        unit: USVString,
    ) -> Fallible<DomRoot<CSSUnitValue>> {
        let unit = unit.0.to_ascii_lowercase();
        if NumericType::from_unit(&unit).is_err() {
            return Err(Error::Type(c"Invalid unit for CSSUnitValue".to_owned()));
        }
        Ok(CSSUnitValue::new_with_proto(
            cx, global, proto, *value, &unit,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunitvalue-value>
    fn Value(&self) -> f64 {
        self.value.get()
    }

    fn SetValue(&self, value: f64) {
        self.value.set(value);
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunitvalue-unit>
    fn Unit(&self) -> USVString {
        USVString(self.unit_str().to_string())
    }
}
