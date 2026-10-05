/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletNode
//
// (Bao 段(1) skeleton, user ruling 2026-10-05; 段(3) wiring by e91): the
// node now constructs a REAL graph face — the servo-media AudioWorkletNode
// (e90) is created through `AudioNodeInit::AudioWorkletNode` with a bridge
// made via `make_bridge_with_capacity`, the name is validated against the
// shared processor registry (spec NotSupportedError arm), the declared
// parameters become a real `AudioParamMap` (one `WorkletParam(i)` per
// descriptor), the `port` is routed through the node↔processor conduit, and
// an instantiation task registers the processor + block-rate pump on the
// worklet thread.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dom_struct::dom_struct;
use indexmap::IndexMap;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::reflect_dom_object_with_proto;
use servo_media::audio::audioworklet_node::{
    AudioWorkletNodeError, AudioWorkletNodeInit, AudioWorkletNodeOptions as MediaOptions,
    DEFAULT_BRIDGE_CAPACITY, WorkletParamInit,
};
use servo_media::audio::audio_node::{AudioNodeInit, AudioNodeType};
use servo_media::audio::param::{ParamRate, ParamType};
use stylo_atoms::Atom;

use crate::dom::audio::audioparam::AudioParam;
use crate::dom::audio::audioworklethandler::{ParamDescriptor, instantiate_processor};
use crate::dom::audio::audioworkletport::{
    AudioWorkletPortConduit, PortDirection, PortRedirect,
};
use crate::dom::audio::audioparammap::AudioParamMap;
use crate::dom::audio::audionode::{
    AudioNode, AudioNodeOptionsHelper, MAX_CHANNEL_COUNT,
};
use crate::dom::audio::baseaudiocontext::BaseAudioContext;
use crate::dom::bindings::codegen::Bindings::AudioNodeBinding::{
    ChannelCountMode, ChannelInterpretation,
};
use crate::dom::bindings::codegen::Bindings::AudioParamBinding::AutomationRate;
use crate::dom::bindings::codegen::Bindings::BaseAudioContextBinding::BaseAudioContextMethods;
use crate::dom::bindings::codegen::Bindings::AudioWorkletNodeBinding::{
    AudioWorkletNodeMethods, AudioWorkletNodeOptions,
};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::window::Window;
use crate::dom::{Event, EventBubbles, EventCancelable};
use crate::dom::globalscope::messageport::MessagePort;
use crate::dom::bindings::reflector::DomGlobal;

/// Processors are keyed by node, not by the media NodeId (which has no
/// public integer extraction), so the script face mints its own key.
static NEXT_NODE_KEY: AtomicU64 = AtomicU64::new(1);

/// The plain-data node shape shipped to the instantiation task (Send): port
/// counts, per-output channel counts, and the declared parameters in
/// `parameterDescriptors` order.
pub(crate) struct WorkletNodeShape {
    pub(crate) input_ports: u32,
    pub(crate) output_ports: u32,
    pub(crate) input_channels: u8,
    pub(crate) output_channels: Vec<u8>,
    pub(crate) params: Vec<ParamDescriptor>,
}

#[dom_struct]
pub(crate) struct AudioWorkletNode {
    node: AudioNode,
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-port>
    /// (Bao 段(3)): redirected through the node↔processor conduit — its
    /// `postMessage` payloads travel to the processor realm.
    port: Dom<MessagePort>,
    /// The declared parameter map (`parameters` attribute), one
    /// `WorkletParam(i)` AudioParam per descriptor, in declaration order.
    parameters: Dom<AudioParamMap>,
    /// The node↔processor port conduit (shared with the worklet-side pump).
    #[no_trace = "lock-free rings of plain data; nothing GC-owned inside"]
    #[ignore_malloc_size_of = "lock-free rings, no heap-owned GC payload"]
    conduit: Arc<AudioWorkletPortConduit>,
    /// The bridge half held for the instantiation task (the media node holds
    /// the other half through the render-thread graph).
    #[no_trace = "atomics and lock-free rings only; nothing GC-owned inside"]
    #[ignore_malloc_size_of = "lock-free bridge, no heap-owned GC payload"]
    bridge: Arc<servo_media::audio::audioworklet_node::AudioWorkletBridge>,
    /// The node shape the instantiation task needs (plain data).
    #[no_trace = "plain port-count/descriptor data; no GC pointers"]
    #[ignore_malloc_size_of = "plain descriptors, port counts"]
    shape: WorkletNodeShape,
    /// This node's processor key.
    #[no_trace = "plain integer key, nothing to trace"]
    node_key: u64,
    /// `processorerror` has fired once for this node (spec: fire once).
    processor_error_fired: Cell<bool>,
    /// The DOMString name the node was constructed with.
    name: String,
}

