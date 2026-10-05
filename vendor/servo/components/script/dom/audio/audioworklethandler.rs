/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// (Bao 段(3) wiring, user ruling 2026-10-05 "自研吧"): the worklet-thread
// SpiderMonkey face of the AudioWorklet render bridge — the script side of
// the e90 media seam ([`AudioWorkletProcessorHandler`]/[`AudioWorkletPump`]).
//
// Everything in this module runs on the audio worklet thread: processor
// instantiation (`new ctor()`), the per-quantum `process()` call, and the
// inbound port drain. The media bridge never drives SpiderMonkey and never
// blocks; it pushes a quantum and calls the wake hook (a
// `schedule_a_worklet_task` post installed at pump registration), which lands
// in [`AudioWorkletGlobalScope::drain_audio_pumps`] on the worklet thread.
//
// GC discipline (W28/W30 class): the processor instance and the persistent
// per-port `Float32Array`s live in the scope's *traced* instance registry
// (`ProcessorInstanceData`), so collection keeps them alive and relocation
// updates their slots. Nothing here holds a GC pointer across a JS call:
// every quantum re-reads the traced slots (short borrows → stack copies into
// rooted locals), calls `process()`, then re-borrows for the output copy. A
// GC inside `process()` may move the arrays; the post-call borrow reads the
// updated slots, and the `Root<Value>` argument locals are exact roots SM
// updates on move.

#![expect(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use std::ptr::null_mut;

use js::context::JSContext;
use js::gc::HandleValue;
use js::jsapi::{HandleValueArray, Heap, JSObject};
use js::jsval::{ObjectValue, UndefinedValue};
use js::realm::AutoRealm;
use js::rust::wrappers2::{Call, Construct1, JS_ClearPendingException, JS_IsExceptionPending};
use servo_media::audio::audioworklet_node::{
    AudioWorkletBridge, AudioWorkletPump, AudioWorkletProcessorHandler, ProcessorControl,
    WorkletQuantum,
};
use servo_media::audio::param::ParamRate;

use crate::dom::audio::audioworkletglobalscope::{
    AudioWorkletGlobalScope, ProcessorInstanceData, RootedArraySlot,
};
use crate::dom::audio::audioworkletnode::AudioWorkletNode;
use crate::dom::bindings::conversions::get_property_jsval;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use script_bindings::reflector::DomObject;
use crate::messaging::{CommonScriptMsg, MainThreadScriptMsg};
use crate::realms::enter_auto_realm;
use crate::runtime::script_runtime::ScriptThreadEventCategory;
use crate::tasks::task_source::TaskSourceName;

/// One declared `parameterDescriptors` entry, extracted on the worklet
/// thread at `registerProcessor` and shared to the script thread through the
/// registry (the name→`WorkletParam(i)` order and the `AudioParam` defaults).
#[derive(Clone, Debug)]
pub(crate) struct ParamDescriptor {
    pub(crate) name: String,
    pub(crate) default_value: f32,
    pub(crate) min_value: f32,
    pub(crate) max_value: f32,
    /// Spec default is `"a-rate"`.
    pub(crate) rate: ParamRate,
}

/// The script-thread-visible registry of registered processors:
/// name → parameter descriptors, in declaration order. Written by
/// `registerProcessor` (worklet thread, each pool thread's scope), read by
/// `new AudioWorkletNode` (script thread) to validate the name (spec
/// NotSupportedError arm) and to build the node's `AudioParamMap` + the
/// media `WorkletParamInit` list.
pub(crate) type SharedProcessorRegistry =
    std::sync::Mutex<rustc_hash::FxHashMap<stylo_atoms::Atom, Vec<ParamDescriptor>>>;

