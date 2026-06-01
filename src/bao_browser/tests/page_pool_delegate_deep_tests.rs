// @trace TEST-BRW-POOL-DEEP-001 [req:REQ-LIB-001,REQ-BRW-001,REQ-BRW-003] [level:unit]
// Deep tests for bao_browser::page_pool and bao_browser::delegate.
//
// Servo's Opts (process-wide OnceLock) and PipelineNamespace (thread-local)
// mean: only ONE Servo per process, and WebViews must be created on the SAME
// thread as Servo.  Rust's test harness spawns each #[test] on its own thread,
// so we cannot use separate test functions for PagePool.  Instead, all PagePool
// assertions live in a single #[test] that creates Servo locally.
//
// BaoWebViewDelegate cannot be tested here — BaoWebViewState is not re-exported.

use std::rc::Rc;
use std::time::Duration;

use bao_browser::{BaoConfig, BaoServoDelegate, BrowserError, PageConfig, PagePool};
use servo::{ConsoleLogLevel, Servo, ServoBuilder, ServoDelegate, ServoError};

fn test_config(max_pages: usize) -> BaoConfig {
    BaoConfig {
        max_pages,
        idle_ttl: Duration::from_secs(300),
        ..BaoConfig::default()
    }
}

fn make_pool(servo: &Rc<Servo>, delegate: &Rc<BaoServoDelegate>, max_pages: usize) -> PagePool {
    PagePool::new(Rc::clone(servo), Rc::clone(delegate), &test_config(max_pages))
}

fn make_pool_with_config(
    servo: &Rc<Servo>,
    delegate: &Rc<BaoServoDelegate>,
    config: &BaoConfig,
) -> PagePool {
    PagePool::new(Rc::clone(servo), Rc::clone(delegate), config)
}

// ============================================================
// §A BaoServoDelegate — construction & defaults
// ============================================================

#[test]
fn servo_delegate_new_has_no_last_error() {
    let d = BaoServoDelegate::new();
    assert!(d.last_error().is_none());
}

#[test]
fn servo_delegate_default_matches_new() {
    let from_new = BaoServoDelegate::new();
    let from_default = BaoServoDelegate::default();
    assert!(from_new.last_error().is_none());
    assert!(from_default.last_error().is_none());
}

#[test]
fn servo_delegate_trait_object_dyn_compatible() {
    let d = BaoServoDelegate::new();
    let _: &dyn ServoDelegate = &d;
}

// ============================================================
// §B BaoServoDelegate — error notification
// ============================================================

#[test]
fn servo_delegate_notify_error_lost_connection() {
    let d = BaoServoDelegate::new();
    d.notify_error(ServoError::LostConnectionWithBackend);
    let err = d.last_error().expect("should have error");
    assert!(
        err.contains("LostConnectionWithBackend"),
        "error should contain variant name: {err}"
    );
}

#[test]
fn servo_delegate_notify_error_devtools_failed() {
    let d = BaoServoDelegate::new();
    d.notify_error(ServoError::DevtoolsFailedToStart);
    let err = d.last_error().expect("should have error");
    assert!(
        err.contains("DevtoolsFailedToStart"),
        "error should contain variant name: {err}"
    );
}

#[test]
fn servo_delegate_last_error_returns_independent_clones() {
    let d = BaoServoDelegate::new();
    d.notify_error(ServoError::LostConnectionWithBackend);
    let first = d.last_error();
    let second = d.last_error();
    assert_eq!(first, second);
    assert!(first.is_some());
    assert!(second.is_some());
}

#[test]
fn servo_delegate_notify_error_overwrites_previous() {
    let d = BaoServoDelegate::new();
    d.notify_error(ServoError::LostConnectionWithBackend);
    d.notify_error(ServoError::DevtoolsFailedToStart);
    let err = d.last_error().expect("should have error");
    assert!(err.contains("DevtoolsFailedToStart"));
    assert!(!err.contains("LostConnectionWithBackend"));
}

