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
use crate::dom::bindings::callback::{CallbackContainer, RootedCallback};
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
    /// Per-parameter arrays (k-rate length 1, a-rate length 128).
    #[ignore_malloc_size_of = "JS-heap payload, wrapper is a slot"]
    pub(crate) param_arrays:
        Vec<crate::dom::bindings::buffer_source::HeapBufferSource<Float32>>,
    /// The processor-side port (its `postMessage` is redirected through the
    /// node's conduit).
    pub(crate) port: Dom<MessagePort>,
}

impl ProcessorInstanceData {
    /// Copy the quantum's inputs and parameter timelines into the persistent
    /// arrays. No JS runs in here (typed-array view reads only), so the
    /// borrows are short by construction. Returns `false` when an array is
    /// missing/neutered (the caller latches the failure).
    #[expect(unsafe_code)]
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
        for (index, arr) in self.param_arrays.iter().enumerate() {
            let Some(quantum_param) = quantum.params.get(index) else {
                break;
            };
            let Ok(mut view) = arr.get_typed_array() else {
                return false;
            };
            let Some(slice) = view.as_mut_slice_safe(cx.no_gc()) else {
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

    /// Copy the processor's output arrays back into the quantum. Called after
    /// `process()` returned without throwing; a GC inside the call has been
    /// reflected into the traced slots already.
    pub(crate) fn read_outputs(&self, cx: &JSContext, quantum: &mut WorkletQuantum) {
        for (port, buffers) in self.output_arrays.iter().enumerate() {
            let Some(quantum_output) = quantum.outputs.get_mut(port) else {
                break;
            };
            for (channel, arr) in buffers.iter().enumerate() {
                if channel >= quantum_output.channels() as usize {
                    break;
                }
                let Ok(view) = arr.get_typed_array() else {
                    continue;
                };
                let Some(slice) = view.as_slice_safe(cx.no_gc()) else {
                    continue;
                };
                let len = slice.len().min(FRAMES_PER_BLOCK_USIZE);
                quantum_output.chan_mut(channel as u8)[..len].copy_from_slice(&slice[..len]);
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
    /// The block-rate pumps (untraced, thread-confined — see [`NodePump`]).
    #[no_trace]
    #[ignore_malloc_size_of = "media pump, no heap-owned GC payload"]
    audio_pumps: DomRefCell<Vec<NodePump>>,
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
            audio_pumps: DomRefCell::new(Vec::new()),
        });
        let origin = global.worklet_global.origin();
        AudioWorkletGlobalScopeBinding::Wrap::<crate::DomTypeHolder>(cx, &origin, global)
    }

    pub(crate) fn audio(&self) -> &AudioWorkletScopeData {
        &self.audio
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
        // Then inbound port payloads: pop under the borrow, dispatch outside
        // it (dispatch runs script).
        let mut payloads = Vec::new();
        {
            let pumps = self.audio_pumps.borrow();
            for entry in pumps.iter() {
                while let Some(payload) = entry.conduit.pop_for_processor() {
                    payloads.push((entry.node_key, payload));
                }
            }
        }
        if payloads.is_empty() {
            return;
        }
        let global = self.upcast::<crate::dom::globalscope::GlobalScope>();
        for (node_key, payload) in payloads {
            let Some(inst) = self.instance_data(node_key) else {
                continue;
            };
            crate::dom::audio::audioworkletport::dispatch_port_payload(
                cx,
                &inst.port,
                global,
                payload,
            );
        }
    }
}

impl AudioWorkletGlobalScopeMethods<crate::DomTypeHolder> for AudioWorkletGlobalScope {
    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-registerprocessor>
    #[expect(unsafe_code)]
    fn RegisterProcessor(
        &self,
        cx: &mut JSContext,
        name: DOMString,
        processor_ctor: RootedCallback<VoidFunction>,
    ) -> Fallible<()> {
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
        Finite::wrap(self.audio.sample_rate() as f64 * self.audio.current_time())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-currenttime>
    fn CurrentTime(&self) -> Finite<f64> {
        Finite::wrap(self.audio.current_time())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioworkletglobalscope-samplerate>
    fn SampleRate(&self) -> Finite<f32> {
        Finite::wrap(self.audio.sample_rate())
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
