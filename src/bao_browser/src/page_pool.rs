// @trace REQ-LIB-001 REQ-LIB-004 [entity:PagePool]
// @trace REQ-BRW-003: Multi-page pool with idle eviction
// @trace REQ-LIB-001: Headless multi-page management API
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::{CreateNewWebViewRequest, Servo};

use crate::config::{BaoConfig, PageConfig};
use crate::permission::Permission;
use crate::delegate::BaoServoDelegate;
use crate::error::BrowserError;
use crate::page::PageHandle;

pub struct PoolStats {
    pub active: usize,
    pub idle: usize,
    pub total_created: usize,
    pub total_destroyed: usize,
}

struct IdleEntry {
    page: PageHandle,
    idle_since: Instant,
}

pub struct PagePool {
    servo: Rc<Servo>,
    servo_delegate: Rc<BaoServoDelegate>,
    active_pages: RefCell<HashMap<usize, PageHandle>>,
    idle_pages: RefCell<HashMap<usize, IdleEntry>>,
    max_total: usize,
    idle_ttl: Duration,
    default_viewport: PhysicalSize<u32>,
    next_id: RefCell<usize>,
    total_created: RefCell<usize>,
    total_destroyed: RefCell<usize>,
    /// Popup page ids adopted by `create_popup_page` whose pipeline-ready wait
    /// + Node/stealth injection are still outstanding (REQ-LIB-001). Drained
    /// by `init_pending_pages` from the pump loops — never from inside the
    /// delegate callback, which runs within an in-flight `spin_event_loop`.
    pending_inits: RefCell<Vec<usize>>,
    /// Bounded retry budget for popup ids whose deferred injection hit a
    /// transient evaluate error (e94 D3 fourth seam): a popup adopted
    /// mid-navigation can sit in the window between its initial pipeline
    /// leaving the script thread's `documents` set and its replacement
    /// entering it, where an injection evaluate comes back
    /// `JavaScript("WebViewNotReady")`. That is a navigation-in-flight
    /// artifact, not a broken page; the id re-queues (each pump turn counts
    /// one retry) until the retry window expires.
    init_retries: RefCell<HashMap<usize, std::time::Instant>>,
    /// Handles retired by `retire_webview_page` (content-initiated
    /// window.close()) whose physical teardown is still outstanding
    /// (REQ-LIB-001 criterion ⑤). The pool map drop is the immediate
    /// accounting signal; the close itself must run pump-side — notify_closed
    /// fires inside servo's embedder dispatch while the closing page's own
    /// evaluate can hold `PageHandle.inner` borrowed, and `PageHandle::close`
    /// needs `borrow_mut` on that same cell.
    pending_closes: RefCell<Vec<PageHandle>>,
    /// This pool's own `Rc` handle, armed post-construction by the runtime
    /// (`BrowserRuntime` holds the only strong `Rc<PagePool>`). The creation
    /// entries hand a derived `Weak` to every page delegate so servo's
    /// embedder dispatch (`request_create_new` / `notify_closed`) can reach
    /// the pool from the pump thread. Unarmed (`Weak::new()`) = delegates
    /// deny popup opens / closes, logged — never a panic.
    self_weak: RefCell<std::rc::Weak<PagePool>>,
}

impl PagePool {
    pub fn new(servo: Rc<Servo>, servo_delegate: Rc<BaoServoDelegate>, config: &BaoConfig) -> Self {
        PagePool {
            servo,
            servo_delegate,
            active_pages: RefCell::new(HashMap::new()),
            idle_pages: RefCell::new(HashMap::new()),
            max_total: config.max_pages,
            idle_ttl: config.idle_ttl,
            default_viewport: PhysicalSize::new(
                config.default_viewport_width,
                config.default_viewport_height,
            ),
            next_id: RefCell::new(1),
            total_created: RefCell::new(0),
            total_destroyed: RefCell::new(0),
            pending_inits: RefCell::new(Vec::new()),
            init_retries: RefCell::new(HashMap::new()),
            pending_closes: RefCell::new(Vec::new()),
            self_weak: RefCell::new(std::rc::Weak::new()),
        }
    }

    /// Arm this pool's own `Rc` handle (REQ-LIB-001). Called ONCE by the
    /// runtime right after the pool's `Rc::new` — inside `PagePool::new` the
    /// `Rc` does not exist yet (chicken-and-egg).
    pub(crate) fn arm_self_weak(&self, weak: std::rc::Weak<PagePool>) {
        *self.self_weak.borrow_mut() = weak;
    }

