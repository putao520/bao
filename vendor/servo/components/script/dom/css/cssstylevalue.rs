/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use cssparser::Parser;
use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use servo_url::ServoUrl;
use stylo_atoms::Atom;

use super::typed_om_reify::{parse_property_value, parse_property_value_all};
use super::typed_om_serialize::serialize_style_value;
use crate::dom::bindings::codegen::Bindings::CSSStyleValueBinding::CSSStyleValueMethods;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::{DOMString, USVString};
use crate::dom::globalscope::GlobalScope;

/// <https://drafts.css-houdini.org/css-typed-om-1/#cssstylevalue>
#[dom_struct]
pub(crate) struct CSSStyleValue {
    reflector: Reflector,
    /// The CSS text this value was created from. For reified values this is the
    /// canonical specified serialization; for the base `CSSStyleValue` class it
    /// is the whole unsupported value.
    value: String,
    /// The property this value was reified from, if any. Setting a base
    /// `CSSStyleValue` on a different property is a `TypeError`
    /// (<https://drafts.css-houdini.org/css-typed-om-1/#stylepropertymap-set-val>).
    /// `Atom` is an interned string (string_cache), not JS-managed.
    #[no_trace]
    #[ignore_malloc_size_of = "interned string (string_cache)"]
    associated_property: Option<Atom>,
}

impl CSSStyleValue {
    pub(crate) fn new_inherited(value: String, associated_property: Option<Atom>) -> CSSStyleValue {
        CSSStyleValue {
            reflector: Reflector::new(),
            value,
            associated_property,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        value: String,
    ) -> DomRoot<CSSStyleValue> {
        reflect_dom_object(
            cx,
            Box::new(CSSStyleValue::new_inherited(value, None)),
            global,
        )
    }

    /// Creates a base value that remembers the property it was reified from.
    pub(crate) fn new_with_property(
        cx: &mut JSContext,
        global: &GlobalScope,
        value: String,
        property: &str,
    ) -> DomRoot<CSSStyleValue> {
        reflect_dom_object(
            cx,
            Box::new(CSSStyleValue::new_inherited(
                value,
                Some(Atom::from(property)),
            )),
            global,
        )
    }

    /// The value's stored CSS text (the base class serialization).
    pub(crate) fn serialized_text(&self) -> &str {
        &self.value
    }

    /// The property this value is associated with, if any.
    pub(crate) fn associated_property(&self) -> Option<&str> {
        self.associated_property.as_deref()
    }

    /// Parse the value as a `url()`.
    /// TODO: This should really always be an absolute URL, but we currently
    /// return relative URLs for computed values, so we pass in a base.
    /// <https://github.com/servo/servo/issues/17625>
    pub(crate) fn get_url(&self, base_url: ServoUrl) -> Option<ServoUrl> {
        let mut parser = Parser::new(&self.value);
        parser
            .expect_url()
            .ok()
            .and_then(|string| base_url.join(&string).ok())
    }
}

impl CSSStyleValueMethods<crate::DomTypeHolder> for CSSStyleValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#CSSStyleValue-stringification-behavior>
    fn Stringifier(&self) -> DOMString {
        DOMString::from(serialize_style_value(self))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssstylevalue-parse>
    fn Parse(
        cx: &mut JSContext,
        global: &GlobalScope,
        property: USVString,
        css_text: USVString,
    ) -> Fallible<DomRoot<CSSStyleValue>> {
        parse_property_value(cx, global, &property.0, &css_text.0)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssstylevalue-parseall>
    fn ParseAll(
        cx: &mut JSContext,
        global: &GlobalScope,
        property: USVString,
        css_text: USVString,
    ) -> Fallible<Vec<DomRoot<CSSStyleValue>>> {
        parse_property_value_all(cx, global, &property.0, &css_text.0)
    }
}
