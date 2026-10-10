/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletGlobalScope
//
// (Bao 段(1), user ruling 2026-10-05): upstream servo has zero AudioWorklet
// runtime (origin/main 2026-10-05 audit: only fetch-pipeline destination
// strings + WPT meta). Mirrors PaintWorkletGlobalScope's scope shape (the
// worklet engine is upstream-shared; zero new architecture).
//
// 段(3) wiring adds the worklet-thread half of the render bridge: the
// processor instance registry (traced — the instance and its persistent
// Float32Arrays must survive and be relocated by GC), the block-rate pump
// registry (untraced, thread-confined — no GC pointers), the shared
// processor registry (name → parameter descriptors, published to the script
// thread), and the pump drain the wake hook posts into.
//
// Thread placement: this scope lives on the worklet engine's own thread(s)
// (WorkletGlobalScopeType::Audio). It carries NO JS objects across threads:
// the audio face is an Arc<Mutex<servo-media AudioContext>> handle plus a
// copied sample_rate scalar and an Arc<Mutex<registry>> of plain descriptor
// data — the only cross-thread shapes allowed under the bao cross-thread
// JSObject rule.

use std::cell::Cell;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::{Heap, IsConstructor, JSObject, Value};
use js::rust::wrappers2::JS_GetElement;
use js::rust::Handle;
use js::jsval::{JSVal, ObjectValue, UndefinedValue};
use script_bindings::cell::DomRefCell;
use script_bindings::interfaces::HasOrigin;
use servo_media::audio::context::AudioContext;
use servo_media::audio::param::ParamRate;
use servo_url::{MutableOrigin, ServoUrl};
use stylo_atoms::Atom;

use crate::dom::audio::audioworklethandler::{ParamDescriptor, SharedProcessorRegistry};
use crate::dom::audio::audioworkletport::AudioWorkletPortConduit;
use script_bindings::callback::{HasCallbackHolder, RootedCallback};
use crate::dom::bindings::codegen::Bindings::VoidFunctionBinding::VoidFunction;
use crate::dom::bindings::codegen::Bindings::AudioWorkletGlobalScopeBinding::{
    self, AudioWorkletGlobalScopeMethods,
};
use crate::dom::bindings::conversions::get_property;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::bindings::trace::HashMapTracedValues;
use servo_base::id::PipelineId;
use crate::dom::globalscope::messageport::MessagePort;
use crate::dom::worklet::WorkletExecutor;
use crate::dom::workletglobalscope::{WorkletGlobalScope, WorkletGlobalScopeInit};
use js::typedarray::Float32;
use servo_media::audio::audioworklet_node::{AudioWorkletPump, WorkletQuantum};
use servo_media::audio::block::FRAMES_PER_BLOCK_USIZE;

/// The per-`AudioContext` audio face carried into the AudioWorklet realm.
///
/// Plumbs the LIVE servo-media handle of the creating `BaseAudioContext`
/// through `WorkletGlobalScopeInit.audio` so `currentTime`/`currentFrame`/
/// `sampleRate` report real values on the worklet thread, and the shared
/// processor registry so `new AudioWorkletNode` on the script thread can
/// validate names and read parameter descriptors. Every member is an
/// `Arc<...>`/copied scalar (no JS objects) — the only cross-thread shapes
/// allowed under the bao cross-thread JSObject rule.
#[derive(Clone, MallocSizeOf)]
pub(crate) struct AudioWorkletScopeData {
    /// Live handle to the creating context's servo-media audio graph.
    #[ignore_malloc_size_of = "servo_media"]
    audio_context: Arc<Mutex<AudioContext>>,
    /// The creating context's sample rate (copied scalar).
    sample_rate: f32,
    /// The shared processor registry (name → parameter descriptors).
    #[ignore_malloc_size_of = "descriptors are small"]
    registry: Arc<SharedProcessorRegistry>,
}

impl AudioWorkletScopeData {
    pub(crate) fn new(
        audio_context: Arc<Mutex<AudioContext>>,
        sample_rate: f32,
        registry: Arc<SharedProcessorRegistry>,
    ) -> Self {
        AudioWorkletScopeData {
            audio_context,
            sample_rate,
            registry,
        }
    }

    pub(crate) fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub(crate) fn registry(&self) -> Arc<SharedProcessorRegistry> {
        self.registry.clone()
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-baseaudiocontext-currenttime>
    pub(crate) fn current_time(&self) -> f64 {
        self.audio_context
            .lock()
            .expect("Locking the servo-media audio context")
            .current_time()
    }
}

/// Which argument object of a registered instance to read.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RootedArraySlot {
    Inputs,
    Outputs,
    Params,
}