// ============================================================
// §C BaoServoDelegate — console messages & independence
// ============================================================

#[test]
fn servo_delegate_console_message_does_not_set_error() {
    let d = BaoServoDelegate::new();
    d.show_console_message(ConsoleLogLevel::Error, "test error".into());
    d.show_console_message(ConsoleLogLevel::Warn, "test warn".into());
    d.show_console_message(ConsoleLogLevel::Info, "test info".into());
    d.show_console_message(ConsoleLogLevel::Debug, "test debug".into());
    d.show_console_message(ConsoleLogLevel::Trace, "test trace".into());
    d.show_console_message(ConsoleLogLevel::Log, "test log".into());
    assert!(
        d.last_error().is_none(),
        "console messages must not set last_error"
    );
}

#[test]
fn servo_delegate_multiple_instances_independent() {
    let d1 = BaoServoDelegate::new();
    let d2 = BaoServoDelegate::new();
    d1.notify_error(ServoError::LostConnectionWithBackend);
    assert!(d1.last_error().is_some());
    assert!(d2.last_error().is_none());
}

// ============================================================
// §D PagePool — comprehensive (single Servo, single thread)
// ============================================================
// All PagePool assertions run in one test so that Servo's thread-local
// PipelineNamespace is available for WebView creation.

#[test]
fn page_pool_comprehensive() {
    let servo = Rc::new(ServoBuilder::default().build());
    let delegate = Rc::new(BaoServoDelegate::new());
    servo.set_delegate(Rc::clone(&delegate) as Rc<dyn ServoDelegate>);

    // ---- D.1 new() initial stats ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.1 active");
        assert_eq!(stats.idle, 0, "D.1 idle");
        assert_eq!(stats.total_created, 0, "D.1 created");
        assert_eq!(stats.total_destroyed, 0, "D.1 destroyed");
    }

    // ---- D.2 custom viewport config ----
    {
        let config = BaoConfig {
            max_pages: 5,
            default_viewport_width: 1280,
            default_viewport_height: 720,
            ..BaoConfig::default()
        };
        let pool = make_pool_with_config(&servo, &delegate, &config);
        let page = pool.create_page(&PageConfig::default()).expect("D.2 create");
        assert!(page.is_alive(), "D.2 alive");
        pool.close_all();
    }

    // ---- D.3 create_page increments stats ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.3 create");
        let stats = pool.stats();
        assert_eq!(stats.active, 1, "D.3 active");
        assert_eq!(stats.idle, 0, "D.3 idle");
        assert_eq!(stats.total_created, 1, "D.3 created");
        assert_eq!(stats.total_destroyed, 0, "D.3 destroyed");
        assert!(page.id() >= 1, "D.3 id >= 1");
        pool.close_all();
    }

    // ---- D.4 sequential IDs start at 1 ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let p1 = pool.create_page(&PageConfig::default()).expect("D.4 p1");
        let p2 = pool.create_page(&PageConfig::default()).expect("D.4 p2");
        let p3 = pool.create_page(&PageConfig::default()).expect("D.4 p3");
        assert_eq!(p1.id(), 1, "D.4 id1");
        assert_eq!(p2.id(), 2, "D.4 id2");
        assert_eq!(p3.id(), 3, "D.4 id3");
        assert_eq!(pool.stats().active, 3, "D.4 active");
        assert_eq!(pool.stats().total_created, 3, "D.4 created");
        pool.close_all();
    }

    // ---- D.5 close active page ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.5 create");
        let id = page.id();
        pool.close_page(id).expect("D.5 close");
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.5 active");
        assert_eq!(stats.idle, 0, "D.5 idle");
        assert_eq!(stats.total_created, 1, "D.5 created");
        assert_eq!(stats.total_destroyed, 1, "D.5 destroyed");
    }

    // ---- D.6 close idle page ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.6 create");
        let id = page.id();
        pool.release_page(id);
        assert_eq!(pool.stats().idle, 1, "D.6 idle before close");
        pool.close_page(id).expect("D.6 close idle");
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.6 active");
        assert_eq!(stats.idle, 0, "D.6 idle");
        assert_eq!(stats.total_destroyed, 1, "D.6 destroyed");
    }

    // ---- D.7 close nonexistent page ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        match pool.close_page(9999) {
            Err(BrowserError::Init(msg)) => {
                assert!(msg.contains("not found"), "D.7 msg: {msg}");
            }
            Err(other) => panic!("D.7 expected Init, got: {other:?}"),
            Ok(()) => panic!("D.7 expected error"),
        }
    }

    // ---- D.8 release moves active to idle ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.8 create");
        let id = page.id();
        pool.release_page(id);
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.8 active");
        assert_eq!(stats.idle, 1, "D.8 idle");
        assert_eq!(stats.total_created, 1, "D.8 created");
        assert_eq!(stats.total_destroyed, 0, "D.8 destroyed");
        pool.close_all();
    }

    // ---- D.9 get_page for active page ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.9 create");
        let id = page.id();
        let retrieved = pool.get_page(id).expect("D.9 get active");
        assert_eq!(retrieved.id(), id, "D.9 id match");
        assert_eq!(pool.stats().active, 1, "D.9 active");
        assert_eq!(pool.stats().idle, 0, "D.9 idle");
        pool.close_all();
    }

    // ---- D.10 get_page promotes idle to active ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.10 create");
        let id = page.id();
        pool.release_page(id);
        assert_eq!(pool.stats().idle, 1, "D.10 idle before get");
        assert_eq!(pool.stats().active, 0, "D.10 active before get");
        let retrieved = pool.get_page(id).expect("D.10 promote");
        assert_eq!(retrieved.id(), id, "D.10 id match");
        assert_eq!(pool.stats().active, 1, "D.10 active after get");
        assert_eq!(pool.stats().idle, 0, "D.10 idle after get");
        pool.close_all();
    }

    // ---- D.11 release nonexistent is silent ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        pool.release_page(9999);
        assert_eq!(pool.stats().active, 0, "D.11 active");
        assert_eq!(pool.stats().idle, 0, "D.11 idle");
    }

    // ---- D.12 get nonexistent returns None ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        assert!(pool.get_page(9999).is_none(), "D.12");
    }

    // ---- D.13 capacity limit exceeded ----
    {
        let pool = make_pool(&servo, &delegate, 2);
        let _p1 = pool.create_page(&PageConfig::default()).expect("D.13 p1");
        let _p2 = pool.create_page(&PageConfig::default()).expect("D.13 p2");
        match pool.create_page(&PageConfig::default()) {
            Err(BrowserError::Init(msg)) => {
                assert!(msg.contains("page limit exceeded"), "D.13 msg: {msg}");
            }
            Err(other) => panic!("D.13 expected Init, got: {other:?}"),
            Ok(_) => panic!("D.13 expected error"),
        }
        pool.close_all();
    }

    // ---- D.14 capacity includes idle pages ----
    {
        let pool = make_pool(&servo, &delegate, 2);
        let p1 = pool.create_page(&PageConfig::default()).expect("D.14 p1");
        let _p2 = pool.create_page(&PageConfig::default()).expect("D.14 p2");
        pool.release_page(p1.id());
        assert!(
            pool.create_page(&PageConfig::default()).is_err(),
            "D.14 idle should count toward capacity"
        );
        pool.close_all();
    }

    // ---- D.15 close frees capacity ----
    {
        let pool = make_pool(&servo, &delegate, 1);
        let page = pool.create_page(&PageConfig::default()).expect("D.15 p1");
        assert!(pool.create_page(&PageConfig::default()).is_err(), "D.15 full");
        pool.close_page(page.id()).expect("D.15 close");
        let _new = pool.create_page(&PageConfig::default()).expect("D.15 after close");
        pool.close_all();
    }

    // ---- D.16 release does NOT free capacity ----
    {
        let pool = make_pool(&servo, &delegate, 1);
        let page = pool.create_page(&PageConfig::default()).expect("D.16 p1");
        pool.release_page(page.id());
        assert!(
            pool.create_page(&PageConfig::default()).is_err(),
            "D.16 released page still occupies capacity"
        );
        pool.close_all();
    }

    // ---- D.17 close_all clears active and idle ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let p1 = pool.create_page(&PageConfig::default()).expect("D.17 p1");
        let _p2 = pool.create_page(&PageConfig::default()).expect("D.17 p2");
        let _p3 = pool.create_page(&PageConfig::default()).expect("D.17 p3");
        pool.release_page(p1.id());
        pool.close_all();
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.17 active");
        assert_eq!(stats.idle, 0, "D.17 idle");
        assert_eq!(stats.total_created, 3, "D.17 created");
        assert_eq!(stats.total_destroyed, 3, "D.17 destroyed");
    }

    // ---- D.18 close_all then create new ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let _p1 = pool.create_page(&PageConfig::default()).expect("D.18 p1");
        let _p2 = pool.create_page(&PageConfig::default()).expect("D.18 p2");
        pool.close_all();
        assert_eq!(pool.stats().total_destroyed, 2, "D.18 destroyed");
        let p3 = pool.create_page(&PageConfig::default()).expect("D.18 p3");
        assert_eq!(pool.stats().active, 1, "D.18 active");
        assert!(p3.id() > 2, "D.18 IDs continue incrementing");
        pool.close_all();
    }

    // ---- D.19 close_all on empty pool ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        pool.close_all();
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.19 active");
        assert_eq!(stats.idle, 0, "D.19 idle");
        assert_eq!(stats.total_destroyed, 0, "D.19 destroyed");
    }

    // ---- D.20 check_idle_pages with no expired ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.20 create");
        pool.release_page(page.id());
        let reclaimed = pool.check_idle_pages();
        assert_eq!(reclaimed, 0, "D.20 no expired");
        assert_eq!(pool.stats().idle, 1, "D.20 idle");
        pool.close_all();
    }

    // ---- D.21 stats after mixed operations ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let p1 = pool.create_page(&PageConfig::default()).expect("D.21 p1");
        let p2 = pool.create_page(&PageConfig::default()).expect("D.21 p2");
        let _p3 = pool.create_page(&PageConfig::default()).expect("D.21 p3");
        pool.release_page(p1.id());
        pool.close_page(p2.id()).expect("D.21 close p2");
        let stats = pool.stats();
        assert_eq!(stats.active, 1, "D.21 active (p3 only)");
        assert_eq!(stats.idle, 1, "D.21 idle (p1)");
        assert_eq!(stats.total_created, 3, "D.21 created");
        assert_eq!(stats.total_destroyed, 1, "D.21 destroyed (p2)");
        pool.close_all();
    }

    // ---- D.22 double release same page ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.22 create");
        let id = page.id();
        pool.release_page(id);
        pool.release_page(id);
        let stats = pool.stats();
        assert_eq!(stats.active, 0, "D.22 active");
        assert_eq!(stats.idle, 1, "D.22 idle");
        pool.close_all();
    }

    // ---- D.23 get_page twice from idle ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.23 create");
        let id = page.id();
        pool.release_page(id);
        let first = pool.get_page(id).expect("D.23 first get");
        assert_eq!(first.id(), id, "D.23 first id");
        let second = pool.get_page(id).expect("D.23 second get");
        assert_eq!(second.id(), id, "D.23 second id");
        assert_eq!(pool.stats().active, 1, "D.23 active");
        assert_eq!(pool.stats().idle, 0, "D.23 idle");
        pool.close_all();
    }

    // ---- D.24 close already-closed page returns error ----
    {
        let pool = make_pool(&servo, &delegate, 10);
        let page = pool.create_page(&PageConfig::default()).expect("D.24 create");
        let id = page.id();
        pool.close_page(id).expect("D.24 first close");
        assert!(
            pool.close_page(id).is_err(),
            "D.24 second close should error"
        );
    }
}
