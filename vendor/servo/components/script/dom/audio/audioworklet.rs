/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorklet
//
// (Bao 段(1), user ruling 2026-10-05): upstream servo has zero AudioWorklet
// runtime. Mirrors the upstream Worklet object shape — the interface is
// `: Worklet`, so `addModule` is inherited from the Worklet prototype; the
// proto chain builds pref-free (codegen builds prototype chains inside
// GetProtoObject regardless of ConstructorEnabled), which keeps
// `audioContext.audioWorklet.addModule` legible while `window.Worklet` itself
// stays pref-gated (upstream parity preserved).
//
// 段(3) wiring: the object also owns the shared processor registry
// (name → parameter descriptors, published by `registerProcessor` on the
// worklet thread and read by `new AudioWorkletNode` on the script thread)
// and forwards the node's instantiation task to its worklet thread pool.

use std::rc::Rc;
use std::sync::Arc;

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::reflect_dom_object;

use crate::dom::audio::audioworklethandler::SharedProcessorRegistry;
use crate::dom::audio::audioworkletglobalscope::AudioWorkletScopeData;
use crate::dom::bindings::root::DomRoot;
use crate::dom::window::Window;
use crate::dom::worklet::{StatelessWorkletThreadPool, Worklet, WorkletTask};
use crate::dom::workletglobalscope::{WorkletGlobalScopeInit, WorkletGlobalScopeType};

#[dom_struct]
pub(crate) struct AudioWorklet {
    /// The inherited Worklet engine face (own thread pool, own WorkletId) —
    /// the webidl parent, held by value (the reflector chain anchor).
    worklet: Worklet,
    /// The shared processor registry (name → parameter descriptors). Created
    /// with the object; clones flow into each worklet scope's audio face, so
    /// `registerProcessor` (worklet threads) and `new AudioWorkletNode`
    /// (script thread) meet in the same map.
    #[no_trace]
    #[ignore_malloc_size_of = "parameter descriptors are small"]
    registry: Arc<SharedProcessorRegistry>,
}

impl AudioWorklet {
    /// Builds the AudioWorklet whose inherited Worklet engine face runs with
    /// `WorkletGlobalScopeType::Audio` and a thread-pool init extended with
    /// the creating context's audio face (servo-media handle + sample rate)
    /// so the AudioWorkletGlobalScope reports real audio values. Mirrors
    /// `Window::new_paint_worklet` + `TestWorklet::new`.
    pub(crate) fn new(
        cx: &mut JSContext,
        window: &Window,
        registry: Arc<SharedProcessorRegistry>,
        audio: AudioWorkletScopeData,
    ) -> DomRoot<AudioWorklet> {
        let mut worklet_global_scope_init = WorkletGlobalScopeInit::from(window);
        worklet_global_scope_init.audio = Some(audio);
        reflect_dom_object(
            cx,
            Box::new(AudioWorklet {
                worklet: Worklet::new_inherited(
                    window,
                    WorkletGlobalScopeType::Audio,
                    Box::new(move || {
                        Rc::new(StatelessWorkletThreadPool::spawn(
                            worklet_global_scope_init,
                        ))
                    }),
                ),
                registry,
            }),
            window,
        )
    }

    /// The shared processor registry (script-thread read face of
    /// `new AudioWorkletNode`).
    pub(crate) fn registry(&self) -> Arc<SharedProcessorRegistry> {
        self.registry.clone()
    }

    /// Forward a task to this worklet's thread pool (the instantiation task
    /// of `new AudioWorkletNode`).
    pub(crate) fn perform_a_worklet_task(&self, task: WorkletTask) {
        self.worklet.perform_a_worklet_task(task);
    }
}
