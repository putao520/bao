/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletProcessor
//
// (Bao 段(1), user ruling 2026-10-05): upstream servo has zero AudioWorklet
// runtime. 段(1) delivers the interface + constructor + port face inside the
// AudioWorklet realm; instantiating processors per render block (the
// AudioWorkletNode render bridge pulling registered constructors from the
// scope's processor registry) is the 段(2) servo-media contract.

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};

use crate::dom::bindings::codegen::Bindings::AudioWorkletProcessorBinding::AudioWorkletProcessorMethods;
use crate::dom::audio::audioworkletglobalscope::AudioWorkletGlobalScope;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{Dom, DomRoot};
use js::jsapi::JSObject;

use crate::dom::globalscope::GlobalScope;
use crate::dom::globalscope::messageport::MessagePort;

#[dom_struct]
pub(crate) struct AudioWorkletProcessor {
    reflector: Reflector,
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletprocessor-port>
    /// 段(1): a real, un-entangled port; the entangled node-side pair is
    /// created when the 段(2) render bridge starts instantiating processors.
    port: Dom<MessagePort>,
}

impl AudioWorkletProcessor {
    fn new_inherited(port: &MessagePort) -> AudioWorkletProcessor {
        AudioWorkletProcessor {
            reflector: Reflector::new(),
            port: Dom::from_ref(port),
        }
    }

    #[cfg_attr(crown, expect(crown::unrooted_must_root))]
    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
    ) -> DomRoot<AudioWorkletProcessor> {
        let port = MessagePort::new(cx, global);
        reflect_dom_object_with_proto(
            cx,
            Box::new(AudioWorkletProcessor::new_inherited(&port)),
            global,
            proto,
        )
    }
}

impl AudioWorkletProcessorMethods<crate::DomTypeHolder> for AudioWorkletProcessor {
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletprocessor-audioworkletprocessor>
    fn Constructor(
        cx: &mut JSContext,
        global: &AudioWorkletGlobalScope,
        proto: Option<HandleObject>,
        _options: Option<*mut JSObject>,
    ) -> DomRoot<AudioWorkletProcessor> {
        AudioWorkletProcessor::new(cx, global.upcast(), proto)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletprocessor-port>
    fn Port(&self) -> DomRoot<MessagePort> {
        DomRoot::from_ref(&*self.port)
    }
}

