/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#stylepropertymapreadonly>

use crate::dom::bindings::codegen::Bindings::StylePropertyMapReadOnlyBinding::StylePropertyMapReadOnlyMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use style::properties::PropertyId;

use super::cssstylevalue::CSSStyleValue;
use super::typed_om_reify::{reify_engine_value, split_top_level_commas};
use crate::dom::bindings::codegen::Bindings::CSSStyleDeclarationBinding::CSSStyleDeclarationMethods;
use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::WindowBinding::Window_Binding::WindowMethods;
use crate::dom::bindings::codegen::UnionTypes::UndefinedOrCSSStyleValue;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::iterable::Iterable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::{DOMString, USVString};
use crate::dom::css::cssstyledeclaration::CSSStyleDeclaration;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct StylePropertyMapReadOnly {
    reflector: Reflector,
    declaration: Dom<CSSStyleDeclaration>,
}

fn invalid_property_error() -> Error {
    Error::Type(std::ffi::CString::new("Invalid property name").unwrap())
}

impl StylePropertyMapReadOnly {
    pub(crate) fn new_inherited(declaration: Dom<CSSStyleDeclaration>) -> StylePropertyMapReadOnly {
        StylePropertyMapReadOnly {
            reflector: Reflector::new(),
            declaration,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        declaration: DomRoot<CSSStyleDeclaration>,
    ) -> DomRoot<StylePropertyMapReadOnly> {
        reflect_dom_object(
            cx,
            Box::new(StylePropertyMapReadOnly::new_inherited(Dom::from_ref(
                &*declaration,
            ))),
            global,
        )
    }

    pub(crate) fn declaration(&self) -> DomRoot<CSSStyleDeclaration> {
        DomRoot::from_ref(&*self.declaration)
    }

    /// Build a standalone map from `(property, css-text)` pairs (the
    /// CSS Paint API `StylePropertyMapReadOnly` shape:
    /// <https://drafts.css-houdini.org/css-paint-api-1/#stylepropertymapreadonly>).
    /// The pairs are written into a detached `CSSStyleDeclaration` backed by
    /// the worklet document's root element style attribute block.
    pub(crate) fn from_iter(
        cx: &mut JSContext,
        global: &GlobalScope,
        properties: impl IntoIterator<Item = (stylo_atoms::Atom, String)>,
    ) -> DomRoot<StylePropertyMapReadOnly> {
        use crate::dom::css::cssstyledeclaration::{CSSModificationAccess, CSSStyleOwner};

        let window = global
            .downcast::<crate::dom::window::Window>()
            .expect("paint worklet global is a Window");
        let element = window
            .Document()
            .GetDocumentElement()
            .expect("paint worklet document must have a root element");
        let declaration = CSSStyleDeclaration::new(
            cx,
            window,
            CSSStyleOwner::Element(Dom::from_ref(element.upcast())),
            None,
            CSSModificationAccess::ReadWrite,
        );
        for (atom, text) in properties {
            let property = atom.to_string();
            declaration.SetProperty(
                cx,
                DOMString::from(property),
                DOMString::from(text),
                DOMString::new(),
            );
        }
        StylePropertyMapReadOnly::new(cx, global, declaration)
    }

    /// Validates a property name for the map API (`TypeError` otherwise).
    pub(crate) fn validate_property(property: &str) -> Fallible<PropertyId> {
        PropertyId::parse_enabled_for_all_content(property).map_err(|_| invalid_property_error())
    }

    /// Reads the CSS text of `property` from the backing declaration.
    fn property_text(&self, property: &str) -> DOMString {
        let declaration = self.declaration();
        declaration.GetPropertyValue(DOMString::from(property))
    }

    /// Splits the property's value into its list of CSS values (empty when the
    /// property is not present).
    fn value_texts(&self, property: &str, id: &PropertyId) -> Vec<String> {
        let text = self.property_text(property);
        if text.is_empty() {
            return Vec::new();
        }
        if matches!(id, PropertyId::Custom(_)) || id.as_shorthand().is_ok() {
            return vec![text.str().to_string()];
        }
        split_top_level_commas(&text.str())
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// Reifies all values of `property` into typed objects.
    pub(crate) fn typed_values(
        &self,
        cx: &mut JSContext,
        property: &str,
        id: &PropertyId,
    ) -> Vec<DomRoot<CSSStyleValue>> {
        let global = self.global();
        self.value_texts(property, id)
            .into_iter()
            .map(|text| reify_engine_value(cx, &global, property, &text))
            .collect()
    }
}

impl StylePropertyMapReadOnlyMethods<crate::DomTypeHolder> for StylePropertyMapReadOnly {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymapreadonly-get>
    fn Get(&self, cx: &mut JSContext, property: USVString) -> Fallible<UndefinedOrCSSStyleValue> {
        let id = StylePropertyMapReadOnly::validate_property(&property.0)?;
        let values = self.typed_values(cx, &property.0, &id);
        Ok(match values.into_iter().next() {
            Some(value) => UndefinedOrCSSStyleValue::CSSStyleValue(value),
            None => UndefinedOrCSSStyleValue::Undefined(()),
        })
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymapreadonly-getall>
    fn GetAll(
        &self,
        cx: &mut JSContext,
        property: USVString,
    ) -> Fallible<Vec<DomRoot<CSSStyleValue>>> {
        let id = StylePropertyMapReadOnly::validate_property(&property.0)?;
        Ok(self.typed_values(cx, &property.0, &id))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymapreadonly-has>
    fn Has(&self, property: USVString) -> Fallible<bool> {
        let _id = StylePropertyMapReadOnly::validate_property(&property.0)?;
        Ok(!self.property_text(&property.0).is_empty())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-stylepropertymapreadonly-size>
    fn Size(&self) -> u32 {
        let declaration = self.declaration();
        declaration.Length()
    }
}

impl Iterable for StylePropertyMapReadOnly {
    type Key = USVString;
    type Value = Vec<DomRoot<CSSStyleValue>>;

    fn get_iterable_length(&self, _cx: &mut JSContext) -> u32 {
        let declaration = self.declaration();
        declaration.Length()
    }

    fn get_value_at_index(&self, cx: &mut JSContext, index: u32) -> Vec<DomRoot<CSSStyleValue>> {
        let declaration = self.declaration();
        let property = declaration.Item(index).str().to_string();
        match StylePropertyMapReadOnly::validate_property(&property) {
            Ok(id) => self.typed_values(cx, &property, &id),
            Err(..) => Vec::new(),
        }
    }

    fn get_key_at_index(&self, _cx: &mut JSContext, index: u32) -> USVString {
        let declaration = self.declaration();
        USVString(declaration.Item(index).str().to_string())
    }
}
