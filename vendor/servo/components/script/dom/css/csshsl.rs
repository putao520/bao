/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#csshsl>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSHSLMethods;
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
pub(crate) struct CSSHSL {
    css_color_value: CSSColorValue,
    h: DomRefCell<ColorChannel>,
    s: DomRefCell<ColorChannel>,
    l: DomRefCell<ColorChannel>,
    alpha: DomRefCell<ColorChannel>,
}

impl CSSHSL {
    fn new_inherited(
        h: ColorChannel,
        s: ColorChannel,
        l: ColorChannel,
        alpha: ColorChannel,
    ) -> CSSHSL {
        CSSHSL {
            css_color_value: CSSColorValue::new_inherited(),
            h: DomRefCell::new(h),
            s: DomRefCell::new(s),
            l: DomRefCell::new(l),
            alpha: DomRefCell::new(alpha),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        h: ColorChannel,
        s: ColorChannel,
        l: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSHSL> {
        reflect_dom_object(cx, Box::new(CSSHSL::new_inherited(h, s, l, alpha)), global)
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        h: ColorChannel,
        s: ColorChannel,
        l: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSHSL> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSHSL::new_inherited(h, s, l, alpha)),
            global,
            proto,
        )
    }

    pub(crate) fn h_serialized(&self) -> String {
        self.h.borrow().serialize()
    }

    pub(crate) fn s_serialized(&self) -> String {
        self.s.borrow().serialize()
    }

    pub(crate) fn l_serialized(&self) -> String {
        self.l.borrow().serialize()
    }

    pub(crate) fn alpha_serialized(&self) -> String {
        self.alpha.borrow().serialize()
    }
}

impl CSSHSLMethods<crate::DomTypeHolder> for CSSHSL {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshsl-csshsl>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        h: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        s: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        l: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        alpha: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> Fallible<DomRoot<CSSHSL>> {
        let h = channel_from_union(cx, global, h, ChannelKind::AngleComponent)?;
        let s = channel_from_union(cx, global, s, ChannelKind::PercentComponent)?;
        let l = channel_from_union(cx, global, l, ChannelKind::PercentComponent)?;
        let alpha = channel_from_union(cx, global, alpha, ChannelKind::PercentComponent)?;
        Ok(CSSHSL::new_with_proto(cx, global, proto, h, s, l, alpha))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshsl-h>
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

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshsl-s>
    fn GetS(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.s.borrow()))
    }

    fn SetS(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.s.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::PercentComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshsl-l>
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

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-csshsl-alpha>
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