/// The traced, GC-managed half of one node's processor instance (worklet
/// thread). Every JS-object member is a traced slot: the pump handler reads
/// these slots fresh per quantum, so collection/relocation is honoured
/// structurally (the W28/W30 class) and nothing here needs raw roots.
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
pub(crate) struct ProcessorInstanceData {
    /// The processor instance object.
    #[ignore_malloc_size_of = "mozjs"]
    pub(crate) instance: Heap<*mut JSObject>,
    /// The worklet global (realm entry anchor for each block).
    #[ignore_malloc_size_of = "mozjs"]
    pub(crate) global: Heap<*mut JSObject>,
    /// `inputs[p][c]` outer array object.
    #[ignore_malloc_size_of = "mozjs"]
    pub(crate) inputs_array: Heap<*mut JSObject>,
    /// `outputs[p][c]` outer array object.
    #[ignore_malloc_size_of = "mozjs"]
    pub(crate) outputs_array: Heap<*mut JSObject>,
    /// `{name: Float32Array}` parameter object.
    #[ignore_malloc_size_of = "mozjs"]
    pub(crate) params_object: Heap<*mut JSObject>,
    /// Per-input-port channel arrays, `[port][channel]`, each
    /// `Float32Array(128)`. Not heap-measured: the payload lives on the JS
    /// heap, already accounted by the engine's memory reporting.
    #[ignore_malloc_size_of = "JS-heap payload, wrapper is a slot"]
    pub(crate) input_arrays:
        Vec<Vec<crate::dom::bindings::buffer_source::HeapBufferSource<Float32>>>,
    /// Per-output-port channel arrays.
    #[ignore_malloc_size_of = "JS-heap payload, wrapper is a slot"]
    pub(crate) output_arrays:
        Vec<Vec<crate::dom::bindings::buffer_source::HeapBufferSource<Float32>>>,
    /// (e155) Declared parameter faces: name + the array length `process()`
    /// exposes for it (k-rate 1, a-rate 128). The per-quantum parameters
    /// read-back validates the JS params object against this face and
    /// rebuilds it on mismatch (the crbug.com/1151069 semantics); the arrays
    /// themselves are owned by the params object — the copy path always goes
    /// through a fresh property `Get`, so no Rust-side handles are kept.
    #[no_trace = "plain shape data"]
    pub(crate) param_shapes: Vec<(std::ffi::CString, usize)>,
    /// (e147) The per-port channel counts the JS `inputs` argument arrays
    /// are currently shaped to — the spec's dynamic input face: `inputs[p]`
    /// is empty while port `p` has no connection, and carries the connected
    /// bus's channel count otherwise. Compared against every quantum's
    /// `WorkletQuantum::input_live`; a mismatch rebuilds the input arrays
    /// before `process()` runs.
    #[no_trace = "plain shape data"]
    pub(crate) input_live: Vec<u8>,
    /// (e147) The per-port channel counts the JS `outputs` argument arrays
    /// are currently shaped to (see
    /// [`WorkletQuantum::output_live`](servo_media::audio::audioworklet_node::WorkletQuantum)):
    /// the explicit `outputChannelCount` entry, else the spec's computed
    /// count. Compared every quantum; a mismatch rebuilds before
    /// `process()`.
    #[no_trace = "plain shape data"]
    pub(crate) output_live: Vec<u8>,
    /// The processor-side port (its `postMessage` is redirected through the
    /// node's conduit).
    pub(crate) port: Dom<MessagePort>,
}

impl ProcessorInstanceData {
    /// Copy the quantum's inputs into the persistent channel arrays. No JS
    /// runs in here (typed-array view reads only), so the borrows are short
    /// by construction. Returns `false` when an array is missing/neutered
    /// (the caller latches the failure). (e155) The parameter timelines moved
    /// out of this copy: they are written through the per-quantum
    /// parameters read-back in [`crate::dom::audio::audioworklethandler`],
    /// which resolves each declared name off the JS params object.
    pub(crate) fn write_inputs(&self, cx: &JSContext, quantum: &WorkletQuantum) -> bool {
        for (port, buffers) in self.input_arrays.iter().enumerate() {
            let Some(quantum_input) = quantum.inputs.get(port) else {
                break;
            };
            for (channel, arr) in buffers.iter().enumerate() {
                let Ok(mut view) = arr.get_typed_array() else {
                    return false;
                };
                let Some(slice) = view.as_mut_slice_safe(cx.no_gc()) else {
                    return false;
                };
                if channel < quantum_input.channels() as usize {
                    let source = quantum_input.chan(channel as u8);
                    let len = slice.len().min(source.len());
                    slice[..len].copy_from_slice(&source[..len]);
                    slice[len..].fill(0.);
                } else {
                    slice.fill(0.);
                }
            }
        }
        true
    }

