/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![cfg_attr(crown, allow(crown::jscontext_first_arg))]

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use devtools_traits::ScriptToDevtoolsControlMsg;
use dom_struct::dom_struct;
use embedder_traits::ScriptToEmbedderChan;
use js::context::JSContext;
use net_traits::ResourceThreads;
use net_traits::image_cache::ImageCache;
use profile_traits::{mem, time};
use script_bindings::cell::DomRefCell;
use script_traits::Painter;
use servo_base::generic_channel::GenericCallback;
use servo_base::id::{PipelineId, WebViewId};
use servo_constellation_traits::ScriptToConstellationSender;
use servo_url::{ImmutableOrigin, MutableOrigin, ServoUrl};
use storage_traits::StorageThreads;
use stylo_atoms::Atom;

use crate::dom::Window;
use crate::dom::audio::audioworkletglobalscope::{AudioWorkletGlobalScope, AudioWorkletScopeData};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::trace::{CustomTraceable, HashMapTracedValues};
use crate::dom::bindings::utils::define_all_exposed_interfaces;
use crate::dom::globalscope::GlobalScope;
use crate::dom::paintworkletglobalscope::PaintWorkletGlobalScope;
#[cfg(feature = "testbinding")]
use crate::dom::testworkletglobalscope::TestWorkletGlobalScope;
#[cfg(feature = "webgpu")]
use crate::dom::webgpu::identityhub::IdentityHub;
use crate::dom::worklet::WorkletExecutor;
use crate::messaging::MainThreadScriptMsg;
use crate::modules::script_module::{ModuleRequest, ModuleStatus};
use crate::realms::enter_auto_realm;
use crate::runtime::job_queue::job_queue_microtask_checkpoint;
use crate::tasks::task::TaskCanceller;
use crate::tasks::task_manager::TaskManager;

#[dom_struct]
/// <https://drafts.css-houdini.org/worklets/#workletglobalscope>
pub(crate) struct WorkletGlobalScope {
    /// The global for this worklet.
    globalscope: GlobalScope,
    /// The base URL for this worklet.
    #[no_trace]
    base_url: ServoUrl,
    /// Sender back to the script thread
    to_script_thread_sender: Sender<MainThreadScriptMsg>,
    /// Worklet task executor
    executor: WorkletExecutor,

    #[no_trace]
    /// The pipeline that created this worklet.
    pipeline_id: PipelineId,

    #[no_trace]
    origin: MutableOrigin,

    /// The owning page's webview identity (plumbed from the creating
    /// Window through `WorkletGlobalScopeInit`); None for worklets created
    /// without a Window.
    #[no_trace]
    webview_id: Option<WebViewId>,

    /// The [`TaskManager`] for this [`WorkletGlobalScope`].
    #[conditional_malloc_size_of]
    task_manager: Rc<TaskManager>,

    #[conditional_malloc_size_of]
    closing: Arc<AtomicBool>,

    /// module map is used when importing JavaScript modules
    /// <https://html.spec.whatwg.org/multipage/#concept-settings-object-module-map>
    #[ignore_malloc_size_of = "mozjs"]
    module_map: DomRefCell<HashMapTracedValues<ModuleRequest, ModuleStatus>>,
}

