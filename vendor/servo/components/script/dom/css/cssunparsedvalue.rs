/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#unparsedvalue>

use crate::dom::bindings::codegen::Bindings::CSSUnparsedValueBinding::CSSUnparsedValueMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::cssstylevalue::CSSStyleValue;
use super::cssvariablereferencevalue::CSSVariableReferenceValue;
use crate::dom::bindings::codegen::UnionTypes::USVStringOrCSSVariableReferenceValue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::USVString;
use crate::dom::globalscope::GlobalScope;

/// <https://drafts.css-houdini.org/css-typed-om-1/#cssunparsedsegment>
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
pub(crate) enum CSSUnparsedSegment {
    Text(String),
    VariableReference(Dom<CSSVariableReferenceValue>),
}

#[dom_struct]
pub(crate) struct CSSUnparsedValue {
    css_style_value: CSSStyleValue,
    segments: DomRefCell<Vec<CSSUnparsedSegment>>,
}

impl CSSUnparsedValue {
    fn new_inherited(segments: Vec<CSSUnparsedSegment>) -> CSSUnparsedValue {
        CSSUnparsedValue {
            css_style_value: CSSStyleValue::new_inherited(String::new(), None),
            segments: DomRefCell::new(segments),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        segments: Vec<CSSUnparsedSegment>,
    ) -> DomRoot<CSSUnparsedValue> {
        reflect_dom_object(
            cx,
            Box::new(CSSUnparsedValue::new_inherited(segments)),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        segments: Vec<CSSUnparsedSegment>,
    ) -> DomRoot<CSSUnparsedValue> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSUnparsedValue::new_inherited(segments)),
            global,
            proto,
        )
    }

    /// Runs `f` with the value's segments (serialization accessor).
    pub(crate) fn with_segments<R>(&self, f: impl FnOnce(&[CSSUnparsedSegment]) -> R) -> R {
        f(&self.segments.borrow())
    }

    fn segment_at(&self, index: u32) -> Option<USVStringOrCSSVariableReferenceValue> {
        let segments = self.segments.borrow();
        match segments.get(index as usize)? {
            CSSUnparsedSegment::Text(text) => Some(
                USVStringOrCSSVariableReferenceValue::USVString(USVString(text.clone())),
            ),
            CSSUnparsedSegment::VariableReference(reference) => Some(
                USVStringOrCSSVariableReferenceValue::CSSVariableReferenceValue(DomRoot::from_ref(
                    &**reference,
                )),
            ),
        }
    }
}

impl CSSUnparsedValueMethods<crate::DomTypeHolder> for CSSUnparsedValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunparsedvalue-cssunparsedvalue>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        members: Vec<USVStringOrCSSVariableReferenceValue>,
    ) -> Fallible<DomRoot<CSSUnparsedValue>> {
        let mut segments = Vec::with_capacity(members.len());
        for member in members {
            segments.push(match member {
                USVStringOrCSSVariableReferenceValue::USVString(text) => {
                    CSSUnparsedSegment::Text(text.0)
                },
                USVStringOrCSSVariableReferenceValue::CSSVariableReferenceValue(reference) => {
                    CSSUnparsedSegment::VariableReference(Dom::from_ref(&*reference))
                },
            });
        }
        Ok(CSSUnparsedValue::new_with_proto(
            cx, global, proto, segments,
        ))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunparsedvalue-length>
    fn Length(&self) -> u32 {
        self.segments.borrow().len() as u32
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunparsedvalue-__getter__-index-index>
    fn IndexedGetter(&self, index: u32) -> Fallible<Option<USVStringOrCSSVariableReferenceValue>> {
        Ok(self.segment_at(index))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssunparsedvalue-__setter__-index-val-val>
    fn IndexedSetter(
        &self,
        index: u32,
        value: USVStringOrCSSVariableReferenceValue,
    ) -> ErrorResult {
        let segment = match value {
            USVStringOrCSSVariableReferenceValue::USVString(text) => {
                CSSUnparsedSegment::Text(text.0)
            },
            USVStringOrCSSVariableReferenceValue::CSSVariableReferenceValue(reference) => {
                CSSUnparsedSegment::VariableReference(Dom::from_ref(&*reference))
            },
        };
        let mut segments = self.segments.borrow_mut();
        if index as usize == segments.len() {
            segments.push(segment);
            return Ok(());
        }
        match segments.get_mut(index as usize) {
            Some(slot) => {
                *slot = segment;
                Ok(())
            },
            None => Err(Error::Range(c"Index out of range".to_owned())),
        }
    }
}