    /// Copy the processor's output arrays back into the quantum, then zero
    /// them. Called after `process()` returned without throwing; a GC inside
    /// the call has been reflected into the traced slots already.
    ///
    /// (e147) The zeroing is the spec's "outputs are zero-initialized for
    /// each `process()` call" face on the JS side: the arrays are persistent
    /// across quanta, so a processor that exits without writing (the
    /// zero-outputs pulse shape) must not see — or render — the previous
    /// quantum's samples. The channel `Float32Array`s stay writable (only
    /// the array *containers* are frozen); zeroing writes through the same
    /// typed-array views the pump uses.
    pub(crate) fn read_outputs(&self, cx: &JSContext, quantum: &mut WorkletQuantum) {
        for (port, buffers) in self.output_arrays.iter().enumerate() {
            let Some(quantum_output) = quantum.outputs.get_mut(port) else {
                break;
            };
            for (channel, arr) in buffers.iter().enumerate() {
                let Ok(mut view) = arr.get_typed_array() else {
                    continue;
                };
                let Some(slice) = view.as_mut_slice_safe(cx.no_gc()) else {
                    continue;
                };
                if channel < quantum_output.channels() as usize {
                    let len = slice.len().min(FRAMES_PER_BLOCK_USIZE);
                    quantum_output.chan_mut(channel as u8)[..len]
                        .copy_from_slice(&slice[..len]);
                }
                slice.fill(0.);
            }
        }
    }
}

/// The untraced, thread-confined half of one node's pump: the media-side
/// pump plus its conduit handle. The pump's handler re-reads the traced
/// instance data per quantum, so this struct deliberately holds no GC
/// pointers and never needs tracing. Created on, drained on, and dropped on
/// the owning worklet thread.
pub(crate) struct NodePump {
    pub(crate) node_key: u64,
    pub(crate) conduit: Arc<AudioWorkletPortConduit>,
    pub(crate) pump: AudioWorkletPump,
}

/// The registered processor constructor (data plane). Consumed by the 段(3)
/// instantiation task when the first node for the name renders.
#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct ProcessorDefinition {
    #[ignore_malloc_size_of = "mozjs"]
    processor_ctor: Heap<JSVal>,
}

impl ProcessorDefinition {
    fn new(processor_ctor: Handle<Value>) -> Box<ProcessorDefinition> {
        let result = Box::new(ProcessorDefinition {
            processor_ctor: Heap::default(),
        });
        result.processor_ctor.set(processor_ctor.get());
        result
    }
}

