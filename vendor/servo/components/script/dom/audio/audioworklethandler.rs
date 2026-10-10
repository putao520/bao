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
use js::rust::wrappers2::{
    Call, Construct1, JS_ClearPendingException, JS_GetPendingException, JS_IsExceptionPending,
    NewArrayObject,
};
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
use crate::dom::bindings::error::ErrorInfo;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::structuredclone;
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
    /// one-shot `processorerror` task to the script thread. `info` is the
    /// captured throw site (message/filename/line/column) when the failure
    /// was a pending JS exception — the spec's `processorerror` is an
    /// `ErrorEvent` carrying exactly those (e122).
    fn report_processor_error(&mut self, info: Option<ErrorInfo>) {
        self.bridge.signal_processor_error();
        if self.error_reported.swap(true, Ordering::AcqRel) {
            return;
        }
        let node = self.node.clone();
        let task = task!(fire_processorerror: move |cx| {
            let node = node.root();
            node.fire_processorerror_once(cx, info);
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
        let global_ptr = match scope.instance_data(self.node_key) {
            Some(inst) => std::ptr::NonNull::new(inst.global.get())
                .expect("Processor instance global is null"),
            None => {
                // The instance data vanished (teardown raced the pump): halt
                // honestly instead of dereferencing anything.
                self.bridge.signal_processor_error();
                return ProcessorControl::Finish;
            },
        };

        // This thread's own runtime context (the worklet thread's Runtime
        // outlives the scope; the call happens on that same thread).
        // SAFETY: one JSContext per worklet thread, live for the call.
        let mut thread_cx = unsafe { JSContext::get_from_thread() }
            .expect("AudioWorklet pump running off the worklet thread");
        let mut realm = AutoRealm::new(&mut thread_cx, global_ptr);
        let cx = &mut *realm;

        // (e147) The spec's dynamic input face: `inputs[p]` is empty while
        // port `p` has no connection and carries the connected bus's channel
        // count otherwise. Reshape the JS argument arrays when this
        // quantum's live per-port counts differ from the current shape
        // (steady state: one Vec compare, no allocation).
        let (inputs_stale, outputs_stale) = scope
            .instance_data(self.node_key)
            .map(|inst| {
                (
                    inst.input_live.as_slice() != &quantum.input_live[..],
                    inst.output_live.as_slice() != &quantum.output_live[..],
                )
            })
            .unwrap_or((true, true));
        if inputs_stale &&
            !rebuild_input_arrays(cx, scope, self.node_key, &quantum.input_live)
        {
            self.report_processor_error(None);
            return ProcessorControl::Finish;
        }
        if outputs_stale &&
            !rebuild_output_arrays(cx, scope, self.node_key, &quantum.output_live)
        {
            self.report_processor_error(None);
            return ProcessorControl::Finish;
        }

        // (e155) The parameters read-back face (crbug.com/1151069 semantics):
        // match → rebuild → copy, per declared name, BEFORE `process` is
        // looked up. A copy failure invalidates the node — silence for the
        // rest of its lifetime, `process()` never invoked again.
        if !sync_param_face(cx, scope, self.node_key, quantum) {
            self.report_processor_error(Some(ErrorInfo {
                message: "process(): Failed to copy parameter data.".to_owned(),
                filename: String::new(),
                lineno: 0,
                column: 0,
            }));
            return ProcessorControl::Finish;
        }

        let Some(inst) = scope.instance_data(self.node_key) else {
            self.bridge.signal_processor_error();
            return ProcessorControl::Finish;
        };

        // Re-read `process` fresh every block: the spec resolves the property
        // per call, so a processor swapping `this.process` is honoured.
        rooted!(&in(cx) let mut process_val = UndefinedValue());
        if inst.instance.get().is_null() {
            self.report_processor_error(None);
            return ProcessorControl::Finish;
        }
        rooted!(&in(cx) let instance_obj = inst.instance.get());
        if get_property_jsval(cx, instance_obj.handle(), c"process", process_val.handle_mut())
            .is_err()
        {
            self.report_processor_error(None);
            return ProcessorControl::Finish;
        }
        if !process_val.is_object() {
            // No callable `process` method: the processor will never produce
            // output. Latch through the same channel (one report, node
            // muted).
            self.report_processor_error(None);
            return ProcessorControl::Finish;
        }

        // Copy inputs and parameter timelines into the persistent arrays.
        // Short borrow: no JS runs inside the copies.
        if !inst.write_inputs(cx, quantum) {
            self.report_processor_error(None);
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
        // (e122) `this` is the processor instance: class bodies are strict
        // mode, so a `this` of `undefined` made every `this.port.*` /
        // `this.<field>` access in `process()` throw a TypeError on the
        // first block (the node latched processorerror and went silent —
        // every WPT processor that touches `this` hung there).
        rooted!(&in(cx) let this_value = ObjectValue(instance_obj.get()));
        rooted!(&in(cx) let mut result = UndefinedValue());
        // (e176) Pin the worklet clock to this quantum for the duration of
        // `process()`: the getters must not roundtrip the live render clock
        // mid-call (see `AudioWorkletGlobalScope::process_quantum_clock`).
        scope.pin_process_quantum_clock(quantum.frame, quantum.time);
        unsafe {
            Call(
                cx,
                this_value.handle(),
                process_val.handle(),
                &args,
                result.handle_mut(),
            );
        }
        scope.unpin_process_quantum_clock();

        if unsafe { JS_IsExceptionPending(cx) } {
            // (e122) Capture the throw site before clearing: the spec's
            // `processorerror` is an ErrorEvent carrying the exception's
            // message/filename/line/column.
            rooted!(&in(cx) let mut exn = UndefinedValue());
            let info = if unsafe { JS_GetPendingException(cx, exn.handle_mut()) } {
                Some(ErrorInfo::from_value(cx, exn.handle()))
            } else {
                None
            };
            unsafe { JS_ClearPendingException(cx) };
            // A throwing block outputs silence (the output copy below is
            // skipped on this path — the quantum buffers stay zeroed).
            self.report_processor_error(info);
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

/// (e147, shared with the per-quantum input rebuild) Create one rooted
/// `Float32Array(len)` and return its index into `roots`.
fn make_channel_array_index(
    cx: &mut JSContext,
    len: usize,
    roots: &mut js::gc::RootedVec<'_, js::jsval::JSVal>,
) -> Option<usize> {
    rooted!(&in(cx) let mut array = null_mut::<JSObject>());
    let zeros = vec![0.; len];
    if crate::dom::bindings::buffer_source::create_buffer_source::<js::typedarray::Float32>(
        cx,
        &zeros,
        array.handle_mut(),
    )
    .is_err()
        || array.get().is_null()
    {
        return None;
    }
    roots.push(ObjectValue(array.get()));
    Some(roots.len() - 1)
}

#[expect(unsafe_code)]
fn outer_array_index(
    cx: &mut JSContext,
    roots: &mut js::gc::RootedVec<'_, js::jsval::JSVal>,
    indices: &[usize],
) -> Option<usize> {
    rooted_vec!(let mut values);
    for &index in indices {
        values.push(roots[index]);
    }
    let array = unsafe { NewArrayObject(cx, &HandleValueArray::from(&values)) };
    if array.is_null() {
        return None;
    }
    roots.push(ObjectValue(array));
    Some(roots.len() - 1)
}

fn rooted_channel(
    cx: &mut JSContext,
    roots: &js::gc::RootedVec<'_, js::jsval::JSVal>,
    index: usize,
) -> crate::dom::bindings::buffer_source::HeapBufferSource<js::typedarray::Float32> {
    rooted!(&in(cx) let obj = roots[index].to_object());
    crate::dom::bindings::buffer_source::HeapBufferSource::<js::typedarray::Float32>::new(
        obj.handle(),
    )
}

/// (e147) Rebuild one instance's JS `inputs` argument face for a new
/// per-port channel shape — the spec's dynamic input face (`inputs[p]` is
/// empty while port `p` has no connection, and carries the connected bus's
/// channel count otherwise). Runs on the worklet thread before `process()`;
/// the fresh containers are frozen exactly like the instantiation-time
/// ones. GC discipline (the e126/e127 window): everything stays rooted in
/// `roots` across every allocation, the channel handles are derived last
/// from the rooted values (boxed `Heap`s, move-safe by construction), and
/// the inline `inputs_array` slot is written at its final registry address
/// inside [`AudioWorkletGlobalScope::update_input_face`].
pub(crate) fn rebuild_input_arrays(
    cx: &mut JSContext,
    scope: &AudioWorkletGlobalScope,
    node_key: u64,
    live: &[u8],
) -> bool {
    use js::rust::wrappers2::JS_FreezeObject;

    rooted_vec!(let mut roots);
    let mut leaf_indices: Vec<Vec<usize>> = Vec::with_capacity(live.len());
    for &channels in live {
        let mut port_indices = Vec::with_capacity(channels as usize);
        for _ in 0..channels {
            match make_channel_array_index(cx, 128, &mut roots) {
                Some(index) => port_indices.push(index),
                None => return false,
            }
        }
        leaf_indices.push(port_indices);
    }
    let mut port_wrapper_indices = Vec::with_capacity(leaf_indices.len());
    for port_indices in &leaf_indices {
        match outer_array_index(cx, &mut roots, port_indices) {
            Some(index) => port_wrapper_indices.push(index),
            None => return false,
        }
    }
    let Some(outer_index) = outer_array_index(cx, &mut roots, &port_wrapper_indices) else {
        return false;
    };
    // Freeze the containers from rooted values (the FrozenArray face).
    let mut frozen = {
        rooted!(&in(cx) let outer_obj = roots[outer_index].to_object());
        unsafe { JS_FreezeObject(cx, outer_obj.handle()) }
    };
    for &port_index in &port_wrapper_indices {
        rooted!(&in(cx) let port_obj = roots[port_index].to_object());
        frozen &= unsafe { JS_FreezeObject(cx, port_obj.handle()) };
    }
    if !frozen {
        return false;
    }
    // Derive the channel handles last — post-GC addresses, never pre-GC
    // copies.
    let arrays = leaf_indices
        .iter()
        .map(|port| {
            port
                .iter()
                .map(|&index| rooted_channel(cx, &roots, index))
                .collect()
        })
        .collect();
    rooted!(&in(cx) let outer_final = roots[outer_index].to_object());
    scope.update_input_face(node_key, outer_final.get(), arrays, live)
}

/// (e147) The output-face twin of [`rebuild_input_arrays`]: reshape the JS
/// `outputs` argument arrays to the quantum's live per-port channel counts
/// (explicit `outputChannelCount`, else the spec's computed count). The
/// fresh channel arrays start zeroed — the per-block read-back zeroes them
/// afterwards, so a processor that skips writing sees zeros.
pub(crate) fn rebuild_output_arrays(
    cx: &mut JSContext,
    scope: &AudioWorkletGlobalScope,
    node_key: u64,
    live: &[u8],
) -> bool {
    use js::rust::wrappers2::JS_FreezeObject;

    rooted_vec!(let mut roots);
    let mut leaf_indices: Vec<Vec<usize>> = Vec::with_capacity(live.len());
    for &channels in live {
        let mut port_indices = Vec::with_capacity(channels as usize);
        for _ in 0..channels {
            match make_channel_array_index(cx, 128, &mut roots) {
                Some(index) => port_indices.push(index),
                None => return false,
            }
        }
        leaf_indices.push(port_indices);
    }
    let mut port_wrapper_indices = Vec::with_capacity(leaf_indices.len());
    for port_indices in &leaf_indices {
        match outer_array_index(cx, &mut roots, port_indices) {
            Some(index) => port_wrapper_indices.push(index),
            None => return false,
        }
    }
    let Some(outer_index) = outer_array_index(cx, &mut roots, &port_wrapper_indices) else {
        return false;
    };
    let mut frozen = {
        rooted!(&in(cx) let outer_obj = roots[outer_index].to_object());
        unsafe { JS_FreezeObject(cx, outer_obj.handle()) }
    };
    for &port_index in &port_wrapper_indices {
        rooted!(&in(cx) let port_obj = roots[port_index].to_object());
        frozen &= unsafe { JS_FreezeObject(cx, port_obj.handle()) };
    }
    if !frozen {
        return false;
    }
    let arrays = leaf_indices
        .iter()
        .map(|port| {
            port
                .iter()
                .map(|&index| rooted_channel(cx, &roots, index))
                .collect()
        })
        .collect();
    rooted!(&in(cx) let outer_final = roots[outer_index].to_object());
    scope.update_output_face(node_key, outer_final.get(), arrays, live)
}

/// (e155) Read one declared parameter property off the JS `parameters`
/// object. `Get` resolves through the prototype chain, so a page-installed
/// `Object.prototype` accessor runs here (the crbug.com/1151069 face): a
/// throwing getter is cleared and counts as absent, and anything that is not
/// a live `Float32Array` of exactly the declared length (k-rate 1, a-rate
/// 128) is rejected. A detached buffer reads as length 0, which fails every
/// declared length.
fn get_param_array(
    cx: &mut JSContext,
    params: js::rust::Handle<*mut JSObject>,
    name: &std::ffi::CStr,
    expected_len: usize,
) -> Option<js::typedarray::TypedArray<js::typedarray::Float32, *mut JSObject>> {
    rooted!(&in(cx) let mut value = UndefinedValue());
    if get_property_jsval(cx, params, name, value.handle_mut()).is_err() {
        // The accessor threw: the reference implementation catches this in
        // its TryCatch and treats the property as unusable.
        unsafe { JS_ClearPendingException(cx) };
        return None;
    }
    if !value.is_object() {
        return None;
    }
    rooted!(&in(cx) let object = value.to_object());
    let array = js::typedarray::TypedArray::<
        js::typedarray::Float32,
        *mut JSObject,
    >::from(object.get());
    match array {
        Ok(array) if array.len() == expected_len => Some(array),
        _ => None,
    }
}

/// (e155) Swap the JS `parameters` argument object for a freshly built one:
/// own data properties (`DefineOwnProperty` — an `Object.prototype`
/// accessor cannot intercept it; the reference clone's `CreateDataProperty`),
/// frozen per the spec's freeze-parameter-object step. The GC discipline
/// matches the e147 input/output rebuilds: every fresh object stays rooted
/// through the window, and the inline `params_object` `Heap` slot is written
/// at its final registry address inside
/// [`AudioWorkletGlobalScope::update_params_face`].
pub(crate) fn rebuild_param_arrays(
    cx: &mut JSContext,
    scope: &AudioWorkletGlobalScope,
    node_key: u64,
) -> bool {
    use js::jsapi::JSPROP_ENUMERATE;
    use js::rust::wrappers2::{JS_DefineProperty, JS_FreezeObject, JS_NewObject};

    let Some(shapes) = scope.instance_data(node_key).map(|inst| inst.param_shapes.clone())
    else {
        return false;
    };
    rooted_vec!(let mut roots);
    let mut leaf_indices = Vec::with_capacity(shapes.len());
    for (_, len) in &shapes {
        match make_channel_array_index(cx, *len, &mut roots) {
            Some(index) => leaf_indices.push(index),
            None => return false,
        }
    }
    let params_object = unsafe { JS_NewObject(cx, std::ptr::null()) };
    if params_object.is_null() {
        return false;
    }
    roots.push(ObjectValue(params_object));
    let params_object_index = roots.len() - 1;
    for (index, (name, _)) in shapes.iter().enumerate() {
        if name.as_bytes().is_empty() {
            continue;
        }
        rooted!(&in(cx) let value = roots[leaf_indices[index]]);
        rooted!(&in(cx) let params_ref = roots[params_object_index].to_object());
        if !unsafe {
            JS_DefineProperty(
                cx,
                params_ref.handle(),
                name.as_ptr(),
                value.handle(),
                JSPROP_ENUMERATE as _,
            )
        } {
            return false;
        }
    }
    // The spec's SetIntegrityLevel(parameter, frozen) — the arrays and their
    // buffers stay writable (the per-quantum copy writes through the views).
    rooted!(&in(cx) let params_final = roots[params_object_index].to_object());
    if !unsafe { JS_FreezeObject(cx, params_final.handle()) } {
        return false;
    }
    scope.update_params_face(node_key, params_final.get())
}

/// (e155) The per-quantum `parameters` read-back — the crbug.com/1151069
/// semantics the upstream lacks (the node stayed live and kept rendering
/// through any page-mangled parameter property):
///
/// * match: every declared name must resolve to a live `Float32Array` of the
///   declared length;
/// * rebuild: a failed match swaps the params object for a fresh own-data
///   property one, shadowing whatever the page put on the prototype (the
///   rebuild result is advisory, exactly like the reference clone — the copy
///   below is the authority);
/// * copy: the quantum's timeline values are written into whatever each
///   property now resolves to. A failure here invalidates the processor:
///   `process()` is never invoked again, the node latches `processorerror`
///   and outputs silence for the rest of its lifetime.
fn sync_param_face(
    cx: &mut JSContext,
    scope: &AudioWorkletGlobalScope,
    node_key: u64,
    quantum: &WorkletQuantum,
) -> bool {
    // Phase 1 — match. The borrow is scoped: a rebuild swaps the face
    // through `update_params_face`'s own mutable borrow.
    let mut matched = true;
    {
        let Some(inst) = scope.instance_data(node_key) else {
            return false;
        };
        if inst.params_object.get().is_null() {
            return false;
        }
        for (name, len) in inst.param_shapes.iter() {
            if name.as_bytes().is_empty() {
                continue;
            }
            rooted!(&in(cx) let params = inst.params_object.get());
            if get_param_array(cx, params.handle(), name, *len).is_none() {
                matched = false;
                break;
            }
        }
    }
    if !matched {
        rebuild_param_arrays(cx, scope, node_key);
    }
    // Phase 2 — copy into whatever each property resolves to now.
    let Some(inst) = scope.instance_data(node_key) else {
        return false;
    };
    if inst.params_object.get().is_null() {
        return false;
    }
    for (index, (name, len)) in inst.param_shapes.iter().enumerate() {
        let Some(quantum_param) = quantum.params.get(index) else {
            break;
        };
        if name.as_bytes().is_empty() {
            continue;
        }
        rooted!(&in(cx) let params = inst.params_object.get());
        let Some(mut array) = get_param_array(cx, params.handle(), name, *len) else {
            return false;
        };
        let Some(slice) = array.as_mut_slice_safe(cx.no_gc()) else {
            return false;
        };
        if slice.len() == 1 {
            // k-rate: the spec exposes a single-element array; the media
            // face fills all 128 slots with the uniform value.
            slice[0] = quantum_param.data()[0];
        } else {
            let timeline = quantum_param.data();
            let len = slice.len().min(timeline.len());
            slice[..len].copy_from_slice(&timeline[..len]);
        }
    }
    true
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
    serialized_options: crate::dom::audio::audioworkletnode::SerializedOptions,
    main_sender: Sender<MainThreadScriptMsg>,
) {
    use crate::dom::audio::audioworkletport::{PortDirection, PortRedirect};
    use js::rust::wrappers2::JS_SetProperty;
    use js::rust::wrappers2::{JS_NewObject, NewArrayObject};

    // The worklet thread mints DOM ids here (the processor's MessagePort) —
    // install the pipeline namespace first, mirroring the worker global scope
    // face (workerglobalscope.rs does the same on its threads).
    servo_base::id::PipelineNamespace::auto_install();

    let mut realm = enter_auto_realm(cx, scope.upcast::<crate::dom::globalscope::GlobalScope>());
    let cx = &mut realm.current_realm();

    // (e114) spec "invoking processor constructor" steps 4-5 then 8:
    // deserialize the options record in this worklet realm, mint the
    // counterpart endpoint of every substituted `processorOptions`
    // `MessagePort` lane and place it back at the recorded position, then
    // `Construct(ctor, «options»)`. The minted ports are wired ToMain into
    // the node's conduit — their `postMessage` rides the lane ring instead
    // of the constellation port path (which never reaches worklet event
    // loops).
    let global = scope.upcast::<crate::dom::globalscope::GlobalScope>();
    rooted!(&in(cx) let mut options_val = UndefinedValue());
    let mut lane_ports: Vec<DomRoot<crate::dom::globalscope::messageport::MessagePort>> =
        Vec::new();
    let (options_data, port_paths) = serialized_options;
    if structuredclone::read(cx, global, options_data, options_val.handle_mut()).is_err() {
        debug!("AudioWorklet options deserialization failed for node {node_key}.");
        latch_failure(&node, &bridge, &main_sender);
        return;
    }
    for (index, slot_paths) in port_paths.iter().enumerate() {
        let lane = index as u32 + 1;
        let port = crate::dom::globalscope::messageport::MessagePort::new(cx, global);
        port.set_bao_port_redirect(PortRedirect {
            conduit: conduit.clone(),
            direction: PortDirection::ToMain { lane },
        });
        rooted!(&in(cx) let port_value = ObjectValue(
            port.reflector().get_jsobject().get(),
        ));
        // One lane per port object: the same object substituted at several
        // positions is one minted port placed back at each of its paths.
        for path in slot_paths {
            if !set_option_path_value(cx, options_val.handle(), path, port_value.handle()) {
                debug!(
                    "AudioWorklet processorOptions port path missing for node {node_key}, \
                     lane {lane}."
                );
                latch_failure(&node, &bridge, &main_sender);
                return;
            }
        }
        lane_ports.push(port);
    }

    // `new ctor(options)` — the deserialized options dictionary object is
    // the constructor's single argument (spec step 8). (e122) The
    // construction handoff is installed BEFORE the call: the
    // `AudioWorkletProcessor` base constructor (reached via `super()`, or a
    // direct `new AudioWorkletProcessor()` in the exotic construction-port
    // shapes) wires its port into the node's lane-0 conduit as it mints it,
    // so a constructor-body `this.port.postMessage` rides the ring instead
    // of the dead constellation path. The handoff is cleared right after
    // the call — a stale `Fresh` slot would mis-wire the next unrelated
    // base construction.
    scope.set_pending_processor_construction(
        crate::dom::audio::audioworkletglobalscope::PendingProcessorConstruction::Fresh(
            conduit.clone(),
        ),
    );
    rooted_vec!(let mut ctor_args);
    ctor_args.push(options_val.get());
    let args = HandleValueArray::from(&ctor_args);
    rooted!(&in(cx) let mut instance = null_mut::<JSObject>());
    unsafe {
        Construct1(cx, ctor, &args, instance.handle_mut());
    }
    scope.clear_pending_processor_construction();
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
    //
    // (e126) Rooting discipline for the construction window: until
    // `register_processor_instance` files the traced `ProcessorInstanceData`,
    // nothing else references the freshly created JS objects below, so each
    // one (every channel `Float32Array`, every wrapper array, the params
    // object) is pushed into `instance_roots` at birth and stays rooted
    // across all later allocations of this function. Without those roots a
    // nursery collection inside the window moved or freed the objects while
    // their `Heap`/raw copies kept pre-GC addresses — the traced instance
    // data then pointed into recycled nursery memory and the next minor GC
    // tenured garbage through it (`TraceIonJSFrame` /
    // `TraceExactStackRootList` → `promoteObject` SIGSEGV on the worklet
    // thread — the e126 crash). The final `Heap` slots are re-derived from
    // the rooted values so they carry post-GC addresses, never pre-GC
    // copies; raw copies never span an allocation.
    rooted_vec!(let mut instance_roots);

    let mut input_leaf_indices: Vec<Vec<usize>> = Vec::with_capacity(shape.input_ports as usize);
    for _ in 0..shape.input_ports {
        let mut port_indices = Vec::with_capacity(shape.input_channels as usize);
        for _ in 0..shape.input_channels {
            match make_channel_array_index(cx, 128, &mut instance_roots) {
                Some(index) => port_indices.push(index),
                None => {
                    debug!("AudioWorklet channel array creation failed for node {node_key}.");
                    latch_failure(&node, &bridge, &main_sender);
                    return;
                },
            }
        }
        input_leaf_indices.push(port_indices);
    }
    let mut output_leaf_indices: Vec<Vec<usize>> = Vec::with_capacity(shape.output_ports as usize);
    for port in 0..shape.output_ports {
        let mut port_indices = Vec::new();
        for _ in 0..shape.output_channels[port as usize].max(1) {
            match make_channel_array_index(cx, 128, &mut instance_roots) {
                Some(index) => port_indices.push(index),
                None => {
                    debug!("AudioWorklet channel array creation failed for node {node_key}.");
                    latch_failure(&node, &bridge, &main_sender);
                    return;
                },
            }
        }
        output_leaf_indices.push(port_indices);
    }
    let mut param_leaf_indices = Vec::with_capacity(shape.params.len());
    for param in &shape.params {
        let len = if param.rate == ParamRate::KRate { 1 } else { 128 };
        match make_channel_array_index(cx, len, &mut instance_roots) {
            Some(index) => param_leaf_indices.push(index),
            None => {
                debug!("AudioWorklet param array creation failed for node {node_key}.");
                latch_failure(&node, &bridge, &main_sender);
                return;
            },
        }
    }

    // Outer argument containers: `inputs[p][c]`, `outputs[p][c]` arrays and
    // the `params` object keyed by parameter name. Each wrapper is rooted at
    // creation; parent levels reference the children only through their root
    // indices, re-read under a fresh borrow at use.
    let mut input_port_indices = Vec::with_capacity(input_leaf_indices.len());
    for port_indices in &input_leaf_indices {
        match outer_array_index(cx, &mut instance_roots, port_indices) {
            Some(index) => input_port_indices.push(index),
            None => {
                debug!("AudioWorklet input wrapper creation failed for node {node_key}.");
                latch_failure(&node, &bridge, &main_sender);
                return;
            },
        }
    }
    let inputs_array_index = match outer_array_index(cx, &mut instance_roots, &input_port_indices)
    {
        Some(index) => index,
        None => {
            debug!("AudioWorklet inputs array creation failed for node {node_key}.");
            latch_failure(&node, &bridge, &main_sender);
            return;
        },
    };
    let mut output_port_indices = Vec::with_capacity(output_leaf_indices.len());
    for port_indices in &output_leaf_indices {
        match outer_array_index(cx, &mut instance_roots, port_indices) {
            Some(index) => output_port_indices.push(index),
            None => {
                debug!("AudioWorklet output wrapper creation failed for node {node_key}.");
                latch_failure(&node, &bridge, &main_sender);
                return;
            },
        }
    }
    let outputs_array_index = match outer_array_index(cx, &mut instance_roots, &output_port_indices)
    {
        Some(index) => index,
        None => {
            debug!("AudioWorklet outputs array creation failed for node {node_key}.");
            latch_failure(&node, &bridge, &main_sender);
            return;
        },
    };
    let params_object = unsafe { JS_NewObject(cx, std::ptr::null()) };
    if params_object.is_null() {
        debug!("AudioWorklet params object creation failed for node {node_key}.");
        latch_failure(&node, &bridge, &main_sender);
        return;
    }
    instance_roots.push(ObjectValue(params_object));
    let params_object_index = instance_roots.len() - 1;
    for (index, param) in shape.params.iter().enumerate() {
        if index >= param_leaf_indices.len() {
            break;
        }
        rooted!(&in(cx) let value = instance_roots[param_leaf_indices[index]]);
        rooted!(&in(cx) let params_ref = instance_roots[params_object_index].to_object());
        let Ok(name) = std::ffi::CString::new(param.name.clone()) else {
            continue;
        };
        unsafe {
            JS_SetProperty(cx, params_ref.handle(), name.as_ptr(), value.handle());
        }
    }
    // (e127) Register-then-set for the five GC `Heap` slots. `Heap::set`'s
    // post-write barrier registers the slot's *current* address in the GC
    // store buffer, and mozjs-sys (`jsgc.rs:341-345`) documents
    // constructing a temporary `Heap`, setting it and then moving it as
    // unsafe. The previous `heap()` closure did exactly that on this
    // function's stack frame: the registered stack-address edges dangled
    // after the struct was moved into the registry, and every later
    // worklet-thread minor GC then read arbitrary stack remnants through
    // them (minting stale OBJECT values into the traced root set — the
    // e126/e127 minor-GC SIGSEGV) and wrote promoted pointers back through
    // them, corrupting live frames. The slots are now created empty; the
    // struct reaches its final registry address first, and only then are
    // the slots written — from values that stayed rooted the whole window
    // (`instance`, `instance_roots`, and the worklet global's reflector
    // handle). No JS runs between registration and the write-back.
    let instance_data = ProcessorInstanceData {
        instance: Heap::default(),
        global: Heap::default(),
        inputs_array: Heap::default(),
        outputs_array: Heap::default(),
        params_object: Heap::default(),
        input_arrays: input_leaf_indices
            .iter()
            .map(|port| {
                port.iter()
                    .map(|&index| rooted_channel(cx, &instance_roots, index))
                    .collect()
            })
            .collect(),
        output_arrays: output_leaf_indices
            .iter()
            .map(|port| {
                port.iter()
                    .map(|&index| rooted_channel(cx, &instance_roots, index))
                    .collect()
            })
            .collect(),
        param_shapes: shape
            .params
            .iter()
            .map(|param| {
                let len = if param.rate == ParamRate::KRate { 1 } else { 128 };
                // An interior-NUL name (pathological WebIDL edge) has no C
                // face: the empty sentinel skips that name in the read-back
                // while keeping this vector index-aligned with
                // `quantum.params` (the pre-e155 Set loop skipped it the
                // same way — no property, node keeps working).
                (std::ffi::CString::new(param.name.as_str()).unwrap_or_default(), len)
            })
            .collect(),
        input_live: (0..shape.input_ports)
            .map(|_| shape.input_channels)
            .collect(),
        output_live: shape.output_channels.clone(),
        port: crate::dom::bindings::root::Dom::from_ref(&*port),
    };
    scope.register_processor_instance(node_key, instance_data);
    let global_obj = scope
        .upcast::<crate::dom::globalscope::GlobalScope>()
        .reflector()
        .get_jsobject();
    if !scope.set_instance_heap_slots(
        node_key,
        instance.get(),
        global_obj.get(),
        instance_roots[inputs_array_index].to_object(),
        instance_roots[outputs_array_index].to_object(),
        instance_roots[params_object_index].to_object(),
    ) {
        debug!("AudioWorklet instance heap slot finalization failed for node {node_key}.");
        latch_failure(&node, &bridge, &main_sender);
        return;
    }

    // (e147) The spec hands `process()` frozen array containers
    // (`Object.isFrozen(inputs) && Object.isFrozen(inputs[0])` — the
    // FrozenArray shape): freeze the outer argument arrays and every
    // port-level wrapper once, before the first scheduling. The channel
    // `Float32Array`s and their buffers stay writable/transferable (the
    // pump's input copy, output read-back and zeroing write through the
    // typed-array views, which container freezing does not touch).
    {
        use js::rust::wrappers2::JS_FreezeObject;
        fn freeze_argument_containers(
            cx: &mut JSContext,
            roots: &js::gc::RootedVec<'_, js::jsval::JSVal>,
            outer: usize,
            ports: &[usize],
        ) -> bool {
            let mut ok = true;
            rooted!(&in(cx) let outer_obj = roots[outer].to_object());
            ok &= unsafe { JS_FreezeObject(cx, outer_obj.handle()) };
            for &port in ports {
                rooted!(&in(cx) let port_obj = roots[port].to_object());
                ok &= unsafe { JS_FreezeObject(cx, port_obj.handle()) };
            }
            ok
        }
        let frozen_inputs = freeze_argument_containers(
            cx,
            &instance_roots,
            inputs_array_index,
            &input_port_indices,
        );
        let frozen_outputs = freeze_argument_containers(
            cx,
            &instance_roots,
            outputs_array_index,
            &output_port_indices,
        );
        // (e155) The `parameters` object gets the same spec face
        // (SetIntegrityLevel(parameter, frozen) — the reference clone always
        // freezes what reaches `process()`); its `Float32Array`s stay
        // writable through their buffers.
        let frozen_params = {
            rooted!(&in(cx) let params_obj = instance_roots[params_object_index].to_object());
            unsafe { JS_FreezeObject(cx, params_obj.handle()) }
        };
        if !frozen_inputs || !frozen_outputs || !frozen_params {
            debug!("AudioWorklet argument array freezing failed for node {node_key}.");
            latch_failure(&node, &bridge, &main_sender);
            return;
        }
    }

    // Route the processor-side port through the conduit's `to_main` ring
    // (lane 0 — the node's own port pair). (e147) First wiring wins: a
    // singleton-style shared instance hands the SAME port object to every
    // node that instantiates it, and re-wiring it here would steal the
    // routing of the first node's conduit (probe: the second message landed
    // on node2's port; Chromium keeps node1). A fresh instance's port was
    // already wired to this same conduit/lane by the base-construction
    // handoff, so the guard is a no-op there; a bare un-entangled port (the
    // exotic construction shapes) still gets wired.
    if !port.has_bao_port_redirect() {
        port.set_bao_port_redirect(PortRedirect {
            conduit: conduit.clone(),
            direction: PortDirection::ToMain { lane: 0 },
        });
    }

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
    // The minted `processorOptions` lane ports (traced here — the drain
    // re-reads them per wake).
    scope.register_port_lanes(node_key, lane_ports);
    scope.drain_audio_pumps(cx);
}

/// Walk `path` from the deserialized options root and set `value` at its
/// end (the worklet-thread half of the e114 transfer-substitution: the
/// minted lane port is placed back exactly where the substituted
/// `MessagePort` sat). Returns false when the path no longer resolves
/// (a hostile deserialization edge — the caller latches the failure).
#[expect(unsafe_code)]
fn set_option_path_value(
    cx: &mut JSContext,
    root: js::rust::Handle<js::jsval::JSVal>,
    path: &[crate::dom::audio::audioworkletnode::PortPathSeg],
    value: js::rust::Handle<js::jsval::JSVal>,
) -> bool {
    use crate::dom::audio::audioworkletnode::PortPathSeg;
    use js::rust::wrappers2::{JS_GetElement, JS_GetProperty, JS_SetElement, JS_SetProperty};

    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    rooted!(&in(cx) let mut current = root.get());
    for seg in parents {
        if !current.is_object() {
            return false;
        }
        rooted!(&in(cx) let obj = current.to_object());
        rooted!(&in(cx) let mut child = UndefinedValue());
        let ok = match seg {
            PortPathSeg::Key(key) => {
                let Ok(c_key) = std::ffi::CString::new(key.as_bytes()) else {
                    return false;
                };
                unsafe { JS_GetProperty(cx, obj.handle(), c_key.as_ptr(), child.handle_mut()) }
            },
            PortPathSeg::Index(index) => unsafe {
                JS_GetElement(cx, obj.handle(), *index, child.handle_mut())
            },
        };
        if !ok {
            unsafe { JS_ClearPendingException(cx) };
            return false;
        }
        current.set(child.get());
    }
    if !current.is_object() {
        return false;
    }
    rooted!(&in(cx) let obj = current.to_object());
    match last {
        PortPathSeg::Key(key) => {
            let Ok(c_key) = std::ffi::CString::new(key.as_bytes()) else {
                return false;
            };
            unsafe { JS_SetProperty(cx, obj.handle(), c_key.as_ptr(), value) }
        },
        PortPathSeg::Index(index) => unsafe {
            JS_SetElement(cx, obj.handle(), *index, value)
        },
    }
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
        node.fire_processorerror_once(cx, None);
    });
    let msg = CommonScriptMsg::Task(
        ScriptThreadEventCategory::WorkletEvent,
        Box::new(task),
        None,
        TaskSourceName::DOMManipulation,
    );
    let _ = sender.send(MainThreadScriptMsg::Common(msg));
}
