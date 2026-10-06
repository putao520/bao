/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use js::rust::{JSEngine, JSEngineError, JSEngineHandle};

static JS_ENGINE: Mutex<Option<JSEngineHandle>> = Mutex::new(None);

pub(crate) fn current_js_engine_handle() -> JSEngineHandle {
    JS_ENGINE.lock().unwrap().as_ref().unwrap().clone()
}

pub struct JSEngineSetup(Option<JSEngine>);

impl Default for JSEngineSetup {
    fn default() -> Self {
        // BAO PATCH (BCE-20260627-009): Idempotent JSEngine init.
        // mozjs's `JSEngine::init()` uses a process-global `ENGINE_STATE` mutex
        // that returns `Err(AlreadyInitialized)` on any re-init. The original
        // servo code did `JSEngine::init().unwrap()`, which panics when a second
        // `BaoRuntime` (cargo multi-threaded test runner, or production
        // multi-tenant) creates a second `Servo` instance in the same process.
        // Each `Servo::new` spawns a `ScriptThread` -> `script::init()` -> this
        // `JSEngineSetup::default()`.
        //
        // Strategy: the FIRST caller initializes the engine and stores its handle
        // in `JS_ENGINE`. Subsequent callers reuse that handle without owning the
        // engine itself (return `JSEngineSetup(None)`). Only the owner (the first
        // `JSEngineSetup`) will `Drop` the real engine and shut it down. This
        // keeps the outstanding-handles refcount correct (no double-decrement)
        // and the engine alive until the owning ScriptThread is torn down.
        let engine = match JSEngine::init() {
            Ok(engine) => {
                *JS_ENGINE.lock().unwrap() = Some(engine.handle());
                Some(engine)
            }
            Err(JSEngineError::AlreadyInitialized) => {
                // Someone else (another ScriptThread / BaoRuntime / bao
                // ensure_engine_handle) already owns the engine. Prefer
                // mozjs::JSEngine::process_handle() (BAO PATCH SSOT), then
                // fall back to spinning on JS_ENGINE for legacy owners.
                let mut attempts = 0;
                loop {
                    if let Some(h) = JSEngine::process_handle() {
                        let mut slot = JS_ENGINE.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(h);
                        }
                        break;
                    }
                    if JS_ENGINE.lock().unwrap().is_some() {
                        break;
                    }
                    attempts += 1;
                    if attempts > 50 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                // Do NOT take ownership of the engine - the first owner keeps it.
                None
            }
            Err(JSEngineError::AlreadyShutDown) => {
                // BAO PATCH (BCE-20260627-009): Engine was previously
                // initialized AND shut down. We cannot recover the handle from
                // `JS_ENGINE` (it was cleared on the owner's Drop). Return
                // `None` and let the runtime proceed - the bao layer ensures
                // the first BaoRuntime's engine stays alive when needed.
                None
            }
            Err(e) => panic!("JSEngine::init() failed: {:?}", e),
        };
        Self(engine)
    }
}

impl Drop for JSEngineSetup {
    fn drop(&mut self) {
        // BAO PATCH (BCE-20260627-009): Do NOT clear JS_ENGINE and do NOT drop
        // the engine. The engine is a process-global singleton; its handle must
        // persist in JS_ENGINE across BaoRuntime teardown so subsequent
        // BaoRuntime instances reuse it.
        //
        // mozjs JSEngine is a process-global singleton with an irreversible
        // state machine (Uninitialized->Initialized->ShutDown). Once
        // `JS_ShutDown()` runs (catalyzed by `JSEngine::drop`), the same
        // process can never re-init. This breaks bao's multi-BaoRuntime model
        // (cargo test runner). Fix: leak the engine (`std::mem::forget`) AND
        // keep its handle in `JS_ENGINE` (do not clear it). The OS reclaims all
        // JS engine resources on process exit; behaviorally equivalent, with no
        // memory-safety regression, and correct for an embedded single-process
        // runtime that must tolerate repeated construction and teardown.
        let Some(engine) = self.0.take() else {
            return;
        };
        std::mem::forget(engine);
    }
}
