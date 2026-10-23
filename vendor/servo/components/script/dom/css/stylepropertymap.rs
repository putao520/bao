/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#stylepropertymap>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::StylePropertyMapBinding::StylePropertyMapMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::reflect_dom_object;
use style::properties::PropertyId;

use super::cssstylevalue::CSSStyleValue;
use super::stylepropertymapreadonly::StylePropertyMapReadOnly;
use super::typed_om_reify::validate_property_value;
use super::typed_om_serialize::serialize_style_value;
use super::typed_om_tokens::{segments_have_variable_reference, tokenize_unparsed};
use crate::dom::bindings::codegen::Bindings::CSSStyleDeclarationBinding::CSSStyleDeclarationMethods;
use crate::dom::bindings::codegen::UnionTypes::CSSStyleValueOrUSVString;
use crate::dom::bindings::error::{Error, ErrorResult};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::{DOMString, USVString};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct StylePropertyMap {
    style_property_map_read_only: StylePropertyMapReadOnly,
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).unwrap())
}

fn text_has_variable_reference(text: &str) -> bool {
    match tokenize_unparsed(text) {
        Ok(segments) => segments_have_variable_reference(&segments),
        Err(()) => false,
    }
}

fn base_value_matches_property(value: &CSSStyleValue, property: &str) -> bool {
    match value.associated_property() {
        None => true,
        Some(associated) => {
            if associated.starts_with("--") || property.starts_with("--") {
                associated == property
            } else {
                associated.eq_ignore_ascii_case(property)
            }
        },
    }
}

/// The serialized CSS text of one `(CSSStyleValue or USVString)` member.
fn value_to_css_text(value: &CSSStyleValueOrUSVString) -> String {
    match value {
        CSSStyleValueOrUSVString::CSSStyleValue(value) => serialize_style_value(value),
        CSSStyleValueOrUSVString::USVString(value) => value.0.clone(),
    }
}

impl StylePropertyMap {
    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        declaration: DomRoot<crate::dom::css::cssstyledeclaration::CSSStyleDeclaration>,
    ) -> DomRoot<StylePropertyMap> {
        let read_only = StylePropertyMapReadOnly::new_inherited(Dom::from_ref(&*declaration));
        reflect_dom_object(
            cx,
            Box::new(StylePropertyMap {
                style_property_map_read_only: read_only,
            }),
            global,
        )
    }

    fn declaration(&self) -> DomRoot<crate::dom::css::cssstyledeclaration::CSSStyleDeclaration> {
        self.style_property_map_read_only.declaration()
    }

    /// Validates and serializes `(CSSStyleValue or USVString)...` members for
    /// `property`, returning the declaration value text.
    fn join_values(property: &str, values: &[CSSStyleValueOrUSVString]) -> Result<String, Error> {
        if values.is_empty() {
            return Err(type_error("At least one value is required"));
        }
        for value in values {
            if let CSSStyleValueOrUSVString::CSSStyleValue(style_value) = value {
                if !base_value_matches_property(style_value, property) {
                    return Err(type_error(
                        "CSSStyleValue was created for a different property",
                    ));
                }
            }
        }
        let texts: Vec<String> = values.iter().map(value_to_css_text).collect();
        if texts.len() > 1 {
            for text in &texts {
                if text_has_variable_reference(text) {
                    return Err(type_error("Cannot mix a var() reference with other values"));
                }
            }
            let joined = texts.join(", ");
            validate_property_value(property, &joined)
                .map_err(|_| type_error("Invalid value for property"))?;
            return Ok(joined);
        }
        let joined = texts.into_iter().next().unwrap();
        validate_property_value(property, &joined)
            .map_err(|_| type_error("Invalid value for property"))?;
        Ok(joined)
    }

    fn apply(&self, cx: &mut JSContext, property: &str, text: &str) -> ErrorResult {
        let declaration = self.declaration();
        declaration.SetProperty(
            cx,
            DOMString::from(property),
            DOMString::from(text),
            DOMString::new(),
        )
    }
}

impl StylePropertyMapMethods<crate::DomTypeHolder> for StylePropertyMap {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymap-set>
    fn Set(
        &self,
        cx: &mut JSContext,
        property: USVString,
        values: Vec<CSSStyleValueOrUSVString>,
    ) -> ErrorResult {
        StylePropertyMapReadOnly::validate_property(&property.0)?;
        let joined = StylePropertyMap::join_values(&property.0, &values)?;
        self.apply(cx, &property.0, &joined)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymap-append>
    fn Append(
        &self,
        cx: &mut JSContext,
        property: USVString,
        values: Vec<CSSStyleValueOrUSVString>,
    ) -> ErrorResult {
        let id = StylePropertyMapReadOnly::validate_property(&property.0)?;
        if id.as_shorthand().is_ok() {
            return Err(type_error("Cannot append to a shorthand property"));
        }
        let joined = StylePropertyMap::join_values(&property.0, &values)?;
        let declaration = self.declaration();
        let existing = declaration.GetPropertyValue(DOMString::from(property.0.clone()));
        let existing_text = existing.str().to_string();
        if existing_text.is_empty() {
            return self.apply(cx, &property.0, &joined);
        }
        if text_has_variable_reference(&existing_text) {
            return Err(type_error("Cannot append to a var() reference"));
        }
        let combined = format!("{}, {}", existing_text, joined);
        validate_property_value(&property.0, &combined)
            .map_err(|_| type_error("Invalid value for property"))?;
        self.apply(cx, &property.0, &combined)
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymap-delete>
    fn Delete(&self, cx: &mut JSContext, property: USVString) -> ErrorResult {
        StylePropertyMapReadOnly::validate_property(&property.0)?;
        let declaration = self.declaration();
        declaration.RemoveProperty(cx, DOMString::from(property.0))?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymap-clear>
    fn Clear(&self, cx: &mut JSContext) -> ErrorResult {
        let declaration = self.declaration();
        declaration.SetCssText(cx, DOMString::new())
    }
}