    /// Derived `Weak` handed to page delegates. An unarmed pool yields a dead
    /// weak: delegates log and deny popup opens (JS null) — fail-closed with
    /// an honest signal, never a panic.
    fn pool_weak(&self) -> std::rc::Weak<PagePool> {
        self.self_weak.borrow().clone()
    }

    pub fn create_page(&self, config: &PageConfig) -> Result<PageHandle, BrowserError> {
        let total = self.active_pages.borrow().len() + self.idle_pages.borrow().len();
        if total >= self.max_total {
            return Err(BrowserError::Init(format!(
                "page limit exceeded: {total}/{}",
                self.max_total
            )));
        }

        // SM-EVOLUTION #28 (verdict consumed 2026-09-10, REQ-STL identity
        // consistency): arm the CREATION-TIME engine timezone policy before
        // ANY realm of this page exists — `forceUTC_` is a per-realm
        // creation option with NO post-creation setter, and this page's
        // Window realm is created during the pipeline setup inside
        // `PageHandle::new` + `wait_for_pipeline_ready` below, so arming any
        // later would silently miss it. Both sinks (servo DOM realms via
        // `set_force_utc_realms` + bao Node-semantics realms via
        // `set_node_force_utc`) are fed from the same
        // `StealthProfile::timezone` field. Stealth-free pages reset the
        // flags explicitly — a stealthed page earlier in the process must
        // not leak its policy onto a stealth-free page's realms
        // (process-global creation-time flags, last write wins; engine-level
        // sink granularity, same class as the canvas noise seed global).
        let force_utc = config
            .stealth_profile
            .as_ref()
            .map_or(false, |p| p.timezone.force_utc);
        servo::set_force_utc_realms(force_utc);
        bao_engine::set_node_force_utc(force_utc);

        let id = {
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            id
        };

        let page = {
            // #40 phase breadcrumb: the webview build itself is async, but
            // keep the span named — a wedge here pins it precisely.
            crate::phase_watch::enter_phase(
                crate::phase_watch::phase::CREATE_WEBVIEW_NEW,
                id as u64,
            );
            let pool_weak = self.pool_weak();
            let p = PageHandle::new(
                &pool_weak,
                Rc::clone(&self.servo),
                Rc::clone(&self.servo_delegate),
                config,
                self.default_viewport,
                id,
            );
            crate::phase_watch::enter_phase(
                crate::phase_watch::phase::CREATE_WAIT_READY,
                id as u64,
            );
            p?
        };

        // Eager Node Realm init — REQ-SEC-002: eliminate lazy init path
        page.wait_for_pipeline_ready(Duration::from_secs(10))?;
        // SINGLE injection point for the whole crate (e36 BCE): engine/Web
        // APIs + stealth props + the servo-native Worker-scope callback
        // (page-script `new Worker()` stealth inheritance), exactly ONCE per
        // page. BrowserRuntime::create_page used to run a SECOND
        // inject_all_with_profile on the already-injected page; the second
        // install_webgl_override stored the first pass's JS hook into
        // __originalGetParameter__, so every un-intercepted getParameter
        // looped JS hook ↔ native override forever and surfaced as literal
        // `undefined` (e36 evidence:
        // .claude/prompts/brw004-getparameter-evidence.md).
        crate::phase_watch::enter_phase(crate::phase_watch::phase::CREATE_INJECT, id as u64);
        crate::runtime_bridge::inject_all_with_profile(&page, &config.stealth_profile)?;

        // ISSUE #20: config → runtime enforcement wiring. The runtime-side
        // enforcement points (fs read/write, net, run in bao_runtime) consult
        // the permission_bridge thread-local guard; without this install the
        // configured permission never reached them (audit finding: the guard
        // was always None — every check_* site silently allowed). The
        // script-thread callback drain (handle_evaluate_javascript, ahead of
        // any user evaluate) installs it on the owning thread.
        //
        // Scope note (DoD-D): the guard is thread-local per ScriptThread and
        // install is last-page-wins — per-page scoping on a shared thread is
        // recorded as a known limitation, not silently claimed.
        if let Some(permission) = &config.permission {
            let check = bun_runtime::permission_bridge::PermissionCheck {
                read_paths: permission.read.clone(),
                write_paths: permission.write.clone(),
                net_hosts: permission.net.clone(),
                env_allowed: permission.env.unwrap_or(true),
                run_allowed: permission.run.unwrap_or(true),
            };
            let webview_id = page
                .webview_id()
                .expect("created page must have a webview id");
            servo::register_script_thread_callback(
                webview_id,
                Box::new(move |_, _| {
                    bun_runtime::permission_bridge::set_permission(Some(check));
                }),
            );
        }

        // PER-WORKER delivery tier (REQ-BRW-004, user ruling 2026-09-09
        // vendor patch — e43 multi-worker gap): the worker-scope callback
        // registered just above (inside inject_all_with_profile) is
        // consume-once, so the FIRST Worker of this page drained it and every
        // 2nd+ page-JS `new Worker()` ran with ZERO stealth injection (engine
        // getters + JS hooks all absent — a bare fingerprintable Worker).
        // These injectors are NON-consuming: EVERY Dedicated Worker this page
        // creates receives both phases (scope init at the first drain point +
        // post-interfaces JS-hook install at the second).
        //
        // Second-drain note (C15 history): the OLD page-init one-shot
        // registration for the second drain point (interfaces-ready callback)
        // is RETIRED by this tier — with both registered, Worker #1 ran the
        // JS hooks blob TWICE at the second point (one-shot + injector), and
        // the audio getChannelData wrapper has no property-slot idempotency
        // guard (unlike getParameter's e36 __originalGetParameter__ gate), so
        // the double wrap applied the deterministic noise twice and broke
        // cross-realm digest equality (c15_worker_window_cross_realm_noise_
        // consistency). The injector alone gives EVERY worker exactly one
        // blob run; SW never drains the interfaces-ready queue (its own path
        // only drains the scope registry), so nothing else consumed the
        // retired one-shot.
        //
        // The FIRST-point double run (scope one-shot + scope injector, both
        // before interfaces exist) stays safe: the JS blob's typeof guards
        // skip everything pre-interfaces and the engine layer is idempotent
        // (define_permanent_getter "prior install" arm / e36 gate) — verified
        // by worker_multi_injection_tests (ua exact-match + permgetter=1 +
        // orignative=1). The scope one-shot itself is kept: a page's
        // ServiceWorker consumes it (S1 f77faf8b), and that drain is
        // S-family domain — untouched here.
        //
        // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK per-Worker
        // @trace REQ-BRW-004 [criterion:15] worker JS-hook per-Worker delivery
        if let Some(webview_id) = page.webview_id() {
            crate::runtime_bridge::register_worker_scope_injector_native(
                webview_id,
                config.stealth_profile.clone(),
            );
            crate::register_worker_interfaces_ready_injector_native(
                webview_id,
                config.stealth_profile.clone(),
            );
        }

        self.active_pages.borrow_mut().insert(id, page.clone());
        *self.total_created.borrow_mut() += 1;
        // create_page fully returned — park the watchdog until the next phase.
        crate::phase_watch::enter_phase(crate::phase_watch::phase::IDLE, 0);

        Ok(page)
    }

