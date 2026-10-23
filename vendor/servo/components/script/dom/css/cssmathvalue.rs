/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://drafts.css-houdini.org/css-typed-om-1/#cssmathvalue>

use crate::dom::bindings::codegen::Bindings::CSSNumericValueBinding::{
    CSSMathOperator, CSSMathValueMethods,
};
use dom_struct::dom_struct;

use super::cssmathclamp::CSSMathClamp;
use super::cssmathinvert::CSSMathInvert;
use super::cssmathmax::CSSMathMax;
use super::cssmathmin::CSSMathMin;
use super::cssmathnegate::CSSMathNegate;
use super::cssmathproduct::CSSMathProduct;
use super::cssmathsum::CSSMathSum;
use super::cssnumericvalue::CSSNumericValue;
use crate::dom::bindings::inheritance::Castable;

#[dom_struct]
pub(crate) struct CSSMathValue {
    css_numeric_value: CSSNumericValue,
}

impl CSSMathValue {
    pub(crate) fn new_inherited() -> CSSMathValue {
        CSSMathValue {
            css_numeric_value: CSSNumericValue::new_inherited(),
        }
    }
}

impl CSSMathValueMethods<crate::DomTypeHolder> for CSSMathValue {
    /// <https://drafts.css-houdini.org/css-typed-om-1/#dom-cssmathvalue-operator>
    fn Operator(&self) -> CSSMathOperator {
        if self.is::<CSSMathSum>() {
            CSSMathOperator::Sum
        } else if self.is::<CSSMathProduct>() {
            CSSMathOperator::Product
        } else if self.is::<CSSMathNegate>() {
            CSSMathOperator::Negate
        } else if self.is::<CSSMathInvert>() {
            CSSMathOperator::Invert
        } else if self.is::<CSSMathMin>() {
            CSSMathOperator::Min
        } else if self.is::<CSSMathMax>() {
            CSSMathOperator::Max
        } else {
            debug_assert!(self.is::<CSSMathClamp>());
            CSSMathOperator::Clamp
        }
    }
}