/// <https://webaudio.github.io/web-audio-api/#AudioWorkletGlobalScope>
#[dom_struct]
pub(crate) struct AudioWorkletGlobalScope {
    /// The worklet global for this object.
    worklet_global: WorkletGlobalScope,
    /// The audio face of the creating `AudioContext`.
    #[no_trace]
    audio: AudioWorkletScopeData,
    /// name → registered processor constructor (data plane). Consumed by
    /// the 段(3) instantiation task when the first node for the name renders.
    processor_registry: DomRefCell<HashMapTracedValues<Atom, Box<ProcessorDefinition>>>,
    /// node key → traced processor instance data (worklet thread).
    processor_instances: DomRefCell<HashMapTracedValues<u64, Box<ProcessorInstanceData>>>,
    /// node key → the minted worklet-side endpoints of the node's
    /// `processorOptions` port lanes (e114; entry `i` is lane `i + 1`).
    /// Traced — the drain re-reads the slots per wake.
    port_lane_ports: DomRefCell<HashMapTracedValues<u64, Vec<Dom<MessagePort>>>>,
    /// The block-rate pumps (untraced, thread-confined — see [`NodePump`]).
    #[no_trace]
    #[ignore_malloc_size_of = "media pump, no heap-owned GC payload"]
    audio_pumps: DomRefCell<Vec<NodePump>>,
    /// (e176) The clock of the quantum whose `process()` call is currently
    /// on this thread's stack: `(frame, time)` snapshotted from the
    /// `WorkletQuantum` payload. `currentFrame`/`currentTime` read this
    /// while set — the spec's clock for a block is fixed for the duration
    /// of its `process()` call, while the previous live render-thread
    /// roundtrip could advance *mid-call* (a processor reading the getter
    /// twice — e.g. WPT's shared `worklet-recorder.js`, which sizes a
    /// `Float32Array.set` offset from `currentFrame` — observed an
    /// inconsistent pair across the recording's final block boundary and
    /// threw "invalid or out-of-range index", latching processorerror and
    /// silently muting the node). Outside `process()` (constructor bodies,
    /// port handlers) the slot is empty and the getters keep the live
    /// roundtrip.
    #[ignore_malloc_size_of = "plain clock data"]
    process_quantum_clock: Cell<Option<(u64, f64)>>,
    /// The in-flight instantiation's base-construction handoff (e122):
    /// installed by `instantiate_processor` *before* invoking the registered
    /// constructor, so the `AudioWorkletProcessor` base constructor wires
    /// its freshly minted port into the node's lane-0 conduit immediately —
    /// a constructor-body `this.port.postMessage` already rides the ring
    /// instead of the dead constellation path (the spec's port is part of
    /// the base-constructed instance, not a post-construction wiring step).
    /// `Consumed` marks the one allowed construction of this instantiation
    /// as spent — any further `new AudioWorkletProcessor()` while the
    /// instantiation is in flight is the spec constructor-reentrancy
    /// TypeError face (processor-construction-port WPT battery).
    #[no_trace]
    #[ignore_malloc_size_of = "no heap-owned GC payload"]
    pending_construction: DomRefCell<Option<PendingProcessorConstruction>>,
}

/// The state of the in-flight processor instantiation's construction
/// handoff (see [`AudioWorkletGlobalScope::pending_construction`]).
pub(crate) enum PendingProcessorConstruction {
    /// The next `AudioWorkletProcessor` construction takes this conduit for
    /// its port's redirect (lane 0).
    Fresh(Arc<AudioWorkletPortConduit>),
    /// The instantiation's one allowed construction already happened.
    Consumed,
}

/// What the `AudioWorkletProcessor` base constructor should do with the
/// minted port (see
/// [`AudioWorkletGlobalScope::take_pending_processor_construction`]).
pub(crate) enum ConstructionHandoff {
    /// Wire the minted port into this conduit (lane 0) — the
    /// instantiation's one allowed construction.
    Wire(Arc<AudioWorkletPortConduit>),
    /// An instantiation is in flight but its construction is spent: the
    /// spec's constructor-reentrancy TypeError face.
    Reentrant,
    /// No instantiation in flight: a plain, un-entangled port.
    Bare,
}