    /// Adopt a `window.open()` auxiliary WebView into the pool (REQ-LIB-001).
    ///
    /// Called from the WebView delegate's `request_create_new` on the pump
    /// thread — the same thread that owns this pool's `Rc` domain. Building
    /// the WebView from `request.builder(..)` (inside
    /// `PageHandle::from_window_open`) answers the opener ScriptThread's
    /// blocked creation channel; returning `None` leaves the request
    /// unanswered, which the script side surfaces as a JS null — the same
    /// Chromium semantics as a popup-blocked / resource-exhausted open (the
    /// `max_total` gate deliberately maps to this, not to a panic).
    ///
    /// Accounting follows `create_page` semantics (active_pages +
    /// total_created). The pipeline-ready wait + the single Node/stealth
    /// injection entry that `create_page` runs inline are DEFERRED to
    /// `init_pending_pages`: this call happens inside an in-flight
    /// `spin_event_loop` (servo embedder-message dispatch) and re-entering
    /// the event loop from within the callback is a message-reordering
    /// hazard servoshell does not expose itself to (its request_create_new
    /// only builds and registers).
    ///
    /// R53-A stealth inheritance: `stealth_profile` is the OPENER's profile
    /// (delegate-side selection). The keyed wire-config / canvas-noise
    /// entries for the child WebViewId are written HERE (before the opener's
    /// ScriptThread can observe the popup or navigate it), and the deferred
    /// injection later installs the full stealth surface through the same
    /// single injection entry as every other page (e36 BCE discipline).
    /// `None` profile = explicit stealth-free keyed entries, so the popup
    /// does not inherit another page's process-global fallback either.
    pub(crate) fn create_popup_page(
        &self,
        request: CreateNewWebViewRequest,
        viewport: PhysicalSize<u32>,
        stealth_profile: Option<bao_stealth::StealthProfile>,
        permission: Option<Permission>,
    ) -> Option<PageHandle> {
        let total = self.active_pages.borrow().len() + self.idle_pages.borrow().len();
        if total >= self.max_total {
            return None;
        }

        // Creation-time realm policy (mirror create_page): the popup's Window
        // realm is created by the opener's ScriptThread as soon as the
        // builder response lands, so the forceUTC policy must be armed BEFORE
        // the build, and a stealth-free popup must reset an earlier stealthed
        // page's process-global flags.
        let force_utc = stealth_profile
            .as_ref()
            .map_or(false, |p| p.timezone.force_utc);
        servo::set_force_utc_realms(force_utc);
        bao_engine::set_node_force_utc(force_utc);

        let id = {
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            id
        };

        let page = {
            crate::phase_watch::enter_phase(
                crate::phase_watch::phase::CREATE_WEBVIEW_NEW,
                id as u64,
            );
            let pool_weak = self.pool_weak();
            let p = PageHandle::from_window_open(
                &pool_weak,
                Rc::clone(&self.servo),
                Rc::clone(&self.servo_delegate),
                request,
                viewport,
                stealth_profile.clone(),
                permission,
                id,
            );
            match p {
                Ok(page) => page,
                Err(e) => {
                    log::error!("[page_pool] window.open popup build failed: {e}");
                    return None;
                }
            }
        };

        // R53-A keyed registration for the child WebViewId — the early window
        // between webview creation and the deferred injection must not run on
        // the process-global fallback bucket (multi-profile runtimes would
        // present another page's wire/canvas fingerprint to any fetch this
        // window sees). `install_all_native` (deferred injection) rewrites
        // the same values — registry writes are idempotent.
        if let Some(webview_id) = page.webview_id() {
            match &stealth_profile {
                Some(profile) => {
                    crate::runtime_bridge::register_stealth_keyed_config_for_webview(
                        webview_id, profile,
                    );
                }
                None => {
                    servo::set_stealth_wire_config_for_webview(webview_id, None, None);
                    servo::set_canvas_noise_for_webview(webview_id, 0, 0.0);
                }
            }
        }

        self.active_pages.borrow_mut().insert(id, page.clone());
        *self.total_created.borrow_mut() += 1;
        self.pending_inits.borrow_mut().push(id);
        crate::phase_watch::enter_phase(crate::phase_watch::phase::IDLE, 0);
        Some(page)
    }

