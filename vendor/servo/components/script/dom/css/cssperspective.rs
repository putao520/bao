/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssperspective>

use std::ffi::CString;

use crate::dom::bindings::codegen::Bindings::CSSTransformValueBinding::CSSPerspectiveMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;

use super::csskeywordvalue::CSSKeywordValue;
use super::cssnumericvalue::CSSNumericValue;
use super::csstransformcomponent::CSSTransformComponent;
use super::typed_om_values::numeric_type_of;
use crate::dom::bindings::codegen::UnionTypes::CSSNumericValueOrCSSKeywordValueOrString;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

/// <https://drafts.css-houdini.org/css-typed-om-1/#cssperspectivevalue>
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
pub(crate) enum PerspectiveLength {
    Numeric(Dom<CSSNumericValue>),
    Keyword(Dom<CSSKeywordValue>),
}

#[dom_struct]
pub(crate) struct CSSPerspective {
    css_transform_component: CSSTransformComponent,
    length: DomRefCell<PerspectiveLength>,
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).unwrap())
}

fn length_from_union(
    cx: &mut JSContext,
    global: &GlobalScope,
    value: CSSNumericValueOrCSSKeywordValueOrString,
) -> Fallible<PerspectiveLength> {
    match value {
        CSSNumericValueOrCSSKeywordValueOrString::CSSNumericValue(value) => {
            if !numeric_type_of(&value).is_ok_and(|numeric_type| numeric_type.matches_length()) {
                return Err(type_error("perspective length must be a <length>"));
            }
            Ok(PerspectiveLength::Numeric(Dom::from_ref(&*value)))
        },
        CSSNumericValueOrCSSKeywordValueOrString::CSSKeywordValue(value) => {
            if !value.value().eq_ignore_ascii_case("none") {
                return Err(type_error("perspective length must be 'none'"));
            }
            Ok(PerspectiveLength::Keyword(Dom::from_ref(&*value)))
        },
        CSSNumericValueOrCSSKeywordValueOrString::String(value) => {
            let text = value.str();
            if !text.eq_ignore_ascii_case("none") {
                return Err(type_error("perspective length must be 'none'"));
            }
            let keyword = CSSKeywordValue::new(cx, global, "none".to_string());
            Ok(PerspectiveLength::Keyword(Dom::from_ref(&*keyword)))
        },
    }
}

impl CSSPerspective {
    fn new_inherited(length: PerspectiveLength) -> CSSPerspective {
        CSSPerspective {
            css_transform_component: CSSTransformComponent::new_inherited(false),
            length: DomRefCell::new(length),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        length: DomRoot<CSSNumericValue>,
    ) -> DomRoot<CSSPerspective> {
        Self::new_with_proto(
            cx,
            global,
            None,
            PerspectiveLength::Numeric(Dom::from_ref(&*length)),
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        length: PerspectiveLength,
    ) -> DomRoot<CSSPerspective> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSPerspective::new_inherited(length)),
            global,
            proto,
        )
    }

    pub(crate) fn length_as_numeric(&self) -> Option<DomRoot<CSSNumericValue>> {
        match &*self.length.borrow() {
            PerspectiveLength::Numeric(value) => Some(DomRoot::from_ref(&**value)),
            PerspectiveLength::Keyword(_) => None,
        }
    }

    pub(crate) fn length_as_keyword(&self) -> Option<DomRoot<CSSKeywordValue>> {
        match &*self.length.borrow() {
            PerspectiveLength::Keyword(value) => Some(DomRoot::from_ref(&**value)),
            PerspectiveLength::Numeric(_) => None,
        }
    }
}

impl CSSPerspectiveMethods<crate::DomTypeHolder> for CSSPerspective {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssperspective-cssperspective>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        length: CSSNumericValueOrCSSKeywordValueOrString,
    ) -> Fallible<DomRoot<CSSPerspective>> {
        let length = length_from_union(cx, global, length)?;
        Ok(CSSPerspective::new_with_proto(cx, global, proto, length))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssperspective-length>
    fn GetLength(&self) -> Fallible<CSSNumericValueOrCSSKeywordValueOrString> {
        Ok(match &*self.length.borrow() {
            PerspectiveLength::Numeric(value) => {
                CSSNumericValueOrCSSKeywordValueOrString::CSSNumericValue(DomRoot::from_ref(
                    &**value,
                ))
            },
            PerspectiveLength::Keyword(value) => {
                CSSNumericValueOrCSSKeywordValueOrString::CSSKeywordValue(DomRoot::from_ref(
                    &**value,
                ))
            },
        })
    }

    fn SetLength(
        &self,
        cx: &mut JSContext,
        value: CSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.length.borrow_mut() = length_from_union(cx, &global, value)?;
        Ok(())
    }
}
