/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioParamMap
//
// (Bao 段(3) wiring, user ruling 2026-10-05 "自研吧"): upstream servo has
// no AudioParamMap (no AudioWorklet at all — see AudioParamMap.webidl). The
// name→AudioParam map of an AudioWorkletNode: one entry per declared
// `parameterDescriptors` item, in declaration order; entry `i` automates the
// media-side `ParamType::WorkletParam(i)` of the node's servo-media graph
// node (the name→index mapping this type carries, consumed by e90's param
// face through the regular AudioParam automation timeline).

use dom_struct::dom_struct;
use indexmap::IndexMap;
use js::context::JSContext;
use script_bindings::cell::DomRefCell;
use script_bindings::like::Maplike;
use script_bindings::reflector::{Reflector, reflect_dom_object};

use crate::dom::GlobalScope;
use crate::dom::audio::audioparam::AudioParam;
use crate::dom::bindings::codegen::Bindings::AudioParamMapBinding::AudioParamMapMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::maplike;

/// <https://webaudio.github.io/web-audio-api/#AudioParamMap>
#[dom_struct]
pub(crate) struct AudioParamMap {
    reflector: Reflector,

    /// The name→AudioParam map, keyed in `parameterDescriptors` order.
    #[custom_trace]
    internal: DomRefCell<IndexMap<DOMString, DomRoot<AudioParam>>>,
}

impl AudioParamMap {
    fn new_inherited(map: IndexMap<DOMString, DomRoot<AudioParam>>) -> AudioParamMap {
        AudioParamMap {
            reflector: Reflector::new(),
            internal: DomRefCell::new(map),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        map: IndexMap<DOMString, DomRoot<AudioParam>>,
    ) -> DomRoot<AudioParamMap> {
        reflect_dom_object(cx, Box::new(AudioParamMap::new_inherited(map)), global)
    }
}

impl Maplike for AudioParamMap {
    type Key = DOMString;
    type Value = DomRoot<AudioParam>;

    maplike!(self, internal);
}

impl AudioParamMapMethods<crate::DomTypeHolder> for AudioParamMap {
    fn Size(&self) -> u32 {
        self.internal.borrow().len() as u32
    }
}