    /// Drive the deferred half of popup page creation (REQ-LIB-001):
    /// pipeline-ready wait + the single Node/stealth injection entry +
    /// per-Worker injector tier + permission-bridge registration for every
    /// popup adopted by `create_popup_page` since the last drain — the exact
    /// `create_page` tail, run pump-side between spins (never re-entrant with
    /// the event loop). Called by the pump loops; a no-op with nothing
    /// pending. Returns the number of popups fully initialized.
    pub fn init_pending_pages(&self) -> usize {
        let ids: Vec<usize> = std::mem::take(&mut *self.pending_inits.borrow_mut());
        let mut initialized = 0;
        for id in ids {
            let Some(page) = self.active_pages.borrow().get(&id).cloned() else {
                // Popup closed (window.close / pool close) before its init
                // turn — nothing left to initialize.
                continue;
            };
            crate::phase_watch::enter_phase(
                crate::phase_watch::phase::CREATE_WAIT_READY,
                id as u64,
            );
            // Popups carry no builder URL (the initial about:blank load is
            // spawned by the opener's ScriptThread), so nav_seq == 0 — this
            // keeps the first-frame contract, exactly like initial pages.
            if let Err(e) = page.wait_for_pipeline_ready(Duration::from_secs(10)) {
                // Same transient-evaluate class as the injection branch below
                // (e94 D3 fourth seam): the frame-ready drain's own evaluate
                // can hit `WebViewNotReady` while the js: navigation's
                // pipeline swap is in flight. Re-queue within the retry
                // window; anything else — or an expired window — stays
                // fail-closed.
                const WAIT_RETRY_WINDOW: std::time::Duration =
                    std::time::Duration::from_secs(5);
                let transient = matches!(&e, BrowserError::JavaScript(msg) if msg == "WebViewNotReady");
                let first_seen = *self
                    .init_retries
                    .borrow_mut()
                    .entry(id)
                    .or_insert_with(std::time::Instant::now);
                if transient && first_seen.elapsed() < WAIT_RETRY_WINDOW {
                    self.pending_inits.borrow_mut().push(id);
                    continue;
                }
                let _ = self.init_retries.borrow_mut().remove(&id);
                // Fail-closed: a popup whose pipeline never came up must not
                // sit in the pool as a fake-alive page.
                log::error!("[page_pool] popup page {id} pipeline init failed: {e} — closing");
                let _ = self.close_page_inner(id);
                continue;
            }
            crate::phase_watch::enter_phase(crate::phase_watch::phase::CREATE_INJECT, id as u64);
            let profile = page.stealth_profile();
            if let Err(e) = crate::runtime_bridge::inject_all_with_profile(&page, &profile) {
                // Transient-evaluate retry (e94 D3 fourth seam): a popup
                // adopted mid-navigation can sit in the window between its
                // initial about:blank pipeline leaving the script thread's
                // `documents` set and its replacement pipeline entering it —
                // an injection evaluate in that window comes back
                // `JavaScript("WebViewNotReady")`. That is a timing artifact
                // of the navigation in flight, not a broken page: closing on
                // it murdered freshly opened popups ~11ms after window.open
                // (the js_popup_load_tests intermittent red). Re-queue for a
                // later pump turn (bounded); anything else — or an exhausted
                // budget — stays fail-closed.
                // The retry window is wall-clock (not a retry count): pump
                // cadence varies (test pumps, page-load stalls), so a count
                // would translate to an unpredictable real-world window.
                const INJECT_RETRY_WINDOW: std::time::Duration =
                    std::time::Duration::from_secs(5);
                let transient = matches!(&e, BrowserError::JavaScript(msg) if msg == "WebViewNotReady");
                let first_seen = *self
                    .init_retries
                    .borrow_mut()
                    .entry(id)
                    .or_insert_with(std::time::Instant::now);
                if transient && first_seen.elapsed() < INJECT_RETRY_WINDOW {
                    self.pending_inits.borrow_mut().push(id);
                    continue;
                }
                let _ = self.init_retries.borrow_mut().remove(&id);
                log::error!("[page_pool] popup page {id} injection failed: {e} — closing");
                let _ = self.close_page_inner(id);
                continue;
            }
            // Per-Worker injector tier (REQ-BRW-004, non-consuming) — mirrors
            // the create_page tail so page-JS `new Worker()` from the popup is
            // not a bare, fingerprintable Worker.
            if let Some(webview_id) = page.webview_id() {
                crate::runtime_bridge::register_worker_scope_injector_native(
                    webview_id,
                    profile.clone(),
                );
                crate::register_worker_interfaces_ready_injector_native(
                    webview_id,
                    profile.clone(),
                );
                // Runtime-side permission enforcement bridge (ISSUE #20) —
                // inherited from the opener (delegate-side selection).
                if let Some(permission) = page.permission().config().cloned() {
                    let check = bun_runtime::permission_bridge::PermissionCheck {
                        read_paths: permission.read,
                        write_paths: permission.write,
                        net_hosts: permission.net,
                        env_allowed: permission.env.unwrap_or(true),
                        run_allowed: permission.run.unwrap_or(true),
                    };
                    servo::register_script_thread_callback(
                        webview_id,
                        Box::new(move |_, _| {
                            bun_runtime::permission_bridge::set_permission(Some(check));
                        }),
                    );
                }
            }
            initialized += 1;
            crate::phase_watch::enter_phase(crate::phase_watch::phase::IDLE, 0);
        }
        initialized
    }