impl AudioWorkletNode {
    #[cfg_attr(crown, expect(crown::unrooted_must_root))]
    fn new_inherited(
        cx: &mut JSContext,
        context: &BaseAudioContext,
        name: DOMString,
        options: &AudioWorkletNodeOptions,
    ) -> Fallible<AudioWorkletNode> {
        // Spec: if outputChannelCount is given, its length must equal
        // numberOfOutputs, else a NotSupportedError.
        let unwrapped = options.parent.unwrap_or(
            2,
            ChannelCountMode::Max,
            ChannelInterpretation::Speakers,
        );
        if let Some(counts) = &options.outputChannelCount {
            if counts.len() as u32 != options.numberOfOutputs {
                return Err(Error::NotSupported(None));
            }
            for count in counts {
                if *count == 0 || *count > MAX_CHANNEL_COUNT {
                    return Err(Error::Range(c"Invalid channel count.".into()));
                }
            }
        }
        // The creating context's AudioWorklet face (SameObject per context):
        // the shared processor registry validates the name (spec
        // NotSupportedError arm) and carries the parameter descriptors.
        let audio_worklet = context.AudioWorklet(cx);
        let name_atom = Atom::from(name.clone());
        let params: Vec<ParamDescriptor> = {
            let registry = audio_worklet.registry();
            let registry = registry
                .lock()
                .expect("Locking the shared processor registry");
            match registry.get(&name_atom) {
                Some(params) => params.clone(),
                None => return Err(Error::NotSupported(None)),
            }
        };

        // Media node options (e90 face) + bridge. `validate()` covers the
        // remaining spec shape checks; its channel-count errors are
        // RangeErrors, the (already-checked) port mismatch NotSupported.
        let media_options = MediaOptions {
            number_of_inputs: options.numberOfInputs,
            number_of_outputs: options.numberOfOutputs,
            output_channel_count: options
                .outputChannelCount
                .as_ref()
                .map(|counts| counts.iter().map(|count| *count as u8).collect())
                .unwrap_or_default(),
            input_channel_count: unwrapped.count as u8,
            params: params
                .iter()
                .map(|param| WorkletParamInit {
                    default_value: param.default_value,
                    rate: param.rate,
                })
                .collect(),
        };
        if let Err(err) = media_options.validate() {
            return Err(match err {
                AudioWorkletNodeError::OutputChannelCountMismatch => Error::NotSupported(None),
                AudioWorkletNodeError::InvalidChannelCount(_) => {
                    Error::Range(c"Invalid channel count.".into())
                },
            });
        }
        let shape = WorkletNodeShape {
            input_ports: options.numberOfInputs,
            output_ports: options.numberOfOutputs,
            input_channels: unwrapped.count as u8,
            output_channels: media_output_channels(&media_options),
            params: params.clone(),
        };
        let bridge = Arc::new(media_options.make_bridge_with_capacity(DEFAULT_BRIDGE_CAPACITY));

        // The real graph face: e90's render-side AudioWorkletNode consumes
        // this bridge. A `None` node id means servo-media could not host the
        // node — the engine's own inert-node degradation (warned in
        // audionode.rs), not a script-visible failure.
        let node = AudioNode::new_inherited(
            cx,
            AudioNodeInit::AudioWorkletNode(AudioWorkletNodeInit {
                options: media_options,
                bridge: bridge.clone(),
            }),
            context,
            unwrapped,
            options.numberOfInputs,
            options.numberOfOutputs,
        )?;

        // One AudioParam per declared parameter: entry `i` automates
        // `ParamType::WorkletParam(i)` through the regular automation
        // timeline (the name→index mapping this node carries).
        let global = context.global();
        let mut param_map = IndexMap::new();
        for (index, param) in params.iter().enumerate() {
            let automation_rate = match param.rate {
                ParamRate::KRate => AutomationRate::K_rate,
                ParamRate::ARate => AutomationRate::A_rate,
            };
            let audio_param = AudioParam::new(
                cx,
                &global,
                context,
                node.node_id(),
                AudioNodeType::AudioWorkletNode,
                ParamType::WorkletParam(index as u32),
                automation_rate,
                param.default_value,
                param.min_value,
                param.max_value,
            );
            param_map.insert(DOMString::from(param.name.clone()), audio_param);
        }
        let parameters = AudioParamMap::new(cx, &global, param_map);

        // The node↔processor port conduit: this port posts into the
        // `to_processor` ring (the worklet wake hook drains it); the
        // processor's port posts back into `to_main`, delivered by the
        // `notify_main` task.
        let port = MessagePort::new(cx, &global);
        let conduit = AudioWorkletPortConduit::new();
        port.set_bao_port_redirect(PortRedirect {
            conduit: conduit.clone(),
            direction: PortDirection::ToProcessor,
        });
        let node_key = NEXT_NODE_KEY.fetch_add(1, Ordering::Relaxed);
        Ok(AudioWorkletNode {
            node,
            port: Dom::from_ref(&*port),
            parameters: Dom::from_ref(&*parameters),
            conduit,
            bridge,
            shape,
            node_key,
            processor_error_fired: Cell::new(false),
            name: name.into(),
        })
    }

