/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssoklch>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSOKLCHMethods;
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
pub(crate) struct CSSOKLCH {
    css_color_value: CSSColorValue,
    l: DomRefCell<ColorChannel>,
    c: DomRefCell<ColorChannel>,
    h: DomRefCell<ColorChannel>,
    alpha: DomRefCell<ColorChannel>,
}

impl CSSOKLCH {
    fn new_inherited(
        l: ColorChannel,
        c: ColorChannel,
        h: ColorChannel,
        alpha: ColorChannel,
    ) -> CSSOKLCH {
        CSSOKLCH {
            css_color_value: CSSColorValue::new_inherited(),
            l: DomRefCell::new(l),
            c: DomRefCell::new(c),
            h: DomRefCell::new(h),
            alpha: DomRefCell::new(alpha),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        l: ColorChannel,
        c: ColorChannel,
        h: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSOKLCH> {
        reflect_dom_object(
            cx,
            Box::new(CSSOKLCH::new_inherited(l, c, h, alpha)),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        l: ColorChannel,
        c: ColorChannel,
        h: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSOKLCH> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSOKLCH::new_inherited(l, c, h, alpha)),
            global,
            proto,
        )
    }

    pub(crate) fn l_serialized(&self) -> String {
        self.l.borrow().serialize()
    }

    pub(crate) fn c_serialized(&self) -> String {
        self.c.borrow().serialize()
    }

    pub(crate) fn h_serialized(&self) -> String {
        self.h.borrow().serialize()
    }
}

impl CSSOKLCHMethods<crate::DomTypeHolder> for CSSOKLCH {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklch-cssoklch>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        l: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        c: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        h: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        alpha: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> Fallible<DomRoot<CSSOKLCH>> {
        let l = channel_from_union(cx, global, l, ChannelKind::PercentComponent)?;
        let c = channel_from_union(cx, global, c, ChannelKind::PercentComponent)?;
        let h = channel_from_union(cx, global, h, ChannelKind::AngleComponent)?;
        let alpha = channel_from_union(cx, global, alpha, ChannelKind::PercentComponent)?;
        Ok(CSSOKLCH::new_with_proto(cx, global, proto, l, c, h, alpha))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklch-l>
    fn GetL(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.l.borrow()))
    }

    fn SetL(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.l.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::PercentComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklch-c>
    fn GetC(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.c.borrow()))
    }

    fn SetC(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.c.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::PercentComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklch-h>
    fn GetH(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.h.borrow()))
    }

    fn SetH(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.h.borrow_mut() = channel_from_union(cx, &global, value, ChannelKind::AngleComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklch-alpha>
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