    /// Retire a page from the pool's maps in response to content-initiated
    /// close (`window.close()` → delegate `notify_closed`, REQ-LIB-001
    /// criterion ⑤). Accounting drops HERE — a pool observer sees the page
    /// gone the moment servo confirmed the close — while the physical
    /// teardown (worker joins, keyed-registry cleanup, WebView drop) is
    /// queued for the pump-side drain (`close_pending_pages`): this runs
    /// inside servo's embedder dispatch where the closing page's own
    /// evaluate legitimately holds `PageHandle.inner` borrowed, which
    /// `PageHandle::close` needs mutably. Idempotent on double-retire.
    pub(crate) fn retire_webview_page(&self, id: usize) -> bool {
        let retired = self
            .active_pages
            .borrow_mut()
            .remove(&id)
            .or_else(|| self.idle_pages.borrow_mut().remove(&id).map(|e| e.page));
        match retired {
            Some(page) => {
                self.pending_closes.borrow_mut().push(page);
                true
            }
            None => false,
        }
    }

    /// Physical teardown for every page retired by `retire_webview_page`
    /// since the last drain. Runs pump-side (never re-entrant with the event
    /// loop); returns the number of pages closed.
    pub fn close_pending_pages(&self) -> usize {
        let pending: Vec<PageHandle> = std::mem::take(&mut *self.pending_closes.borrow_mut());
        let mut closed = 0;
        for page in pending {
            let id = page.id();
            let result = page.close();
            // Counted as destroyed regardless of close() errors: the pool no
            // longer holds the page and its WebView drops with the handle —
            // reporting it alive would be the fake-alive failure mode.
            *self.total_destroyed.borrow_mut() += 1;
            closed += 1;
            if let Err(e) = result {
                log::error!("[page_pool] deferred close of page {id} errored: {e}");
            }
        }
        closed
    }