impl WorkletGlobalScope {
    /// Create a new heap-allocated `WorkletGlobalScope`.
    #[allow(clippy::too_many_arguments)]
    #[expect(unsafe_code)]
    pub(crate) fn new(
        scope_type: WorkletGlobalScopeType,
        pipeline_id: PipelineId,
        base_url: ServoUrl,
        inherited_secure_context: Option<bool>,
        executor: WorkletExecutor,
        init: &WorkletGlobalScopeInit,
        cx: &mut JSContext,
        closing: Arc<AtomicBool>,
    ) -> DomRoot<WorkletGlobalScope> {
        let scope: DomRoot<WorkletGlobalScope> = match scope_type {
            #[cfg(feature = "testbinding")]
            WorkletGlobalScopeType::Test => DomRoot::upcast(TestWorkletGlobalScope::new(
                pipeline_id,
                base_url,
                inherited_secure_context,
                executor,
                init,
                cx,
                closing,
            )),
            WorkletGlobalScopeType::Paint => DomRoot::upcast(PaintWorkletGlobalScope::new(
                cx,
                pipeline_id,
                base_url,
                inherited_secure_context,
                executor,
                init,
                closing,
            )),
            WorkletGlobalScopeType::Audio => DomRoot::upcast(AudioWorkletGlobalScope::new(
                cx,
                pipeline_id,
                base_url,
                inherited_secure_context,
                executor,
                init,
                closing,
            )),
        };

        let mut realm = enter_auto_realm(cx, &*scope);
        let mut realm = realm.current_realm();
        define_all_exposed_interfaces(&mut realm, scope.upcast());

        // BAO PATCH (REQ-BRW-004 4th injection realm, user ruling 2026-10-05,
        // AudioWorklet 段(1)): the AudioWorklet realm must not be a bare realm
        // (anti-fingerprint constitution A). Both REQ-BRW-004 injector layers
        // run here, AFTER `define_all_exposed_interfaces` — the worklet realm
        // has exactly one creation point, so the engine-layer getters and the
        // post-interfaces JS hooks land in the same drain (the embedder
        // install is idempotent — same property as the worker second drain).
        // Only the NON-consuming injector registries are drained: the
        // consume-once Worker queues keep their first-Worker semantics, and a
        // worklet realm must not steal a future Worker's one-shot install.
        // Paint/Test worklet realms keep the upstream bare-realm behavior
        // (untouched scope).
        if scope_type == WorkletGlobalScopeType::Audio {
            if let Some(webview_id) = init.webview_id {
                for injector in
                    crate::event_loop::script_thread::worker_scope_injectors(webview_id)
                {
                    unsafe {
                        injector(
                            realm.raw_cx_no_gc() as *mut std::ffi::c_void,
                            script_bindings::reflector::DomObject::reflector(
                                scope.upcast::<GlobalScope>(),
                            )
                            .get_jsobject()
                            .get() as *mut std::ffi::c_void,
                        );
                    }
                }
                for injector in
                    crate::event_loop::script_thread::worker_interfaces_ready_injectors(
                        webview_id,
                    )
                {
                    unsafe {
                        injector(
                            realm.raw_cx_no_gc() as *mut std::ffi::c_void,
                            script_bindings::reflector::DomObject::reflector(
                                scope.upcast::<GlobalScope>(),
                            )
                            .get_jsobject()
                            .get() as *mut std::ffi::c_void,
                        );
                    }
                }
            }
        }

        scope
    }

    /// Create a new stack-allocated `WorkletGlobalScope`.
    pub(crate) fn new_inherited(
        pipeline_id: PipelineId,
        base_url: ServoUrl,
        inherited_secure_context: Option<bool>,
        executor: WorkletExecutor,
        init: &WorkletGlobalScopeInit,
        closing: Arc<AtomicBool>,
    ) -> Self {
        let script_event_loop_sender = executor.event_loop_sender();

        Self {
            globalscope: GlobalScope::new_inherited(
                init.devtools_chan.clone(),
                init.mem_profiler_chan.clone(),
                init.time_profiler_chan.clone(),
                init.script_to_constellation_sender.clone(),
                init.to_embedder_sender.clone(),
                init.resource_threads.clone(),
                init.storage_threads.clone(),
                base_url.clone(),
                None,
                #[cfg(feature = "webgpu")]
                init.gpu_id_hub.clone(),
                inherited_secure_context,
                false,
            ),
            base_url,
            to_script_thread_sender: init.to_script_thread_sender.clone(),
            executor,
            pipeline_id,
            task_manager: Rc::new(TaskManager::new(
                Some(script_event_loop_sender),
                pipeline_id,
                Some(TaskCanceller {
                    cancelled: closing.clone(),
                }),
            )),
            origin: MutableOrigin::new(ImmutableOrigin::new_opaque()),
            webview_id: init.webview_id,
            closing,
            module_map: Default::default(),
        }
    }

    /// The owning page's webview identity, if this worklet was created from
    /// a Window (REQ-BRW-004 4th injection realm keying + module-fetch
    /// webview attribution).
    pub(crate) fn webview_id(&self) -> Option<WebViewId> {
        self.webview_id
    }

    pub(crate) fn module_map(
        &self,
    ) -> &DomRefCell<HashMapTracedValues<ModuleRequest, ModuleStatus>> {
        &self.module_map
    }

    pub(crate) fn origin(&self) -> MutableOrigin {
        self.origin.clone()
    }

