/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssoklab>

use crate::dom::bindings::codegen::Bindings::CSSColorValueBinding::CSSOKLabMethods;
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
pub(crate) struct CSSOKLab {
    css_color_value: CSSColorValue,
    l: DomRefCell<ColorChannel>,
    a: DomRefCell<ColorChannel>,
    b: DomRefCell<ColorChannel>,
    alpha: DomRefCell<ColorChannel>,
}

impl CSSOKLab {
    fn new_inherited(
        l: ColorChannel,
        a: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> CSSOKLab {
        CSSOKLab {
            css_color_value: CSSColorValue::new_inherited(),
            l: DomRefCell::new(l),
            a: DomRefCell::new(a),
            b: DomRefCell::new(b),
            alpha: DomRefCell::new(alpha),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        l: ColorChannel,
        a: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSOKLab> {
        reflect_dom_object(
            cx,
            Box::new(CSSOKLab::new_inherited(l, a, b, alpha)),
            global,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        l: ColorChannel,
        a: ColorChannel,
        b: ColorChannel,
        alpha: ColorChannel,
    ) -> DomRoot<CSSOKLab> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(CSSOKLab::new_inherited(l, a, b, alpha)),
            global,
            proto,
        )
    }

    pub(crate) fn l_serialized(&self) -> String {
        self.l.borrow().serialize()
    }

    pub(crate) fn a_serialized(&self) -> String {
        self.a.borrow().serialize()
    }

    pub(crate) fn b_serialized(&self) -> String {
        self.b.borrow().serialize()
    }
}

impl CSSOKLabMethods<crate::DomTypeHolder> for CSSOKLab {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklab-cssoklab>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        l: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        a: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        b: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
        alpha: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> Fallible<DomRoot<CSSOKLab>> {
        let l = channel_from_union(cx, global, l, ChannelKind::PercentComponent)?;
        let a = channel_from_union(cx, global, a, ChannelKind::NumberComponent)?;
        let b = channel_from_union(cx, global, b, ChannelKind::NumberComponent)?;
        let alpha = channel_from_union(cx, global, alpha, ChannelKind::PercentComponent)?;
        Ok(CSSOKLab::new_with_proto(cx, global, proto, l, a, b, alpha))
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklab-l>
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

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklab-a>
    fn GetA(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.a.borrow()))
    }

    fn SetA(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.a.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::NumberComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklab-b>
    fn GetB(&self) -> Fallible<DoubleOrCSSNumericValueOrCSSKeywordValueOrString> {
        Ok(channel_to_union(&self.b.borrow()))
    }

    fn SetB(
        &self,
        cx: &mut JSContext,
        value: DoubleOrCSSNumericValueOrCSSKeywordValueOrString,
    ) -> ErrorResult {
        let global = self.global();
        *self.b.borrow_mut() =
            channel_from_union(cx, &global, value, ChannelKind::NumberComponent)?;
        Ok(())
    }

    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssoklab-alpha>
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
