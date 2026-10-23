/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssrgb>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSRGBMethods;
use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{reflect_dom_object, reflect_dom_object_with_proto};

use super::csscolorvalue::{
    CSSColorValue, ChannelKind, ColorChannel, channel_from_union, channel_to_union,
};
use crate::dom::bindings::codegen::UnionTypes::DoubleOrCSSNumericValueOrCSSKeywordValueOrString;
use crate::dom::bindings::error::{ErrorResult, Fallible};
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct CSSRGB {
    css_color_value: CSSColorValue,
    r: DomRefCell<ColorChannel>,
    g: DomRefCell<ColorChannel>,
    b: DomRefCell<ColorChannel>,
    alpha: DomRefCell<ColorChannel>,
}

impl CSSRGB {
    fn new_inherited(
        r: ColorChannel,
        g: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> CSSRGB {
        CSSRGB {
            css_color_value: CSSColorValue::new_inherited(),
            r: DomRefCell::new(r),
            g: DomRefCell::new(g),
            b: DomRefCell::new(b),
            alpha: DomRefCell::new(alpha),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        r: ColorChannel,
        g: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSRGB> {
        reflect_dom_object(cx, Box::new(CSSRGB::new_inherited(r, g, b, alpha)), global)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        r: ColorChannel,
        g: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSRGB> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSRGB::new_inherited(r, g, b, alpha)),
            global,
            proto,
        )
    }

    pub(crate) fn r_serialized(&self) -> String {
        self.r.borrow().serialize()
    }

    pub(crate) fn g_serialized(&self) -> String {
        self.g.borrow().serialize()
    }

    pub(crate) fn b_serialized(&self) -> String {
        self.b.borrow().serialize()
    }

    pub(crate) fn alpha_serialized(&self) -> String {
        self.alpha.borrow().serialize()
    }
}

impl CSSRGBMethods<crate::DomTypeHolder> for CSSRGB {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrgb-cssrgb>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        r: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        g: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        b: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        alpha: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> Fallible<DomRoot<CSSRGB>> {
        let r = channel_from_union(cx, global, r, ChannelKind::RgbComponent)?;
        let g = channel_from_union(cx, global, g, ChannelKind::RgbComponent)?;
        let b = channel_from_union(cx, global, b, ChannelKind::RgbComponent)?;
        let alpha = channel_from_union(cx, global, alpha, ChannelKind::PercentComponent)?;
        Ok(CSSRGB::new_with_proto(cx, global, proto, r, g, b, alpha))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrgb-r>
    fn GetR(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.r.borrow()))
    }

    fn SetR(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.r.borrow_mut() = channel_from_union(cx, &global, value, ChannelKind::RgbComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrgb-g>
    fn GetG(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.g.borrow()))
    }

    fn SetG(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.g.borrow_mut() = channel_from_union(cx, &global, value, ChannelKind::RgbComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrgb-b>
    fn GetB(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.b.borrow()))
    }

    fn SetB(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.b.borrow_mut() = channel_from_union(cx, &global, value, ChannelKind::RgbComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssrgb-alpha>
    fn GetAlpha(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.alpha.borrow()))
    }

    fn SetAlpha(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.alpha.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::PercentComponent)?;
        Ok(())
    }
}