    pub(crate) fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// Register a paint worklet to the script thread.
    pub(crate) fn register_paint_worklet(
        &self,
        name: Atom,
        properties: Vec<Atom>,
        painter: Box<dyn Painter>,
    ) {
        self.to_script_thread_sender
            .send(MainThreadScriptMsg::RegisterPaintWorklet {
                pipeline_id: self.globalscope.pipeline_id(),
                name,
                properties,
                painter,
            })
            .expect("Worklet thread outlived script thread.");
    }

    /// The base URL of this global.
    pub(crate) fn base_url(&self) -> ServoUrl {
        self.base_url.clone()
    }

    /// The worklet executor.
    pub(crate) fn executor(&self) -> WorkletExecutor {
        self.executor.clone()
    }

    pub(crate) fn task_manager(&self) -> Rc<TaskManager> {
        self.task_manager.clone()
    }

    pub(crate) fn perform_a_microtask_checkpoint(&self, cx: &mut JSContext) {
        if !self.closing.load(Ordering::SeqCst) {
            job_queue_microtask_checkpoint(cx, vec![DomRoot::from_ref(&self.globalscope)]);
        }
    }
}

impl From<&Window> for WorkletGlobalScopeInit {
    fn from(window: &Window) -> Self {
        let global_scope = window.as_global_scope();

        WorkletGlobalScopeInit {
            to_script_thread_sender: window.main_thread_script_chan().clone(),
            resource_threads: global_scope.resource_threads().clone(),
            storage_threads: global_scope.storage_threads().clone(),
            mem_profiler_chan: global_scope.mem_profiler_chan().clone(),
            time_profiler_chan: global_scope.time_profiler_chan().clone(),
            devtools_chan: global_scope.devtools_chan().cloned(),
            script_to_constellation_sender: global_scope.script_to_constellation_chan().sender,
            to_embedder_sender: global_scope.script_to_embedder_chan().clone(),
            image_cache: global_scope.image_cache(),
            #[cfg(feature = "webgpu")]
            gpu_id_hub: global_scope.wgpu_id_hub(),
            webview_id: Some(window.webview_id()),
            audio: None,
        }
    }
}

/// Resources required by workletglobalscopes
#[derive(Clone)]
pub(crate) struct WorkletGlobalScopeInit {
    /// Channel to the main script thread
    pub(crate) to_script_thread_sender: Sender<MainThreadScriptMsg>,
    /// Channel to a resource thread
    pub(crate) resource_threads: ResourceThreads,
    /// Channels to the [`StorageThreads`].
    pub(crate) storage_threads: StorageThreads,
    /// Channel to the memory profiler
    pub(crate) mem_profiler_chan: mem::ProfilerChan,
    /// Channel to the time profiler
    pub(crate) time_profiler_chan: time::ProfilerChan,
    /// Channel to devtools
    pub(crate) devtools_chan: Option<GenericCallback<ScriptToDevtoolsControlMsg>>,
    /// Messages to send to the Embedder
    pub(crate) to_embedder_sender: ScriptToEmbedderChan,
    /// The image cache
    pub(crate) image_cache: Arc<dyn ImageCache>,
    /// Identity manager for WebGPU resources
    #[cfg(feature = "webgpu")]
    pub(crate) gpu_id_hub: Arc<IdentityHub>,
    pub(crate) script_to_constellation_sender: ScriptToConstellationSender,
    /// The owning page's webview identity (None when no Window is known).
    pub(crate) webview_id: Option<WebViewId>,
    /// (Bao 段(1)) The audio face for `WorkletGlobalScopeType::Audio` scopes;
    /// set by the creating `BaseAudioContext`'s `audioWorklet` getter so the
    /// scope reads real servo-media values (suspended contexts report real
    /// zeros, not fabricated numbers).
    pub(crate) audio: Option<AudioWorkletScopeData>,
}

/// <https://drafts.css-houdini.org/worklets/#worklet-global-scope-type>
#[derive(Clone, Copy, Debug, JSTraceable, MallocSizeOf, PartialEq)]
pub(crate) enum WorkletGlobalScopeType {
    /// A servo-specific testing worklet
    #[cfg(feature = "testbinding")]
    Test,
    /// A paint worklet
    Paint,
    /// An audio worklet (Bao 段(1), user ruling 2026-10-05; upstream has
    /// zero AudioWorklet runtime — e83 profile).
    Audio,
}
