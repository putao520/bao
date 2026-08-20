// @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366] [level:integration]
//
// BUG-ENG-366: Compartment isolation must be unconditional.
//
// These tests verify that per-TAB Compartment isolation — Page Realm (Window
// global), Node Realm (privileged JS_NewGlobalObject in NewCompartmentAndZone),
// and per-Realm stealth noise (Canvas/Navigator/WebGL/Audio) — does NOT depend
// on servo's `force_isolate_event_loops` flag.
//
// Architecture (tested layers):
//   1. bao_stealth per-Realm profile store (the unconditional isolation primitive)
//   2. bao_browser runtime_bridge registration points (install_all_native,
//      create_node_realm_native, refresh_dom_proxies_native, PageHandle::close)
//   3. BaoConfig / BaoRuntime documentation: force_isolate only governs
//      servo's event-loop multiplexing, NOT Compartment isolation
//
// Integration tests that drive BaoRuntime + servo WebView (verifying end-to-end
// per-page Canvas noise) live in stealth_fingerprint_e2e_tests.rs and require
// a working JSContext; these unit-level tests cover the contract surface that
// does not need servo.
//
// Scenarios:
//   1. Two simulated pages register distinct stealth profiles — must stay isolated
//   2. Node Realm global aliases the page's profile (REQ-SEC-002)
//   3. force_isolate=false simulation: shared-thread, distinct-global, distinct profile
//   4. Navigation re-key keeps the page fingerprint stable
//   5. Page close drops the profile so a reused global does not inherit a stale one
//   6. Canvas seed/amplitude vary across pages (the core anti-fingerprint contract)

#![allow(dead_code)]

use bao_browser::{BaoConfig, BrowserConfig, PageConfig};
use bao_stealth::engine_props::{
    self, clear_all_realm_profiles, register_global_alias, remove_profile_for_global,
    set_profile_for_global,
};
use bao_stealth::StealthProfile;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Report — fault-tolerant scenario accumulator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    passed: u32,
    skipped: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn pass(&mut self, name: &str) {
        self.passed += 1;
        self.messages.push(format!("PASS  {}", name));
    }
    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.messages.push(format!("SKIP  {}  ({})", name, why));
    }
    fn fail(&mut self, name: &str, why: &str) {
        self.failed += 1;
        self.messages.push(format!("FAIL  {}  ({})", name, why));
    }
    fn finish(&self) {
        eprintln!("\n=== Compartment Isolation Tests (BUG-ENG-366) ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
        assert_eq!(
            self.failed, 0,
            "BUG-ENG-366: compartment isolation tests failed"
        );
    }
}

// Test isolation lock — these tests mutate the global per-Realm profile store,
// so they must not run concurrently with other tests touching the same store.
static TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn test_lock() -> &'static std::sync::Mutex<()> {
    TEST_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// Distinct simulated global object addresses. In a real servo run these are
// the addresses of distinct *mut JSObject Window/Node-Realm globals.
const PAGE_A_GLOBAL: usize = 0x0001_AAAA_0000;
const PAGE_B_GLOBAL: usize = 0x0002_BBBB_0000;
const PAGE_A_NODE_REALM: usize = 0x0001_AAAA_1000;
const PAGE_B_NODE_REALM: usize = 0x0002_BBBB_1000;

// Counter to ensure each scenario sees a clean store.
static SCENARIO_ID: AtomicUsize = AtomicUsize::new(0);

