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
use js::conversions::{jsstr_to_string, ToJSValConvertible};
use js::gc::CustomAutoRooter;
use js::jsapi::{ESClass, JSITER_OWNONLY, JSObject, JSPROP_ENUMERATE};
use js::jsval::{JSVal, NullValue, ObjectValue, UndefinedValue};
use js::rust::wrappers2::{
    GetArrayLength, GetBuiltinClass, GetPropertyKeys, JS_ClearPendingException, JS_DefineElement,
    JS_DefineProperty, JS_GetElement, JS_GetPropertyById, JS_IdToValue, JS_NewObject,
    NewArrayObject1,
};
use js::rust::{CustomAutoRooterGuard, HandleObject, IdVector};
use rustc_hash::FxHashMap;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_proto;
use servo_constellation_traits::StructuredSerializedData;
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
use crate::dom::bindings::conversions::root_from_object;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::structuredclone;
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;
use crate::dom::window::Window;
use crate::dom::event::errorevent::ErrorEvent;
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

/// One step of a [`PortPath`]: a named own property of a plain object, or an
/// element index of an array. Plain data (`Send`) — travels with the
/// instantiation task so the worklet thread can place each minted lane port
/// back at the position the substituted `MessagePort` occupied.
#[derive(Clone, Debug)]
pub(crate) enum PortPathSeg {
    Key(Box<str>),
    Index(u32),
}

/// The path from the options-object root to one substituted `MessagePort`
/// position inside `processorOptions` (plain data, `Send`).
pub(crate) type PortPath = Vec<PortPathSeg>;

/// One substituted `MessagePort`: the page's port object plus every position
/// it occupied in the options graph (the same object at two positions is one
/// slot with two paths — identity is preserved by minting one lane port and
/// placing it at each path).
pub(crate) struct OptionPortSlot {
    pub(crate) port: DomRoot<MessagePort>,
    pub(crate) paths: Vec<PortPath>,
}