/// The worklet-thread handler for one node's processor instance: the
/// [`AudioWorkletProcessorHandler`] the e90 pump drives at block rate.
///
/// SAFETY (Send): `scope` is a raw borrow of the scope this pump is
/// registered on. The handler is created on, called on, and dropped on that
/// same worklet thread (pump entries are scope fields), so the pointer never
/// crosses a thread boundary — it only crosses the `Send` bound the
/// media-side trait requires. The scope outlives every pump entry it owns,
/// so the pointer is valid whenever the handler runs.
pub(crate) struct WorkletProcessorHandler {
    scope: *const AudioWorkletGlobalScope,
    /// The media graph node key of the owning `AudioWorkletNode`.
    node_key: u64,
    /// The shared bridge; failure latches here so the render side mutes.
    bridge: Arc<AudioWorkletBridge>,
    /// The DOM node, for the one-shot `processorerror` task.
    node: Trusted<AudioWorkletNode>,
    main_sender: Sender<MainThreadScriptMsg>,
    /// Script-thread failure report already queued (the node also carries a
    /// once-guard; this keeps a throw in every block from flooding the
    /// script thread's queue).
    error_reported: AtomicBool,
}

impl WorkletProcessorHandler {
    pub(crate) fn new(
        scope: *const AudioWorkletGlobalScope,
        node_key: u64,
        bridge: Arc<AudioWorkletBridge>,
        node: Trusted<AudioWorkletNode>,
        main_sender: Sender<MainThreadScriptMsg>,
    ) -> WorkletProcessorHandler {
        WorkletProcessorHandler {
            scope,
            node_key,
            bridge,
            node,
            main_sender,
            error_reported: AtomicBool::new(false),
        }
    }

    /// Latch the failure on the bridge (render side mutes) and queue the
    /// one-shot `processorerror` task to the script thread.
    fn report_processor_error(&mut self) {
        self.bridge.signal_processor_error();
        if self.error_reported.swap(true, Ordering::AcqRel) {
            return;
        }
        let node = self.node.clone();
        let task = task!(fire_processorerror: move |cx| {
            let node = node.root();
            node.fire_processorerror_once(cx);
        });
        let msg = CommonScriptMsg::Task(
            ScriptThreadEventCategory::WorkletEvent,
            Box::new(task),
            None,
            TaskSourceName::DOMManipulation,
        );
        let _ = self.main_sender.send(MainThreadScriptMsg::Common(msg));
    }
}

// SAFETY: see the struct doc — thread-confined raw borrow, never
// dereferenced off the owning worklet thread.
unsafe impl Send for WorkletProcessorHandler {}

impl AudioWorkletProcessorHandler for WorkletProcessorHandler {
    fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
        // SAFETY: see Send — same-thread borrow of the scope owning this
        // pump entry.
        let scope = unsafe { &*self.scope };
        let Some(inst) = scope.instance_data(self.node_key) else {
            // The instance data vanished (teardown raced the pump): halt
            // honestly instead of dereferencing anything.
            self.bridge.signal_processor_error();
            return ProcessorControl::Finish;
        };

        // This thread's own runtime context (the worklet thread's Runtime
        // outlives the scope; the call happens on that same thread).
        // SAFETY: one JSContext per worklet thread, live for the call.
        let mut thread_cx = unsafe { JSContext::get_from_thread() }
            .expect("AudioWorklet pump running off the worklet thread");
        let mut realm = AutoRealm::new(
            &mut thread_cx,
            std::ptr::NonNull::new(inst.global.get())
                .expect("Processor instance global is null"),
        );
        let cx = &mut *realm;

        // Re-read `process` fresh every block: the spec resolves the property
        // per call, so a processor swapping `this.process` is honoured.
        rooted!(&in(cx) let mut process_val = UndefinedValue());
        if inst.instance.get().is_null() {
            self.report_processor_error();
            return ProcessorControl::Finish;
        }
        rooted!(&in(cx) let instance_obj = inst.instance.get());
        if get_property_jsval(cx, instance_obj.handle(), c"process", process_val.handle_mut())
            .is_err()
        {
            self.report_processor_error();
            return ProcessorControl::Finish;
        }
        if !process_val.is_object() {
            // No callable `process` method: the processor will never produce
            // output. Latch through the same channel (one report, node
            // muted).
            self.report_processor_error();
            return ProcessorControl::Finish;
        }