    /// Wire the worklet half once this node is reflected: install the
    /// `notify_main` hook (script-thread drain task for the node's port) and
    /// ship the instantiation task (processor construction, persistent
    /// arrays, block-rate pump) to the worklet thread. Called exactly once,
    /// from the constructor, after reflection — the `Trusted` handles below
    /// need the rooted DOM object.
    pub(crate) fn wire_processor(&self, cx: &mut JSContext, audio_worklet: &crate::dom::audio::audioworklet::AudioWorklet) {
        let global = self.global();
        {
            // `Trusted` and `SendableTaskSource` are Send but not Sync; the
            // notify hook is a `Send + Sync` closure, so both ride in mutexes
            // and are taken out per notification.
            let task_source = Arc::new(Mutex::new(
                global.task_manager().port_message_queue().to_sendable(),
            ));
            let conduit_for_task = self.conduit.clone();
            let node_for_task = Arc::new(Mutex::new(Trusted::<AudioWorkletNode>::new(self)));
            self.conduit.set_notify_main(Arc::new(move || {
                let conduit = conduit_for_task.clone();
                let node = node_for_task.lock().expect("Locking the port node handle").clone();
                let task_source = task_source.lock().expect("Locking the port task source");
                task_source.queue(task!(audioworklet_port_drain: move |cx| {
                    let node = node.root();
                    node.drain_inbound_port(cx, &conduit);
                }));
            }));
        }

        let main_sender = global.as_window().main_thread_script_chan().clone();
        let task_node: Trusted<AudioWorkletNode> = Trusted::new(self);
        let task_name = Atom::from(self.name.clone());
        let task_bridge = self.bridge.clone();
        let task_conduit = self.conduit.clone();
        let task_shape = WorkletNodeShape {
            input_ports: self.shape.input_ports,
            output_ports: self.shape.output_ports,
            input_channels: self.shape.input_channels,
            output_channels: self.shape.output_channels.clone(),
            params: self.shape.params.clone(),
        };
        let node_key = self.node_key;
        audio_worklet.perform_a_worklet_task(Box::new(
            move |cx: &mut JSContext, scope: &crate::dom::workletglobalscope::WorkletGlobalScope| {
                let Some(audio_scope) = scope.downcast::<
                    crate::dom::audio::audioworkletglobalscope::AudioWorkletGlobalScope,
                >() else {
                    return;
                };
                let Some(ctor) = audio_scope.processor_ctor(&task_name) else {
                    log::warn!(
                        "AudioWorklet processor {task_name} not registered on the worklet scope"
                    );
                    return;
                };
                rooted!(&in(cx) let ctor_value = ctor);
                instantiate_processor(
                    cx,
                    audio_scope,
                    node_key,
                    ctor_value.handle(),
                    task_node,
                    task_bridge,
                    task_conduit,
                    &task_shape,
                    main_sender,
                );
            },
        ));
    }

    /// Fire the `processorerror` event once for this node (the spec fires it
    /// a single time; both the worklet-side handler and the instantiation
    /// failure path funnel through here).
    pub(crate) fn fire_processorerror_once(&self, cx: &mut JSContext) {
        if self.processor_error_fired.get() {
            return;
        }
        self.processor_error_fired.set(true);
        let event = Event::new(
            cx,
            &self.global(),
            Atom::from("processorerror"),
            EventBubbles::DoesNotBubble,
            EventCancelable::NotCancelable,
        );
        event.upcast::<Event>().fire(cx, self.upcast());
    }

    /// Drain the conduit's `to_main` ring on the script thread, dispatching
    /// `message` events on this node's port (the `notify_main` hook body).
    pub(crate) fn drain_inbound_port(
        &self,
        cx: &mut JSContext,
        conduit: &Arc<AudioWorkletPortConduit>,
    ) {
        while let Some(payload) = conduit.pop_for_main() {
            let global = self.global();
            crate::dom::audio::audioworkletport::dispatch_port_payload(
                cx,
                &self.port,
                &global,
                payload,
            );
        }
    }
}

fn media_output_channels(options: &MediaOptions) -> Vec<u8> {
    (0..options.number_of_outputs)
        .map(|port| {
            options
                .output_channel_count
                .get(port as usize)
                .copied()
                .unwrap_or(options.input_channel_count)
        })
        .collect()
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
        let node = AudioWorkletNode::new_inherited(cx, context, name, options)?;
        let dom_root = reflect_dom_object_with_proto(
            cx,
            Box::new(node),
            window,
            proto,
        );
        let audio_worklet = context.AudioWorklet(cx);
        dom_root.wire_processor(cx, &audio_worklet);
        Ok(dom_root)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-port>
    fn Port(&self) -> DomRoot<MessagePort> {
        DomRoot::from_ref(&*self.port)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-parameters>
    fn Parameters(&self) -> DomRoot<AudioParamMap> {
        DomRoot::from_ref(&*self.parameters)
    }

    // https://webaudio.github.io/web-audio-api/#dom-audioworkletnode-onprocessorerror
    event_handler!(processorerror, GetOnprocessorerror, SetOnprocessorerror);
}