/// (e114) The options record shipped to the instantiation task: the
/// structured-clone bytes of the options dictionary (spec step 10) plus the
/// positions of any substituted `processorOptions` ports. Plain data
/// (`Send`).
pub(crate) type SerializedOptions = (StructuredSerializedData, Vec<Vec<PortPath>>);

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
    /// (e114) The main-side endpoints of the `processorOptions` port lanes:
    /// the page's own port objects, redirected into this node's conduit.
    /// Kept alive here so the `notify_main` drain can dispatch on them.
    lane_ports: DomRefCell<Vec<Dom<MessagePort>>>,
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
    ) -> Fallible<(AudioWorkletNode, SerializedOptions)> {
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
        let audio_worklet = context.AudioWorklet();
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

        // (e114) spec §AudioWorkletNode-constructors steps 9-10: convert the
        // options dictionary to a JS object and StructuredSerialize it — the
        // worklet thread's "invoking processor constructor" step 5
        // deserializes this record into the constructor's single argument.
        // A `MessagePort` inside `processorOptions` (the Transferable
        // subface) cannot ride the generic constellation port-router path
        // (worklet event loops have no port delivery — the documented
        // limitation the node↔processor conduit exists around), so it is
        // substituted out for a null placeholder here, its position
        // recorded, and the worklet thread mints the counterpart lane
        // endpoint at instantiation. See `substitute_option_ports`.
        rooted!(&in(cx) let mut options_object = UndefinedValue());
        options.to_jsval(cx, options_object.handle_mut());
        let (serialized_options, option_slots): (_, Vec<OptionPortSlot>) =
            match structuredclone::write(cx, options_object.handle(), None) {
                Ok(data) => (data, Vec::new()),
                Err(Error::DataClone(_)) => {
                    // A Transferable (a MessagePort) sits somewhere inside
                    // `processorOptions`: substitute it out and serialize the
                    // substituted graph. A second DataCloneError from this
                    // write (e.g. a port hidden inside a Map) propagates —
                    // the construction fails loudly, DataCloneError-shaped.
                    rooted!(&in(cx) let mut substituted = UndefinedValue());
                    let slots = substitute_option_ports(
                        cx,
                        options_object.handle(),
                        substituted.handle_mut(),
                    )?;
                    let data = structuredclone::write(cx, substituted.handle(), None)?;
                    (data, slots)
                },
                Err(err) => return Err(err),
            };
        let option_port_paths: Vec<Vec<PortPath>> = option_slots
            .iter()
            .map(|slot| slot.paths.clone())
            .collect();
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
        let window = global.as_window();
        let mut param_map = IndexMap::new();
        for (index, param) in params.iter().enumerate() {
            let automation_rate = match param.rate {
                ParamRate::KRate => AutomationRate::K_rate,
                ParamRate::ARate => AutomationRate::A_rate,
            };
            let audio_param = AudioParam::new(
                cx,
                window.upcast::<GlobalScope>(),
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
        // Register the node's port in the main global's port registry:
        // `port.onmessage = ...` runs the spec's implicit `start()` on the
        // port, and an unregistered port would panic once the global manages
        // ANY port (e.g. the page also created a MessageChannel). The node's
        // own traffic is fully redirected into the conduit, so the registry
        // entry only serves the start/close bookkeeping.
        global.track_message_port(&port, None);
        let conduit = AudioWorkletPortConduit::with_lane_count(1 + option_slots.len() as u32);
        port.set_bao_port_redirect(PortRedirect {
            conduit: conduit.clone(),
            direction: PortDirection::ToProcessor { lane: 0 },
        });
        // Each substituted `processorOptions` port becomes the main-side
        // endpoint of its own conduit lane: its `postMessage` feeds the
        // worklet-thread counterpart minted at instantiation, and the lane's
        // `to_main` ring dispatches `message` events back on this object.
        let mut lane_ports = Vec::with_capacity(option_slots.len());
        for (index, slot) in option_slots.iter().enumerate() {
            let lane = index as u32 + 1;
            slot.port.set_bao_port_redirect(PortRedirect {
                conduit: conduit.clone(),
                direction: PortDirection::ToProcessor { lane },
            });
            lane_ports.push(Dom::from_ref(&*slot.port));
        }
        let node_key = NEXT_NODE_KEY.fetch_add(1, Ordering::Relaxed);
        let built = AudioWorkletNode {
            node,
            port: Dom::from_ref(&*port),
            parameters: Dom::from_ref(&*parameters),
            conduit,
            bridge,
            shape,
            node_key,
            lane_ports: DomRefCell::new(lane_ports),
            processor_error_fired: Cell::new(false),
            name: name.into(),
        };
        Ok((built, (serialized_options, option_port_paths)))
    }

    /// Wire the worklet half once this node is reflected: install the
    /// `notify_main` hook (script-thread drain task for the node's port) and
    /// ship the instantiation task (processor construction, persistent
    /// arrays, block-rate pump) to the worklet thread. Called exactly once,
    /// from the constructor, after reflection — the `Trusted` handles below
    /// need the rooted DOM object.
    pub(crate) fn wire_processor(
        &self,
        _cx: &mut JSContext,
        audio_worklet: &crate::dom::audio::audioworklet::AudioWorklet,
        serialized_options: SerializedOptions,
    ) {
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
        let task_options = serialized_options;
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
                    task_options,
                    main_sender,
                );
            },
        ));
    }

    /// Fire the `processorerror` event once for this node (the spec fires it
    /// a single time; both the worklet-side handler and the instantiation
    /// failure path funnel through here).
    pub(crate) fn fire_processorerror_once(
        &self,
        cx: &mut JSContext,
        info: Option<crate::dom::bindings::error::ErrorInfo>,
    ) {
        if self.processor_error_fired.get() {
            return;
        }
        self.processor_error_fired.set(true);
        // (e122) The spec's `processorerror` is an ErrorEvent: it carries the
        // captured throw site (message/filename/lineno/colno) of the
        // processor failure. The `error` property stays undefined — the
        // exception value itself lives in the worklet realm and cannot
        // cross (the pre-activation cross-realm rule).
        let (message, filename, lineno, colno) = info
            .map(|info| (info.message, info.filename, info.lineno, info.column))
            .unwrap_or_default();
        rooted!(&in(cx) let error_value = UndefinedValue());
        let event = ErrorEvent::new(
            cx,
            &self.global(),
            Atom::from("processorerror"),
            EventBubbles::DoesNotBubble,
            EventCancelable::NotCancelable,
            message.into(),
            filename.into(),
            lineno,
            colno,
            error_value.handle(),
        );
        event.upcast::<Event>().fire(cx, self.upcast());
    }

    /// Drain the conduit's `to_main` rings on the script thread, dispatching
    /// `message` events on this node's port (lane 0) and on each
    /// `processorOptions` lane's main-side endpoint port (the `notify_main`
    /// hook body).
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
        // Collect under a short borrow (dispatch runs script), then fire.
        let mut lane_payloads = Vec::new();
        {
            let lanes = self.lane_ports.borrow();
            for (index, port) in lanes.iter().enumerate() {
                let lane = index as u32 + 1;
                while let Some(payload) = conduit.pop_for_main_lane(lane) {
                    lane_payloads.push((DomRoot::from_ref(&**port), payload));
                }
            }
        }
        for (port, payload) in lane_payloads {
            let global = self.global();
            crate::dom::audio::audioworkletport::dispatch_port_payload(
                cx,
                &port,
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

// ── (e114) processorOptions Transferable substitution ──

/// How a walked value is placed into the substituted clone: a primitive
/// (relocation-free `JSVal` copy), an object held in the walk's auto-rooter
/// (index into the rooter's vector — the pointer is re-read at every use so
/// a moving GC inside a getter cannot stale it), or the null placeholder
/// written at a substituted `MessagePort` position.
enum Placement {
    Value(JSVal),
    Object(usize),
    Null,
}

/// Soft bounds of the substitution walk (mirrors the bounded-walk
/// discipline of the surrounding audio code): a port-free options graph of
/// any size never enters this path (the plain `structuredclone::write`
/// handles it first), so these only cap the port-carrying case.
const MAX_SUBSTITUTION_DEPTH: usize = 256;
const MAX_SUBSTITUTION_OBJECTS: usize = 8192;

/// Walk the converted options graph — the same plain-object/array domain
/// SM's own structured-clone traversal covers — building a substituted
/// clone in which every `MessagePort` is replaced by `null`, and recording
/// each port's position(s). Getters therefore run exactly once per property
/// (the follow-up `structuredclone::write` reads the substituted clone, not
/// the original graph). Non-plain containers (Date, Map, TypedArray, ...) are
/// copied by reference: a port hidden inside one cannot be substituted, and
/// the follow-up write then fails loudly (`DataCloneError`).
///
/// This is the DOM-layer expression of structured-clone transfer
/// placeholder mechanics (SM's `SCTAG_TRANSFER_MAP_PENDING_ENTRY` is the
/// engine-layer equivalent): the positions travel out-of-band so the
/// worklet thread can mint each lane port and place it back into the
/// deserialized value (see `instantiate_processor`).
#[expect(unsafe_code)]
fn substitute_option_ports(
    cx: &mut JSContext,
    value: js::rust::Handle<JSVal>,
    mut out: js::rust::MutableHandleValue,
) -> Fallible<Vec<OptionPortSlot>> {
    let mut slots = Vec::new();
    let mut rooter = CustomAutoRooter::new(Vec::<*mut JSObject>::new());
    let mut visited: FxHashMap<*mut JSObject, usize> = FxHashMap::default();
    {
        // SAFETY: the guard is created and dropped on this thread with its
        // runtime's own context, per the CustomAutoRooter contract.
        let mut guard = unsafe { rooter.root(cx.raw_cx()) };
        let mut path = Vec::new();
        let placement = substitution_walk(
            cx,
            value.get(),
            &mut path,
            &mut guard,
            &mut slots,
            &mut visited,
            0,
        )?;
        let substituted = match placement {
            Placement::Value(value) => value,
            Placement::Object(index) => ObjectValue(guard[index]),
            Placement::Null => NullValue(),
        };
        out.set(substituted);
    }
    Ok(slots)
}

/// One recursion step of [`substitute_option_ports`]. `source` objects are
/// stack-rooted per level; every object the walk creates or keeps by
/// reference is additionally held in `guard`'s auto-rooter and only ever
/// re-read from it (never carried as a bare `JSVal` across a JS call).
#[expect(unsafe_code)]
fn substitution_walk(
    cx: &mut JSContext,
    value: JSVal,
    path: &mut Vec<PortPathSeg>,
    guard: &mut CustomAutoRooterGuard<'_, Vec<*mut JSObject>>,
    slots: &mut Vec<OptionPortSlot>,
    visited: &mut FxHashMap<*mut JSObject, usize>,
    depth: usize,
) -> Fallible<Placement> {
    if depth > MAX_SUBSTITUTION_DEPTH || guard.len() > MAX_SUBSTITUTION_OBJECTS {
        return Err(Error::DataClone(None));
    }
    if !value.is_object() {
        return Ok(Placement::Value(value));
    }
    rooted!(&in(cx) let source = value.to_object());

    // A `MessagePort`: record the slot (identity-deduped — the same object
    // at two positions is one lane with two paths) and place the null
    // placeholder.
    // SAFETY: a plain read of the reflector for `source.get()` (an object
    // the walk rooted above); no JS runs inside.
    let rooted_port = unsafe { root_from_object::<MessagePort>(cx, source.get()) };
    if let Ok(port) = rooted_port {
        let id = *port.message_port_id();
        let slot = match slots
            .iter_mut()
            .find(|slot| *slot.port.message_port_id() == id)
        {
            Some(slot) => slot,
            None => {
                slots.push(OptionPortSlot {
                    port,
                    paths: Vec::new(),
                });
                slots.last_mut().expect("slot just pushed")
            },
        };
        slot.paths.push(path.clone());
        return Ok(Placement::Null);
    }

    let mut class = ESClass::Other;
    if !unsafe { GetBuiltinClass(cx, source.handle(), &mut class) } {
        unsafe { JS_ClearPendingException(cx) };
        return Err(Error::DataClone(None));
    }
    match class {
        ESClass::Object => {
            if let Some(&index) = visited.get(&source.get()) {
                return Ok(Placement::Object(index));
            }
            let clone = unsafe { JS_NewObject(cx, std::ptr::null()) };
            if clone.is_null() {
                return Err(Error::DataClone(None));
            }
            guard.push(clone);
            let clone_index = guard.len() - 1;
            visited.insert(source.get(), clone_index);

            let mut ids = IdVector::new(cx);
            if !unsafe {
                GetPropertyKeys(cx, source.handle(), JSITER_OWNONLY, ids.handle_mut())
            } {
                unsafe { JS_ClearPendingException(cx) };
                return Err(Error::DataClone(None));
            }
            for id in ids.iter() {
                rooted!(&in(cx) let id = *id);
                rooted!(&in(cx) let mut key_val = UndefinedValue());
                if !unsafe { JS_IdToValue(cx, id.get(), key_val.handle_mut()) } {
                    continue;
                }
                let key = if key_val.is_string() {
                    rooted!(&in(cx) let js_string = key_val.to_string());
                    let Some(js_string) = std::ptr::NonNull::new(js_string.get()) else {
                        continue;
                    };
                    unsafe { jsstr_to_string(cx, js_string) }
                } else if key_val.is_int32() {
                    key_val.to_int32().to_string()
                } else {
                    // Symbol-keyed properties are skipped by structured
                    // clone; skip them here too.
                    continue;
                };
                let Ok(c_key) = std::ffi::CString::new(key.as_bytes()) else {
                    continue;
                };
                rooted!(&in(cx) let mut prop_val = UndefinedValue());
                if !unsafe {
                    JS_GetPropertyById(cx, source.handle(), id.handle(), prop_val.handle_mut())
                } {
                    // A getter threw: fail the construction DataClone-shaped.
                    unsafe { JS_ClearPendingException(cx) };
                    return Err(Error::DataClone(None));
                }
                path.push(PortPathSeg::Key(key.into()));
                let placement = substitution_walk(
                    cx,
                    prop_val.get(),
                    path,
                    guard,
                    slots,
                    visited,
                    depth + 1,
                )?;
                path.pop();
                rooted!(&in(cx) let clone_here = guard[clone_index]);
                rooted!(&in(cx) let mut defined = UndefinedValue());
                match placement {
                    Placement::Value(value) => defined.set(value),
                    Placement::Object(index) => defined.set(ObjectValue(guard[index])),
                    Placement::Null => defined.set(NullValue()),
                }
                if !unsafe {
                    JS_DefineProperty(
                        cx,
                        clone_here.handle(),
                        c_key.as_ptr(),
                        defined.handle(),
                        JSPROP_ENUMERATE as _,
                    )
                } {
                    unsafe { JS_ClearPendingException(cx) };
                    return Err(Error::DataClone(None));
                }
            }
            Ok(Placement::Object(clone_index))
        },
        ESClass::Array => {
            if let Some(&index) = visited.get(&source.get()) {
                return Ok(Placement::Object(index));
            }
            let mut length = 0u32;
            if !unsafe { GetArrayLength(cx, source.handle(), &mut length) } {
                return Err(Error::DataClone(None));
            }
            let clone = unsafe { NewArrayObject1(cx, length as usize) };
            if clone.is_null() {
                return Err(Error::DataClone(None));
            }
            guard.push(clone);
            let clone_index = guard.len() - 1;
            visited.insert(source.get(), clone_index);
            for index in 0..length {
                rooted!(&in(cx) let mut element = UndefinedValue());
                if !unsafe { JS_GetElement(cx, source.handle(), index, element.handle_mut()) } {
                    unsafe { JS_ClearPendingException(cx) };
                    return Err(Error::DataClone(None));
                }
                path.push(PortPathSeg::Index(index));
                let placement = substitution_walk(
                    cx,
                    element.get(),
                    path,
                    guard,
                    slots,
                    visited,
                    depth + 1,
                )?;
                path.pop();
                rooted!(&in(cx) let clone_here = guard[clone_index]);
                rooted!(&in(cx) let mut defined = UndefinedValue());
                match placement {
                    Placement::Value(value) => defined.set(value),
                    Placement::Object(index) => defined.set(ObjectValue(guard[index])),
                    Placement::Null => defined.set(NullValue()),
                }
                if !unsafe {
                    JS_DefineElement(
                        cx,
                        clone_here.handle(),
                        index,
                        defined.handle(),
                        JSPROP_ENUMERATE as _,
                    )
                } {
                    unsafe { JS_ClearPendingException(cx) };
                    return Err(Error::DataClone(None));
                }
            }
            Ok(Placement::Object(clone_index))
        },
        _ => {
            // Non-plain container: keep by reference; the serializer handles
            // it (a port nested inside such a value fails the follow-up
            // write loudly).
            guard.push(source.get());
            Ok(Placement::Object(guard.len() - 1))
        },
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
        let (node, serialized_options) =
            AudioWorkletNode::new_inherited(cx, context, name, options)?;
        let dom_root = reflect_dom_object_with_proto(
            cx,
            Box::new(node),
            window,
            proto,
        );
        let audio_worklet = context.AudioWorklet();
        dom_root.wire_processor(cx, &audio_worklet, serialized_options);
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
