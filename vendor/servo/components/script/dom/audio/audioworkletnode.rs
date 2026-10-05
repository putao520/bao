/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletNode
//
// (Bao 段(1), user ruling 2026-10-05): upstream servo has zero AudioWorklet
// runtime. The node constructs as an INERT AudioNode — `node_id = None` is
// the engine's own "no backend" degradation form (audionode.rs
// `new_inherited` warns and keeps an inert node when servo-media cannot
// host the node type); servo-media gains its AudioWorklet node type in the
// 段(2) servo-media contract, which also flips this node to a real graph
// face. `parameters` (AudioParamMap, k-rate/a-rate) is likewise 段(2).

use dom_struct::dom_struct;
use js::context::JSContext;
use log::warn;
use js::rust::HandleObject;
use crate::dom::bindings::reflector::DomGlobal;
use script_bindings::reflector::reflect_dom_object_with_proto;

use crate::dom::audio::audionode::{
    AudioNode, AudioNodeOptionsHelper, MAX_CHANNEL_COUNT,
};
use crate::dom::audio::baseaudiocontext::BaseAudioContext;
use crate::dom::bindings::codegen::Bindings::AudioNodeBinding::{
    ChannelCountMode, ChannelInterpretation,
};
use crate::dom::bindings::codegen::Bindings::AudioWorkletNodeBinding::{
    AudioWorkletNodeMethods, AudioWorkletNodeOptions,
};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::window::Window;
use crate::dom::globalscope::messageport::MessagePort;

#[dom_struct]
pub(crate) struct AudioWorkletNode {
    node: AudioNode,
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-port>
    /// 段(1): a real, un-entangled port (the entangled pair with the
    /// processor-side port is created when the 段(2) render bridge starts
    /// instantiating processors).
    port: Dom<MessagePort>,
}

impl AudioWorkletNode {
    #[cfg_attr(crown, expect(crown::unrooted_must_root))]
    fn new_inherited(
        cx: &mut JSContext,
        context: &BaseAudioContext,
        _name: DOMString,
        options: &AudioWorkletNodeOptions,
    ) -> Fallible<AudioWorkletNode> {
        // Spec: if outputChannelCount is given, its length must equal
        // numberOfOutputs, else a NotSupportedError.
        let mut channel_count = options.parent.unwrap_or(
            2,
            ChannelCountMode::Max,
            ChannelInterpretation::Speakers,
        );
        if let Some(counts) = &options.outputChannelCount {
            if counts.len() as u32 != options.numberOfOutputs {
                return Err(Error::NotSupported(None));
            }
            if let Some(first) = counts.first() {
                if *first == 0 || *first > MAX_CHANNEL_COUNT {
                    return Err(Error::NotSupported(None));
                }
                channel_count.count = *first;
            }
        }
        // 段(1) boundary: servo-media has no AudioWorklet node type yet (the
        // 段(2) servo-media contract adds AudioNodeType::AudioWorklet and the
        // render bridge). Construct the node INERT — node_id = None is the
        // engine's own no-backend degradation form, the same shape
        // `AudioNode::new_inherited` produces when the backend cannot host a
        // node. The warning keeps the degradation observable.
        warn!(
            "AudioWorkletNode backend not wired until the servo-media bridge \
             (段(2)): the constructed AudioWorkletNode will be inert."
        );
        let node = AudioNode::new_inherited_for_id(
            None,
            context,
            channel_count,
            options.numberOfInputs,
            options.numberOfOutputs,
        );
        let global = context.global();
        let port = MessagePort::new(cx, &global);
        Ok(AudioWorkletNode {
            node,
            port: Dom::from_ref(&port),
        })
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        window: &Window,
        context: &BaseAudioContext,
        name: DOMString,
        options: &AudioWorkletNodeOptions,
    ) -> Fallible<DomRoot<AudioWorkletNode>> {
        Self::new_with_proto(cx, window, None, context, name, options)
    }

    #[cfg_attr(crown, expect(crown::unrooted_must_root))]
    fn new_with_proto(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        context: &BaseAudioContext,
        name: DOMString,
        options: &AudioWorkletNodeOptions,
    ) -> Fallible<DomRoot<AudioWorkletNode>> {
        let node = AudioWorkletNode::new_inherited(cx, context, name, options)?;
        Ok(reflect_dom_object_with_proto(
            cx,
            Box::new(node),
            window,
            proto,
        ))
    }
}

impl AudioWorkletNodeMethods<crate::DomTypeHolder> for AudioWorkletNode {
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-audioworkletnode>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        context: &BaseAudioContext,
        name: DOMString,
        options: &AudioWorkletNodeOptions,
    ) -> Fallible<DomRoot<AudioWorkletNode>> {
        AudioWorkletNode::new_with_proto(cx, window, proto, context, name, options)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-port>
    fn Port(&self) -> DomRoot<MessagePort> {
        DomRoot::from_ref(&*self.port)
    }

    // https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-onprocessorerror
    event_handler!(processorerror, GetOnprocessorerror, SetOnprocessorerror);
}