        // Copy inputs and parameter timelines into the persistent arrays.
        // Short borrow: no JS runs inside the copies.
        if !inst.write_inputs(cx, quantum) {
            self.report_processor_error();
            return ProcessorControl::Finish;
        }

        // Root the three argument objects (exact roots: SM updates the
        // object pointers they carry across a moving GC inside the call).
        rooted!(&in(cx) let inputs_value = ObjectValue(scope.rooted_array(self.node_key, RootedArraySlot::Inputs)));
        rooted!(&in(cx) let outputs_value = ObjectValue(scope.rooted_array(self.node_key, RootedArraySlot::Outputs)));
        rooted!(&in(cx) let params_value = ObjectValue(scope.rooted_array(self.node_key, RootedArraySlot::Params)));

        rooted_vec!(let mut call_args);
        call_args.push(inputs_value.get());
        call_args.push(outputs_value.get());
        call_args.push(params_value.get());
        let args = HandleValueArray::from(&call_args);
        rooted!(&in(cx) let this_value = UndefinedValue());
        rooted!(&in(cx) let mut result = UndefinedValue());
        unsafe {
            Call(
                cx,
                this_value.handle(),
                process_val.handle(),
                &args,
                result.handle_mut(),
            );
        }

        if unsafe { JS_IsExceptionPending(cx) } {
            unsafe { JS_ClearPendingException(cx) };
            // A throwing block outputs silence (the output copy below is
            // skipped on this path — the quantum buffers stay zeroed).
            self.report_processor_error();
            return ProcessorControl::Finish;
        }

        // Copy the processor's output into the quantum (fresh borrow — a GC
        // inside `process()` may have moved the arrays; the slots are up to
        // date).
        inst.read_outputs(cx, quantum);

        // The boolean return drives the spec's "no more output" halt.
        if result.is_boolean() && !result.to_boolean() {
            return ProcessorControl::Finish;
        }
        ProcessorControl::Continue
    }
}