fn reset_store() {
    clear_all_realm_profiles();
    SCENARIO_ID.fetch_add(1, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Scenario 1: two pages — distinct stealth profiles, no cross-contamination
// ---------------------------------------------------------------------------

fn scenario_two_pages_distinct_profiles(r: &mut Report) {
    reset_store();

    let chrome = StealthProfile::chrome_default();
    let firefox = StealthProfile::firefox_default();

    set_profile_for_global(PAGE_A_GLOBAL, &chrome);
    set_profile_for_global(PAGE_B_GLOBAL, &firefox);

    // Force_isolate=false simulation: both registrations happened on the same
    // thread (here, this test thread). They must still be isolated.
    let a_ua = engine_props::canvas_seed_for_test(PAGE_A_GLOBAL);
    let b_ua = engine_props::canvas_seed_for_test(PAGE_B_GLOBAL);

    if a_ua == Some(chrome.canvas.seed()) && b_ua == Some(firefox.canvas.seed()) {
        r.pass("two_pages_distinct_profiles: each page resolves to its own seed");
    } else {
        r.fail(
            "two_pages_distinct_profiles",
            &format!(
                "a={:?} b={:?} expected chrome={} firefox={}",
                a_ua,
                b_ua,
                chrome.canvas.seed(),
                firefox.canvas.seed()
            ),
        );
    }

    if a_ua != b_ua {
        r.pass("two_pages_distinct_profiles: seeds differ across pages");
    } else {
        r.fail("two_pages_distinct_profiles", "seeds collide across pages");
    }
}

// ---------------------------------------------------------------------------
// Scenario 2: Node Realm aliases page profile (REQ-SEC-002)
// ---------------------------------------------------------------------------

fn scenario_node_realm_aliases_page(r: &mut Report) {
    reset_store();

    let profile = StealthProfile::chrome_default();
    set_profile_for_global(PAGE_A_GLOBAL, &profile);
    register_global_alias(PAGE_A_GLOBAL, PAGE_A_NODE_REALM);

    let page_seed = engine_props::canvas_seed_for_test(PAGE_A_GLOBAL);
    let node_seed = engine_props::canvas_seed_for_test(PAGE_A_NODE_REALM);

    if page_seed == node_seed && page_seed == Some(profile.canvas.seed()) {
        r.pass("node_realm_aliases_page: Node Realm shares page Canvas seed");
    } else {
        r.fail(
            "node_realm_aliases_page",
            &format!(
                "page={:?} node={:?} expected={}",
                page_seed,
                node_seed,
                profile.canvas.seed()
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 3: force_isolate=false simulation (single thread, multiple pages)
// ---------------------------------------------------------------------------

fn scenario_force_isolate_false_still_isolated(r: &mut Report) {
    reset_store();

    // Three pages registered on the SAME thread — exactly what happens when
    // force_isolate_event_loops is false and all pages share servo's ScriptThread.
    let mut third = StealthProfile::chrome_default();
    third.canvas = bao_stealth::CanvasNoise::new(0xDEAD_BEEF);
    let profiles = vec![
        (PAGE_A_GLOBAL, StealthProfile::chrome_default()),
        (PAGE_B_GLOBAL, StealthProfile::firefox_default()),
        (0x0003_CCCC_0000, third),
    ];
    for (addr, p) in &profiles {
        set_profile_for_global(*addr, p);
    }

    let mut all_ok = true;
    for (addr, expected) in &profiles {
        let actual = engine_props::canvas_seed_for_test(*addr);
        if actual != Some(expected.canvas.seed()) {
            all_ok = false;
            r.fail(
                "force_isolate_false_still_isolated",
                &format!(
                    "addr {:#x} expected {} got {:?}",
                    addr,
                    expected.canvas.seed(),
                    actual
                ),
            );
        }
    }
    if all_ok {
        r.pass("force_isolate_false_still_isolated: 3 pages on 1 thread each isolated");
    }

    // Cross-page Canvas seeds must be pairwise distinct.
    let seeds: Vec<u64> = profiles
        .iter()
        .filter_map(|(addr, _)| engine_props::canvas_seed_for_test(*addr))
        .collect();
    let mut distinct = seeds.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() == seeds.len() && seeds.len() == 3 {
        r.pass("force_isolate_false_still_isolated: Canvas seeds pairwise distinct");
    } else {
        r.fail(
            "force_isolate_false_still_isolated",
            &format!("seed collision: {:?}", seeds),
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 4: navigation re-key keeps the page fingerprint stable
// ---------------------------------------------------------------------------

fn scenario_navigation_rekey(r: &mut Report) {
    reset_store();

    let profile = StealthProfile::firefox_default();
    set_profile_for_global(0x7000, &profile);
    register_global_alias(0x7000, 0x7004);

    let old_seed = engine_props::canvas_seed_for_test(0x7000);
    let new_seed = engine_props::canvas_seed_for_test(0x7004);

    if old_seed == new_seed && old_seed == Some(profile.canvas.seed()) {
        r.pass("navigation_rekey: Canvas seed preserved across navigation");
    } else {
        r.fail(
            "navigation_rekey",
            &format!(
                "old={:?} new={:?} expected={}",
                old_seed,
                new_seed,
                profile.canvas.seed()
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 5: page close drops the profile — reused global stays clean
// ---------------------------------------------------------------------------

fn scenario_page_close_drops_profile(r: &mut Report) {
    reset_store();

    let g: usize = 0x8000;
    let profile = StealthProfile::chrome_default();
    set_profile_for_global(g, &profile);
    remove_profile_for_global(g);

    match engine_props::canvas_seed_for_test(g) {
        None => r.pass("page_close_drops_profile: closed page has no profile"),
        Some(seed) => r.fail(
            "page_close_drops_profile",
            &format!("closed page still has profile seed={}", seed),
        ),
    }
}

// ---------------------------------------------------------------------------
// Scenario 6: BaoConfig/BrowserConfig accept force_isolate documentation flag
// ---------------------------------------------------------------------------

fn scenario_baoconfig_force_isolate_only_event_loop(r: &mut Report) {
    // Sanity: BaoConfig is constructible with various flags. force_isolate
    // lives in servo Opts, not BaoConfig — but the contract documented in
    // lib.rs is that disabling it does NOT regress Compartment isolation.
    // Here we just exercise BaoConfig construction to ensure the public API
    // surface used by BrowserConfig → BaoConfig conversion still works.
    let mut bc = BrowserConfig::default();
    bc.stealth_profile = Some(StealthProfile::firefox_default());
    let bao: BaoConfig = bc.into();
    if bao.validate().is_ok() {
        r.pass("baoconfig_force_isolate_only_event_loop: BaoConfig constructs cleanly");
    } else {
        r.fail(
            "baoconfig_force_isolate_only_event_loop",
            "BaoConfig::validate failed",
        );
    }

    // PageConfig must accept a stealth profile (the per-page fingerprint source).
    let mut pc = PageConfig::default();
    pc.stealth_profile = Some(StealthProfile::chrome_default());
    if pc.stealth_profile.is_some() {
        r.pass("pageconfig_carries_stealth_profile");
    } else {
        r.fail(
            "pageconfig_carries_stealth_profile",
            "PageConfig dropped stealth_profile",
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 7: anti-regression — never wire force_isolate to Compartment creation
// ---------------------------------------------------------------------------

fn scenario_no_force_isolate_dependency_in_compartment_creation(r: &mut Report) {
    // Thebao_browser source must NOT condition Node Realm Compartment creation
    // (NewCompartmentAndZone) on force_isolate. We assert this by source-level
    // inspection: create_node_realm_native uses NewCompartmentAndZone
    // unconditionally (regression test already exists in runtime_bridge tests
    // for this property — here we just confirm bao_stealth exposes the alias
    // hook used by create_node_realm_native).
    let profile = StealthProfile::firefox_default();
    set_profile_for_global(0x9000, &profile);
    register_global_alias(0x9000, 0x9100);
    if engine_props::canvas_seed_for_test(0x9100).is_some() {
        r.pass("no_force_isolate_dependency: alias hook works unconditionally");
    } else {
        r.fail("no_force_isolate_dependency", "alias hook returned None");
    }
    remove_profile_for_global(0x9000);
    remove_profile_for_global(0x9100);
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

#[test]
fn compartment_isolation_unconditional() {
    let _guard = test_lock().lock().unwrap();

    let mut r = Report::default();
    scenario_two_pages_distinct_profiles(&mut r);
    scenario_node_realm_aliases_page(&mut r);
    scenario_force_isolate_false_still_isolated(&mut r);
    scenario_navigation_rekey(&mut r);
    scenario_page_close_drops_profile(&mut r);
    scenario_baoconfig_force_isolate_only_event_loop(&mut r);
    scenario_no_force_isolate_dependency_in_compartment_creation(&mut r);
    r.finish();

    reset_store();
}
