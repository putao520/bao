/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletGlobalScope
//
// (Bao 段(1), user ruling 2026-10-05): upstream servo has zero AudioWorklet
// runtime (origin/main 2026-10-05 audit: only fetch-pipeline destination
// strings + WPT meta). Mirrors PaintWorkletGlobalScope's scope shape (the
// worklet engine is upstream-shared; zero new architecture). The real-time
// render bridge (process() at block rate, AudioParamMap, port messaging
// between the render thread and this scope) is 段(2), the servo-media
// contract.
//
// Thread placement: this scope lives on the worklet engine's own thread(s)
// (WorkletGlobalScopeType::Audio). It carries NO JS objects across threads:
// the audio face is an Arc<Mutex<servo-media AudioContext>> handle plus a
// copied sample_rate scalar — the only cross-thread shape allowed under the
// bao cross-thread JSObject rule.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::{Heap, IsConstructor, Value};
use js::rust::Handle;
use js::jsval::{JSVal, ObjectValue};
use script_bindings::cell::DomRefCell;
use script_bindings::interfaces::HasOrigin;
use servo_media::audio::context::AudioContext;
use servo_url::{MutableOrigin, ServoUrl};
use stylo_atoms::Atom;

use crate::dom::bindings::callback::{CallbackContainer, RootedCallback};
use crate::dom::bindings::codegen::Bindings::AudioWorkletGlobalScopeBinding::{
    self, AudioWorkletGlobalScopeMethods,
};
use crate::dom::bindings::codegen::Bindings::VoidFunctionBinding::VoidFunction;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::bindings::trace::HashMapTracedValues;
use servo_base::id::PipelineId;
use crate::dom::worklet::WorkletExecutor;
use crate::dom::workletglobalscope::{WorkletGlobalScope, WorkletGlobalScopeInit};

/// The per-`AudioContext` audio face carried into the AudioWorklet realm.
///
/// Plumbs the LIVE servo-media handle of the creating `BaseAudioContext`
/// through `WorkletGlobalScopeInit.audio` so `currentTime`/`currentFrame`/
/// `sampleRate` report real values on the worklet thread. The handle is an
/// `Arc<Mutex<..>>` (no JS objects) — the only cross-thread shape allowed
/// under the bao cross-thread JSObject rule; `sample_rate` is a copied
/// scalar.
#[derive(Clone, MallocSizeOf)]
pub(crate) struct AudioWorkletScopeData {
    /// Live handle to the creating context's servo-media audio graph.
    #[ignore_malloc_size_of = "servo_media"]
    audio_context: Arc<Mutex<AudioContext>>,
    /// The creating context's sample rate (copied scalar).
    sample_rate: f32,
}

impl AudioWorkletScopeData {
    pub(crate) fn new(audio_context: Arc<Mutex<AudioContext>>, sample_rate: f32) -> Self {
        AudioWorkletScopeData {
            audio_context,
            sample_rate,
        }
    }

    pub(crate) fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-baseaudiocontext-currenttime>
    pub(crate) fn current_time(&self) -> f64 {
        self.audio_context
            .lock()
            .expect("Locking the servo-media audio context")
            .current_time()
    }
}

/// The registered processor constructor (data plane). Parameter descriptors
/// and the per-render-block invocation face are the 段(2) servo-media
/// contract.
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
    /// the 段(2) render bridge when the first node for the name renders.
    processor_registry: DomRefCell<HashMapTracedValues<Atom, Box<ProcessorDefinition>>>,
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
        });
        let origin = global.worklet_global.origin();
        AudioWorkletGlobalScopeBinding::Wrap::<crate::DomTypeHolder>(cx, &origin, global)
    }

    pub(crate) fn audio(&self) -> &AudioWorkletScopeData {
        &self.audio
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

        // Store the constructor (data plane). The AudioWorkletProcessor
        // prototype-chain validation and the per-name instance lifecycle
        // are the 段(2) render-bridge face.
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

impl HasOrigin for AudioWorkletGlobalScope {
    fn origin(&self) -> MutableOrigin {
        self.upcast::<WorkletGlobalScope>().origin()
    }
}