/// Instantiate the processor for one node (worklet thread). Runs inside a
/// realm of the given scope: spec
/// `InstantiateAProcessorForAudioWorkletNode` — `new processorCtor()`, wire
/// the processor-side port through the node's conduit, build the persistent
/// per-port arrays, and register the traced instance data + the block-rate
/// pump. Any failure latches the bridge (mutes the node) and reports
/// `processorerror` through the one-shot channel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn instantiate_processor(
    cx: &mut JSContext,
    scope: &AudioWorkletGlobalScope,
    node_key: u64,
    ctor: HandleValue,
    node: Trusted<AudioWorkletNode>,
    bridge: Arc<AudioWorkletBridge>,
    conduit: Arc<crate::dom::audio::audioworkletport::AudioWorkletPortConduit>,
    shape: &crate::dom::audio::audioworkletnode::WorkletNodeShape,
    main_sender: Sender<MainThreadScriptMsg>,
) {
    use crate::dom::audio::audioworkletport::{PortDirection, PortRedirect};
    use js::rust::wrappers2::{JS_NewObject, NewArrayObject};
    use js::rust::wrappers2::JS_SetProperty;

    // The worklet thread mints DOM ids here (the processor's MessagePort) —
    // install the pipeline namespace first, mirroring the worker global scope
    // face (workerglobalscope.rs does the same on its threads).
    servo_base::id::PipelineNamespace::auto_install();

    let mut realm = enter_auto_realm(cx, scope.upcast::<crate::dom::globalscope::GlobalScope>());
    let cx = &mut realm.current_realm();

    // `new ctor()`: `processorOptions` is not plumbed yet (documented 段(3)
    // limitation) — the processor constructs with `undefined`.
    rooted_vec!(let mut ctor_args);
    ctor_args.push(UndefinedValue());
    let args = HandleValueArray::from(&ctor_args);
    rooted!(&in(cx) let mut instance = null_mut::<JSObject>());
    unsafe {
        Construct1(cx, ctor, &args, instance.handle_mut());
    }
    if unsafe { JS_IsExceptionPending(cx) } {
        debug!("AudioWorklet processor constructor threw for node {node_key}.");
        unsafe { JS_ClearPendingException(cx) };
        latch_failure(&node, &bridge, &main_sender);
        return;
    }

    // The instance's `port` property (its constructor created it).
    rooted!(&in(cx) let mut port_value = UndefinedValue());
    if get_property_jsval(cx, instance.handle(), c"port", port_value.handle_mut()).is_err() ||
        !port_value.is_object()
    {
        debug!("AudioWorklet processor instance has no port for node {node_key}.");
        latch_failure(&node, &bridge, &main_sender);
        return;
    }
    let port_obj = port_value.to_object();
    let port = match unsafe { crate::dom::bindings::conversions::root_from_object::<
        crate::dom::globalscope::messageport::MessagePort,
    >(cx, port_obj) } {
        Ok(port) => port,
        Err(_) => {
            latch_failure(&node, &bridge, &main_sender);
            return;
        },
    };

    // Persistent per-port channel arrays, one `Float32Array(128)` each
    // (k-rate parameters expose a length-1 array per the spec). Created once;
    // every block reuses them through their traced slots.
    fn make_channel_array(
        cx: &mut JSContext,
        len: usize,
    ) -> crate::dom::bindings::buffer_source::HeapBufferSource<js::typedarray::Float32> {
        let arr = crate::dom::bindings::buffer_source::HeapBufferSource::
            <js::typedarray::Float32>::default();
        let zeros = vec![0.; len];
        let _ = arr.set_data(cx, &zeros);
        arr
    }
    let input_arrays: Vec<Vec<_>> = (0..shape.input_ports)
        .map(|_| {
            (0..shape.input_channels)
                .map(|_| make_channel_array(cx, 128))
                .collect()
        })
        .collect();
    let output_arrays: Vec<Vec<_>> = (0..shape.output_ports)
        .map(|port| {
            (0..shape.output_channels[port as usize].max(1))
                .map(|_| make_channel_array(cx, 128))
                .collect()
        })
        .collect();
    let param_arrays: Vec<_> = shape
        .params
        .iter()
        .map(|param| {
            make_channel_array(cx, if param.rate == ParamRate::KRate { 1 } else { 128 })
        })
        .collect();

    // Outer argument containers: `inputs[p][c]`, `outputs[p][c]` arrays and
    // the `params` object keyed by parameter name. The per-channel objects
    // are read through their traced slots (`RootedTypedArray` keeps each
    // alive for the read; no JS runs in here).
    fn inner_object(
        arr: &crate::dom::bindings::buffer_source::HeapBufferSource<js::typedarray::Float32>,
    ) -> *mut JSObject {
        let Ok(view) = arr.get_typed_array() else {
            return std::ptr::null_mut();
        };
        unsafe { view.underlying_object().get() }
    }
    #[expect(unsafe_code)]
    fn outer_array(
        cx: &mut JSContext,
        objects: Vec<*mut JSObject>,
    ) -> *mut JSObject {
        rooted_vec!(let mut values);
        for obj in objects {
            values.push(ObjectValue(obj));
        }
        unsafe { NewArrayObject(cx, &HandleValueArray::from(&values)) }
    }
    let mut input_port_objects = Vec::with_capacity(input_arrays.len());
    for port in &input_arrays {
        let mut channel_objects = Vec::with_capacity(port.len());
        for arr in port {
            channel_objects.push(inner_object(arr));
        }
        input_port_objects.push(outer_array(cx, channel_objects));
    }
    let inputs_array = outer_array(cx, input_port_objects);
    let mut output_port_objects = Vec::with_capacity(output_arrays.len());
    for port in &output_arrays {
        let mut channel_objects = Vec::with_capacity(port.len());
        for arr in port {
            channel_objects.push(inner_object(arr));
        }
        output_port_objects.push(outer_array(cx, channel_objects));
    }
    let outputs_array = outer_array(cx, output_port_objects);
    let params_object = unsafe { JS_NewObject(cx, std::ptr::null()) };
    for (index, param) in shape.params.iter().enumerate() {
        if index >= param_arrays.len() {
            break;
        }
        let obj = inner_object(&param_arrays[index]);
        if obj.is_null() {
            continue;
        }
        rooted!(&in(cx) let value = ObjectValue(obj));
        rooted!(&in(cx) let params_ref = params_object);
        let Ok(name) = std::ffi::CString::new(param.name.clone()) else {
            continue;
        };
        unsafe {
            JS_SetProperty(cx, params_ref.handle(), name.as_ptr(), value.handle());
        }
    }
    let heap = |obj: *mut JSObject| {
        let slot = Heap::default();
        slot.set(obj);
        slot
    };

    let instance_data = ProcessorInstanceData {
        instance: heap(instance.get()),
        global: heap(
            scope
                .upcast::<crate::dom::globalscope::GlobalScope>()
                .reflector()
                .get_jsobject()
                .get(),
        ),
        inputs_array: heap(inputs_array),
        outputs_array: heap(outputs_array),
        params_object: heap(params_object),
        input_arrays,
        output_arrays,
        param_arrays,
        port: crate::dom::bindings::root::Dom::from_ref(&*port),
    };
    scope.register_processor_instance(node_key, instance_data);

    // Route the processor-side port through the conduit's `to_main` ring.
    port.set_bao_port_redirect(PortRedirect {
        conduit: conduit.clone(),
        direction: PortDirection::ToMain,
    });

    // Wake hook: the render bridge (quantum pushes) and the node port's
    // postMessage path notify this closure, which posts the pump-drain
    // worklet task. Installed on BOTH the bridge (media face) and the
    // conduit (script face) — they are separate wake surfaces over the same
    // worklet thread.
    let executor = scope.upcast::<crate::dom::workletglobalscope::WorkletGlobalScope>().executor();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = executor.schedule_a_worklet_task(Box::new(|cx, scope| {
            if let Some(audio) = scope.downcast::<AudioWorkletGlobalScope>() {
                audio.drain_audio_pumps(cx);
            }
        }));
    });
    bridge.set_wake_fn(wake.clone());
    conduit.set_wake_worklet(wake);

    // Register the pump last, then pick up any quantum the render side
    // published before registration. The handler and the pump share the
    // bridge Arc: the handler latches failures on it, the pump drives it.
    let handler = WorkletProcessorHandler::new(scope, node_key, bridge.clone(), node, main_sender);
    let pump = AudioWorkletPump::new(bridge, Box::new(handler));
    scope.register_pump(node_key, conduit, pump);
    scope.drain_audio_pumps(cx);
}

/// Bridge latch + one-shot script-thread report shared by the instantiation
/// failure arms (no handler exists yet at that point).
fn latch_failure(
    node: &Trusted<AudioWorkletNode>,
    bridge: &Arc<AudioWorkletBridge>,
    main_sender: &Sender<MainThreadScriptMsg>,
) {
    bridge.signal_processor_error();
    let node = node.clone();
    let sender = main_sender.clone();
    let task = task!(fire_processorerror: move |cx| {
        let node = node.root();
        node.fire_processorerror_once(cx);
    });
    let msg = CommonScriptMsg::Task(
        ScriptThreadEventCategory::WorkletEvent,
        Box::new(task),
        None,
        TaskSourceName::DOMManipulation,
    );
    let _ = sender.send(MainThreadScriptMsg::Common(msg));
}