impl AudioWorkletGlobalScope {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cx: &mut JSContext,
        pipeline_id: PipelineId,
        base_url: ServoUrl,
        inherited_secure_context: Option<bool>,
        executor: WorkletExecutor,
        init: &WorkletGlobalScopeInit,
        closing: Arc<AtomicBool>,
    ) -> DomRoot<AudioWorkletGlobalScope> {
        debug!(
            "Creating audio worklet global scope for pipeline {}.",
            pipeline_id
        );
        let audio = init
            .audio
            .clone()
            .expect("AudioWorkletGlobalScope requires audio scope data");
        let global = Box::new(AudioWorkletGlobalScope {
            worklet_global: WorkletGlobalScope::new_inherited(
                pipeline_id,
                base_url,
                inherited_secure_context,
                executor,
                init,
                closing,
            ),
            audio,
            processor_registry: Default::default(),
            processor_instances: Default::default(),
            port_lane_ports: Default::default(),
            audio_pumps: DomRefCell::new(Vec::new()),
            process_quantum_clock: Cell::new(None),
            pending_construction: DomRefCell::new(None),
        });
        let origin = global.worklet_global.origin();
        AudioWorkletGlobalScopeBinding::Wrap::<crate::DomTypeHolder>(cx, &origin, global)
    }

    pub(crate) fn audio(&self) -> &AudioWorkletScopeData {
        &self.audio
    }

    // ── e176: the in-flight quantum's clock (see the field docs) ──

    /// Pin `currentFrame`/`currentTime` to `process()`'s quantum (worklet
    /// thread, called by `WorkletProcessorHandler::process_quantum` right
    /// before invoking `process()`).
    pub(crate) fn pin_process_quantum_clock(&self, frame: u64, time: f64) {
        self.process_quantum_clock.set(Some((frame, time)));
    }

    /// Release the pin when the `process()` call returns.
    pub(crate) fn unpin_process_quantum_clock(&self) {
        self.process_quantum_clock.set(None);
    }

    // ── e122: the in-flight instantiation's construction handoff ──

    /// Install the handoff for the instantiation that is about to invoke
    /// its registered constructor (worklet thread; see
    /// `instantiate_processor`).
    pub(crate) fn set_pending_processor_construction(
        &self,
        pending: PendingProcessorConstruction,
    ) {
        *self.pending_construction.borrow_mut() = Some(pending);
    }

    /// Take the handoff for the `AudioWorkletProcessor` base constructor:
    /// `Fresh` yields the conduit (flipping the slot to `Consumed` — the
    /// instantiation's one allowed construction), `Consumed` reports the
    /// reentrancy face, `None` means no instantiation is in flight.
    pub(crate) fn take_pending_processor_construction(&self) -> ConstructionHandoff {
        let mut slot = self.pending_construction.borrow_mut();
        match slot.take() {
            Some(PendingProcessorConstruction::Fresh(conduit)) => {
                *slot = Some(PendingProcessorConstruction::Consumed);
                ConstructionHandoff::Wire(conduit)
            },
            Some(PendingProcessorConstruction::Consumed) => ConstructionHandoff::Reentrant,
            None => ConstructionHandoff::Bare,
        }
    }

    /// Clear the handoff once the registered constructor returned (both the
    /// success and every failure arm — a stale `Fresh` slot would mis-wire
    /// the next unrelated base construction).
    pub(crate) fn clear_pending_processor_construction(&self) {
        *self.pending_construction.borrow_mut() = None;
    }

    // ── 段(3) wiring: instance/pump registries and the block-rate drain ──

    /// The registered constructor value for `name` (the data-plane registry
    /// this scope's module evaluation populated). Returns a plain `JSVal`
    /// copy — the caller roots it before any JS runs.
    pub(crate) fn processor_ctor(&self, name: &Atom) -> Option<JSVal> {
        self.processor_registry
            .borrow()
            .get(name)
            .map(|definition| definition.processor_ctor.get())
    }

    /// The traced instance data for `node_key`, as a guard on the registry
    /// borrow. Nothing re-enters this cell while JS runs (registration is
    /// instantiation-time only), so holding the guard across `process()` is
    /// safe; GC tracing does not touch Rust-side borrows.
    pub(crate) fn instance_data(
        &self,
        node_key: u64,
    ) -> Option<std::cell::Ref<'_, Box<ProcessorInstanceData>>> {
        std::cell::Ref::filter_map(self.processor_instances.borrow(), |map| map.get(&node_key))
            .ok()
    }

    /// One of the instance's rooted argument objects, by slot.
    pub(crate) fn rooted_array(&self, node_key: u64, slot: RootedArraySlot) -> *mut JSObject {
        let Some(inst) = self.instance_data(node_key) else {
            return std::ptr::null_mut();
        };
        match slot {
            RootedArraySlot::Inputs => inst.inputs_array.get(),
            RootedArraySlot::Outputs => inst.outputs_array.get(),
            RootedArraySlot::Params => inst.params_object.get(),
        }
    }

    /// Register the traced instance data for a freshly instantiated
    /// processor (worklet thread; replaces any previous instance for the
    /// same node key).
    pub(crate) fn register_processor_instance(&self, node_key: u64, data: ProcessorInstanceData) {
        self.processor_instances
            .borrow_mut()
            .insert(node_key, Box::new(data));
    }

    /// (e147) Swap one registered instance's JS `inputs` argument face to a
    /// new per-port channel shape (see
    /// [`ProcessorInstanceData::input_live`]). The `inputs_array` inline
    /// `Heap` slot is written here — at its final registry address (the
    /// e127 register-then-set discipline); the channel handles are boxed
    /// `Heap`s, safe to move into the field.
    pub(crate) fn update_input_face(
        &self,
        node_key: u64,
        outer: *mut JSObject,
        arrays: Vec<Vec<crate::dom::bindings::buffer_source::HeapBufferSource<Float32>>>,
        live: &[u8],
    ) -> bool {
        let mut instances = self.processor_instances.borrow_mut();
        let Some(inst) = instances.0.get_mut(&node_key) else {
            return false;
        };
        inst.inputs_array.set(outer);
        inst.input_arrays = arrays;
        inst.input_live = live.to_vec();
        true
    }

    /// (e147) The output-face twin of [`Self::update_input_face`].
    pub(crate) fn update_output_face(
        &self,
        node_key: u64,
        outer: *mut JSObject,
        arrays: Vec<Vec<crate::dom::bindings::buffer_source::HeapBufferSource<Float32>>>,
        live: &[u8],
    ) -> bool {
        let mut instances = self.processor_instances.borrow_mut();
        let Some(inst) = instances.0.get_mut(&node_key) else {
            return false;
        };
        inst.outputs_array.set(outer);
        inst.output_arrays = arrays;
        inst.output_live = live.to_vec();
        true
    }

    /// (e155) Swap one registered instance's JS `parameters` argument object
    /// for a freshly built one (the rebuild half of the crbug.com/1151069
    /// read-back: an own-data-property clone, frozen, shadowing any
    /// page-installed `Object.prototype` accessor). Same discipline as
    /// [`Self::update_input_face`]: the inline `params_object` `Heap` slot is
    /// written here, at its final registry address.
    pub(crate) fn update_params_face(
        &self,
        node_key: u64,
        params_object: *mut JSObject,
    ) -> bool {
        let mut instances = self.processor_instances.borrow_mut();
        instances
            .0
            .get_mut(&node_key)
            .map(|inst| inst.params_object.set(params_object))
            .is_some()
    }

    /// (e127) Post-registration write of the instance's five GC `Heap`
    /// slots — the register-then-set discipline. `Heap::set`'s post-write
    /// barrier registers the slot's current address in the store buffer,
    /// and mozjs-sys (`jsgc.rs:341-345`) forbids setting a temporary
    /// `Heap` and then moving it, so these slots must only be written once
    /// the `ProcessorInstanceData` box has reached its final registry
    /// address (writing them on the stack-constructed struct left dangling
    /// stack-address store-buffer edges that every later worklet minor GC
    /// read and wrote through — the e126/e127 SIGSEGV class). The caller
    /// guarantees no JS runs between registration and this call.
    pub(crate) fn set_instance_heap_slots(
        &self,
        node_key: u64,
        instance: *mut JSObject,
        global: *mut JSObject,
        inputs_array: *mut JSObject,
        outputs_array: *mut JSObject,
        params_object: *mut JSObject,
    ) -> bool {
        let mut instances = self.processor_instances.borrow_mut();
        instances
            .0
            .get_mut(&node_key)
            .map(|inst| {
                inst.instance.set(instance);
                inst.global.set(global);
                inst.inputs_array.set(inputs_array);
                inst.outputs_array.set(outputs_array);
                inst.params_object.set(params_object);
            })
            .is_some()
    }

    /// Register the block-rate pump for a node (worklet thread).
    pub(crate) fn register_pump(
        &self,
        node_key: u64,
        conduit: Arc<AudioWorkletPortConduit>,
        pump: AudioWorkletPump,
    ) {
        self.audio_pumps.borrow_mut().push(NodePump {
            node_key,
            conduit,
            pump,
        });
    }

    /// Register the minted worklet-side endpoints of a node's
    /// `processorOptions` port lanes (e114; worklet thread, called at
    /// instantiation right after `register_pump`).
    pub(crate) fn register_port_lanes(
        &self,
        node_key: u64,
        ports: Vec<DomRoot<MessagePort>>,
    ) {
        self.port_lane_ports
            .borrow_mut()
            .0
            .entry(node_key)
            .or_default()
            .extend(ports.into_iter().map(|port| Dom::from_ref(&*port)));
    }

    /// 段(3) teardown (worklet thread, while this thread's runtime is alive):
    /// flush the SM store buffer BEFORE the instance Heap slots free.
    /// `Heap::set` records nursery edges against the slot addresses; freeing
    /// the slots with pending edges leaves the next nursery collection
    /// reading freed memory (the teardown SIGSEGV class). A nursery GC
    /// empties the store buffer, after which the drop is edge-free. Also
    /// stops the pumps so queued wake tasks find nothing to run.
    pub(crate) fn teardown_audio(&self, cx: &mut JSContext) {
        self.audio_pumps.borrow_mut().clear();
        #[expect(unsafe_code)]
        {
            // SAFETY: the worklet thread's own runtime context, alive for the
            // call; JS_GC is the documented flush face for the store buffer.
            unsafe { js::jsapi::JS_GC(cx.raw_cx(), js::jsapi::GCReason::API) };
        }
        self.processor_instances.borrow_mut().0.clear();
        self.port_lane_ports.borrow_mut().0.clear();
    }

    /// The block-rate step, posted by the wake hook (render side publishing
    /// a quantum, or a node-port message arriving): run every pump to idle,
    /// then dispatch inbound port payloads. Runs on the worklet thread.
    pub(crate) fn drain_audio_pumps(&self, cx: &mut JSContext) {
        // Quanta first. The pumps borrow is held across `pump_once`, which
        // runs `process()` — no re-entrancy path reaches `drain_audio_pumps`
        // from inside (port posts and worklet tasks only land on the event
        // loop after this task returns), and GC tracing does not touch this
        // untraced cell.
        {
            let mut pumps = self.audio_pumps.borrow_mut();
            for entry in pumps.iter_mut() {
                use servo_media::audio::audioworklet_node::PumpOutcome;
                loop {
                    match entry.pump.pump_once() {
                        PumpOutcome::Delivered => continue,
                        // The output ring is full (render side behind):
                        // stop and let the next wake deliver the staged
                        // quantum.
                        PumpOutcome::Staged => break,
                        PumpOutcome::Idle => break,
                        PumpOutcome::Halted => break,
                    }
                }
            }
        }
        // Then inbound port payloads: lane 0 (the processor's own port) and
        // each `processorOptions` lane (its minted counterpart port). Pop
        // under short borrows, dispatch outside them (dispatch runs script).
        let mut dispatches: Vec<(DomRoot<MessagePort>, crate::dom::audio::audioworkletport::PortPayload)> =
            Vec::new();
        {
            let pumps = self.audio_pumps.borrow();
            let lane_ports = self.port_lane_ports.borrow();
            for entry in pumps.iter() {
                while let Some(payload) = entry.conduit.pop_for_processor() {
                    if let Some(inst) = self.instance_data(entry.node_key) {
                        dispatches.push((DomRoot::from_ref(&*inst.port), payload));
                    }
                }
                for lane in 1..entry.conduit.lane_count() {
                    while let Some(payload) = entry.conduit.pop_for_processor_lane(lane) {
                        let Some(ports) = lane_ports.get(&entry.node_key) else {
                            continue;
                        };
                        let Some(port) = ports.get(lane as usize - 1) else {
                            continue;
                        };
                        dispatches.push((DomRoot::from_ref(&**port), payload));
                    }
                }
            }
        }
        if dispatches.is_empty() {
            return;
        }
        let global = self.upcast::<crate::dom::globalscope::GlobalScope>();
        for (port, payload) in dispatches {
            crate::dom::audio::audioworkletport::dispatch_port_payload(cx, &port, global, payload);
        }
    }
}

