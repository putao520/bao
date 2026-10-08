/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `TextFormat` from the W3C EditContext API (REQ-BRW-050, Bao fork
//! self-build — see `dom::editcontext`).

use std::cell::Cell;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};

use crate::dom::bindings::codegen::Bindings::TextFormatUpdateBinding::TextFormatInit;
use crate::dom::bindings::codegen::Bindings::TextFormatUpdateBinding::TextFormatMethods;
use crate::dom::bindings::codegen::Bindings::TextFormatUpdateBinding::{
    UnderlineStyle, UnderlineThickness,
};
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::DomRoot;
use crate::dom::types::Window;

/// <https://w3c.github.io/edit-context/#textformat>
#[dom_struct]
pub(crate) struct TextFormat {
    reflector_: Reflector,
    range_start: Cell<u32>,
    range_end: Cell<u32>,
    underline_style: UnderlineStyle,
    underline_thickness: UnderlineThickness,
}

impl TextFormat {
    fn new_inherited(
        range_start: u32,
        range_end: u32,
        underline_style: UnderlineStyle,
        underline_thickness: UnderlineThickness,
    ) -> TextFormat {
        TextFormat {
            reflector_: Reflector::new(),
            range_start: Cell::new(range_start),
            range_end: Cell::new(range_end),
            underline_style,
            underline_thickness,
        }
    }

    fn new(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        range_start: u32,
        range_end: u32,
        underline_style: UnderlineStyle,
        underline_thickness: UnderlineThickness,
    ) -> DomRoot<TextFormat> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(TextFormat::new_inherited(
                range_start,
                range_end,
                underline_style,
                underline_thickness,
            )),
            window,
            proto,
        )
    }
}

impl TextFormatMethods<crate::DomTypeHolder> for TextFormat {
    /// <https://w3c.github.io/edit-context/#dom-textformat>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        options: &TextFormatInit,
    ) -> DomRoot<TextFormat> {
        TextFormat::new(
            cx,
            window,
            proto,
            options.rangeStart,
            options.rangeEnd,
            options.underlineStyle,
            options.underlineThickness,
        )
    }

    /// <https://w3c.github.io/edit-context/#dom-textformat-rangestart>
    fn RangeStart(&self) -> u32 {
        self.range_start.get()
    }

    /// <https://w3c.github.io/edit-context/#dom-textformat-rangeend>
    fn RangeEnd(&self) -> u32 {
        self.range_end.get()
    }

    /// <https://w3c.github.io/edit-context/#dom-textformat-underlinestyle>
    fn UnderlineStyle(&self) -> UnderlineStyle {
        self.underline_style
    }

    /// <https://w3c.github.io/edit-context/#dom-textformat-underlinethickness>
    fn UnderlineThickness(&self) -> UnderlineThickness {
        self.underline_thickness
    }
}