    pub fn get_page(&self, id: usize) -> Option<PageHandle> {
        if let Some(page) = self.active_pages.borrow().get(&id) {
            return Some(page.clone());
        }
        if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
            // G1 (W16 T6): leaving the idle map is the SPEC
            // `Idle --handle_reacquired--> Interactive` transition. Same
            // thread, synchronous with the map move — no observation window.
            // A mid-load-released page (stored Navigating) never entered
            // Idle and is left alone.
            entry.page.lifecycle_handle_reacquired();
            self.active_pages
                .borrow_mut()
                .insert(id, entry.page.clone());
            return Some(entry.page);
        }
        None
    }

    pub fn close_page(&self, id: usize) -> Result<(), BrowserError> {
        crate::phase_watch::enter_phase(crate::phase_watch::phase::CLOSE, id as u64);
        let result = self.close_page_inner(id);
        crate::phase_watch::enter_phase(crate::phase_watch::phase::IDLE, 0);
        result
    }

    fn close_page_inner(&self, id: usize) -> Result<(), BrowserError> {
        let _ = self.init_retries.borrow_mut().remove(&id);
        if let Some(page) = self.active_pages.borrow_mut().remove(&id) {
            page.close()?;
            *self.total_destroyed.borrow_mut() += 1;
            return Ok(());
        }
        if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
            entry.page.close()?;
            *self.total_destroyed.borrow_mut() += 1;
            return Ok(());
        }
        Err(BrowserError::Init(format!("page {id} not found")))
    }

    pub fn release_page(&self, id: usize) {
        if let Some(page) = self.active_pages.borrow_mut().remove(&id) {
            // G1 (W16 T5): entering the idle map materializes the SPEC
            // `Interactive --handle_dropped--> Idle` transition for pages
            // observably Interactive (the pool-level idle model and the
            // page-level PageState stay in lockstep). Mid-load pages keep
            // Navigating (no pseudo-Idle); their TTL reclaim later enters
            // Closing via close_during_load.
            page.lifecycle_handle_dropped();
            self.idle_pages.borrow_mut().insert(
                id,
                IdleEntry {
                    page,
                    idle_since: Instant::now(),
                },
            );
        }
    }

    pub fn check_idle_pages(&self) -> usize {
        let mut reclaimed = 0;
        let expired: Vec<usize> = self
            .idle_pages
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.idle_since.elapsed() > self.idle_ttl)
            .map(|(id, _)| *id)
            .collect();

        for id in expired {
            if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
                let _ = entry.page.close();
                *self.total_destroyed.borrow_mut() += 1;
                reclaimed += 1;
            }
        }

        reclaimed
    }

    /// Drain every page's servo repaint latch and run the pending composites.
    ///
    /// This is the headless redraw leg of servo's embedder contract: servo
    /// latches "a new frame is ready" per webview (`notify_new_frame_ready`),
    /// the embedder composites on its event loop (servoshell does it on winit's
    /// `RedrawRequested`). `Painter::render` is the render pipeline's heartbeat
    /// — refresh-driver ticks (rAF/animation) and screenshot capture only
    /// advance inside it — so skipping the composite stalls the whole pipeline
    /// after the per-webview boot kick (the boot-once-frame root cause,
    /// BCE-20260910-003 premise correction). Called from the steady-state pump
    /// loops (`run` / `pump_cdp` / `run_with_bridge`).
    ///
    /// @trace REQ-BRW-002 [entity:PageHandle] [entity:PagePool]
    pub fn paint_pages_needing_repaint(&self) {
        // Fast path (the pump loops call this every iteration): no page has a
        // latch → zero allocation, zero servo traffic.
        let any_pending = {
            let active = self.active_pages.borrow();
            let idle = self.idle_pages.borrow();
            active
                .values()
                .chain(idle.values().map(|entry| &entry.page))
                .any(|page| page.repaint_pending())
        };
        if !any_pending {
            return;
        }
        let handles: Vec<PageHandle> = {
            let active = self.active_pages.borrow();
            let idle = self.idle_pages.borrow();
            active
                .values()
                .cloned()
                .chain(idle.values().map(|entry| entry.page.clone()))
                .collect()
        };
        for page in handles {
            page.paint_if_needed();
        }
    }

    /// Ids of every live page (active + idle). Page ids are monotonic —
    /// closed pages free their slot but never their id — so callers MUST NOT
    /// reconstruct the live set from `stats()` counts (the webdriver host
    /// enumerator regressed exactly this way: after the first window close
    /// the 1..=count range silently missed live pages with higher ids).
    pub fn live_page_ids(&self) -> Vec<usize> {
        let active = self.active_pages.borrow().keys().copied().collect::<Vec<_>>();
        let idle = self.idle_pages.borrow().keys().copied().collect::<Vec<_>>();
        active.into_iter().chain(idle).collect()
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            active: self.active_pages.borrow().len(),
            idle: self.idle_pages.borrow().len(),
            total_created: *self.total_created.borrow(),
            total_destroyed: *self.total_destroyed.borrow(),
        }
    }

    pub fn close_all(&self) {
        for (_, page) in self.active_pages.borrow_mut().drain() {
            let _ = page.close();
            *self.total_destroyed.borrow_mut() += 1;
        }
        for (_, entry) in self.idle_pages.borrow_mut().drain() {
            let _ = entry.page.close();
            *self.total_destroyed.borrow_mut() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_stats_construction() {
        let stats = PoolStats {
            active: 3,
            idle: 2,
            total_created: 10,
            total_destroyed: 5,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 2);
        assert_eq!(stats.total_created, 10);
        assert_eq!(stats.total_destroyed, 5);
    }

    #[test]
    fn pool_stats_zero() {
        let stats = PoolStats {
            active: 0,
            idle: 0,
            total_created: 0,
            total_destroyed: 0,
        };
        assert_eq!(stats.active + stats.idle, 0);
    }

    #[test]
    fn pool_stats_invariant() {
        // total_created >= total_destroyed (can't destroy more than created)
        let stats = PoolStats {
            active: 5,
            idle: 3,
            total_created: 20,
            total_destroyed: 12,
        };
        assert!(stats.total_created >= stats.total_destroyed);
        assert_eq!(
            stats.active + stats.idle,
            stats.total_created - stats.total_destroyed
        );
    }

    #[test]
    fn pool_stats_all_active() {
        let stats = PoolStats {
            active: 8,
            idle: 0,
            total_created: 8,
            total_destroyed: 0,
        };
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.active, stats.total_created);
    }

    #[test]
    fn pool_stats_all_idle() {
        let stats = PoolStats {
            active: 0,
            idle: 4,
            total_created: 4,
            total_destroyed: 0,
        };
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, stats.total_created);
    }

    // ─── PoolStats ─────────────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn test_pool_stats_fields() {
        let stats = crate::page_pool::PoolStats {
            active: 3,
            idle: 1,
            total_created: 5,
            total_destroyed: 2,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_created, 5);
        assert_eq!(stats.total_destroyed, 2);
    }
}