impl AudioWorkletGlobalScopeMethods<crate::DomTypeHolder> for AudioWorkletGlobalScope {
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-registerprocessor>
    #[expect(unsafe_code)]
    fn RegisterProcessor(
        &self,
        name: DOMString,
        processor_ctor: RootedCallback<VoidFunction>,
    ) -> Fallible<()> {
        // (Bao) the fork's codegen passes no cx for this member (the
        // typeNeedsCx stub); take the script/worklet thread's active context
        // (serviceworker/cache.rs precedent).
        let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        let cx = &mut cx;
        let name = Atom::from(name);
        rooted!(&in(cx) let ctor_obj = processor_ctor.callback_holder().get());
        rooted!(&in(cx) let ctor_val = ObjectValue(ctor_obj.get()));

        // Step 1. If name is the empty string, throw a NotSupportedError
        // (the RegisterPaint empty-name face).
        if name.is_empty() {
            return Err(Error::Type(c"Empty processor name.".to_owned()));
        }

        // Step 2. Duplicate key in the name-to-processor map: NotSupportedError.
        if self
            .processor_registry
            .borrow()
            .contains_key(&name)
        {
            return Err(Error::NotSupported(None));
        }

        // The registration value must be a constructor (RegisterPaint mirror).
        if unsafe { !IsConstructor(ctor_obj.get()) } {
            return Err(Error::Type(c"Not a constructor.".to_owned()));
        }

        // 段(3): publish the static `parameterDescriptors` (in declaration
        // order) to the shared registry, so the script thread's
        // `new AudioWorkletNode` can validate the name and build the
        // `AudioParamMap` + media parameter list. Extraction failures
        // register an empty parameter list (the node still works; it just
        // declares no parameters).
        let params = extract_parameter_descriptors(cx, ctor_obj.handle());
        if let Ok(mut registry) = self.audio.registry().lock() {
            registry.entry(name.clone()).or_insert(params);
        }

        // Store the constructor (data plane). The AudioWorkletProcessor
        // prototype-chain validation and the per-name instance lifecycle
        // are the 段(3) instantiation face.
        debug!("Registering audio worklet processor {}.", name);
        self.processor_registry
            .borrow_mut()
            .insert(name.clone(), ProcessorDefinition::new(ctor_val.handle()));
        Ok(())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-currentframe>
    fn CurrentFrame(&self) -> Finite<f64> {
        // (e176) Fixed for the duration of a `process()` call: the
        // in-flight quantum's frame, falling back to the live render clock
        // outside `process()` (see the `process_quantum_clock` field).
        match self.process_quantum_clock.get() {
            Some((frame, _)) => Finite::wrap(frame as f64),
            None => Finite::wrap(self.audio.sample_rate() as f64 * self.audio.current_time()),
        }
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-currenttime>
    fn CurrentTime(&self) -> Finite<f64> {
        match self.process_quantum_clock.get() {
            Some((_, time)) => Finite::wrap(time),
            None => Finite::wrap(self.audio.current_time()),
        }
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-samplerate>
    fn SampleRate(&self) -> Finite<f32> {
        Finite::wrap(self.audio.sample_rate())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-renderquantumsize>
    ///
    /// (e122) Always the default 128: servo-media renders at the fixed
    /// quantum — the `renderSizeHint` context option is not plumbed. The
    /// getter's presence keeps rendersizehint-style constructor bodies from
    /// hitting a ReferenceError (the honest completion is a content FAIL on
    /// the size assertion, matching the servo ini expectation).
    fn RenderQuantumSize(&self) -> u32 {
        128
    }
}

/// Read the static `parameterDescriptors` (an array of
/// `{name, defaultValue?, minValue?, maxValue?, automationRate?}`) off a
/// processor constructor. Any read failure yields an empty list — the
/// processor simply declares no parameters.
#[expect(unsafe_code)]
fn extract_parameter_descriptors(
    cx: &mut JSContext,
    ctor: Handle<*mut JSObject>,
) -> Vec<ParamDescriptor> {
    /// One optional numeric descriptor field.
    #[expect(unsafe_code)]
    fn read_f32(
        cx: &mut JSContext,
        entry: Handle<*mut JSObject>,
        prop: &std::ffi::CStr,
    ) -> Option<f32> {
        get_property::<f64>(cx, entry, prop, ()).ok().flatten().map(|value| value as f32)
    }

    let mut descriptors: Vec<ParamDescriptor> = Vec::new();
    let Some(array_value) =
        get_property::<JSVal>(cx, ctor, c"parameterDescriptors", ()).ok().flatten()
    else {
        return descriptors;
    };
    if !array_value.is_object() {
        return descriptors;
    }
    rooted!(&in(cx) let array_object = array_value.to_object());
    let Some(length) =
        get_property::<f64>(cx, array_object.handle(), c"length", ()).ok().flatten()
    else {
        return descriptors;
    };
    for index in 0..(length.clamp(0., 1024.) as u32) {
        rooted!(&in(cx) let mut element = UndefinedValue());
        // SAFETY: pure read; no exception expected, guarded anyway.
        if unsafe { !JS_GetElement(cx, array_object.handle(), index, element.handle_mut()) } ||
            !element.is_object()
        {
            continue;
        }
        rooted!(&in(cx) let entry = element.to_object());
        let Some(name) = get_property::<DOMString>(cx, entry.handle(), c"name", crate::dom::bindings::conversions::StringificationBehavior::Default).ok().flatten()
        else {
            continue;
        };
        let name = String::from(name);
        if name.is_empty() {
            continue;
        }
        let rate = match get_property::<DOMString>(
            cx,
            entry.handle(),
            c"automationRate",
            crate::dom::bindings::conversions::StringificationBehavior::Default,
        )
        .ok()
        .flatten()
        {
            Some(rate) if rate.str() == "k-rate" => ParamRate::KRate,
            _ => ParamRate::ARate,
        };
        descriptors.push(ParamDescriptor {
            name,
            default_value: read_f32(cx, entry.handle(), c"defaultValue").unwrap_or(0.),
            min_value: read_f32(cx, entry.handle(), c"minValue").unwrap_or(f32::NEG_INFINITY),
            max_value: read_f32(cx, entry.handle(), c"maxValue").unwrap_or(f32::INFINITY),
            rate,
        });
    }
    descriptors
}

impl HasOrigin for AudioWorkletGlobalScope {
    fn origin(&self) -> MutableOrigin {
        self.upcast::<WorkletGlobalScope>().origin()
    }
}
