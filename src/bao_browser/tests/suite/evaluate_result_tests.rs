// @trace TEST-SEC-002 [req:REQ-SEC-002] [level:unit]
// Tests for EvaluateResult struct and evaluate_in_node_realm return value capture.
//
// REQ-SEC-002: evaluate_in_node_realm must capture return values as serialized
// strings via EvaluateResult, never silently discarding them.

use bao_browser::EvaluateResult;
use std::sync::{Arc, Mutex};

// ═══════════════════════════════════════════════════════════════════════
// EvaluateResult construction
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn evaluate_result_default_has_no_value_and_no_error() {
    let result = EvaluateResult::default();
    assert!(result.value.is_none(), "default value should be None");
    assert!(result.error.is_none(), "default error should be None");
    assert!(result.is_ok(), "default should be ok (no error)");
    assert!(!result.is_err(), "default should not be err");
}

#[test]
fn evaluate_result_ok_constructs_with_value() {
    let result = EvaluateResult::ok("42".into());
    assert_eq!(result.value, Some("42".to_string()));
    assert!(result.error.is_none());
    assert!(result.is_ok());
    assert!(!result.is_err());
}

#[test]
fn evaluate_result_err_constructs_with_error() {
    let result = EvaluateResult::err("something failed".into());
    assert!(result.value.is_none());
    assert_eq!(result.error, Some("something failed".to_string()));
    assert!(!result.is_ok());
    assert!(result.is_err());
}

#[test]
fn evaluate_result_ok_with_empty_string() {
    let result = EvaluateResult::ok(String::new());
    assert_eq!(result.value, Some(String::new()));
    assert!(result.is_ok());
}

#[test]
fn evaluate_result_err_with_empty_message() {
    let result = EvaluateResult::err(String::new());
    assert_eq!(result.error, Some(String::new()));
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════════
// EvaluateResult equality
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn evaluate_result_ok_equality() {
    let a = EvaluateResult::ok("hello".into());
    let b = EvaluateResult::ok("hello".into());
    assert_eq!(a, b);
}

#[test]
fn evaluate_result_ok_inequality_different_values() {
    let a = EvaluateResult::ok("hello".into());
    let b = EvaluateResult::ok("world".into());
    assert_ne!(a, b);
}

#[test]
fn evaluate_result_err_equality() {
    let a = EvaluateResult::err("error".into());
    let b = EvaluateResult::err("error".into());
    assert_eq!(a, b);
}

#[test]
fn evaluate_result_err_inequality_different_messages() {
    let a = EvaluateResult::err("error a".into());
    let b = EvaluateResult::err("error b".into());
    assert_ne!(a, b);
}

#[test]
fn evaluate_result_ok_and_err_are_not_equal() {
    let ok = EvaluateResult::ok("value".into());
    let err = EvaluateResult::err("value".into());
    assert_ne!(ok, err);
}

#[test]
fn evaluate_result_default_equality() {
    let a = EvaluateResult::default();
    let b = EvaluateResult::default();
    assert_eq!(a, b);
}

// ═══════════════════════════════════════════════════════════════════════
// EvaluateResult clone
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn evaluate_result_clone_ok() {
    let original = EvaluateResult::ok("value".into());
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn evaluate_result_clone_err() {
    let original = EvaluateResult::err("error message".into());
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn evaluate_result_clone_default() {
    let original = EvaluateResult::default();
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

// ═══════════════════════════════════════════════════════════════════════
// EvaluateResult debug
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn evaluate_result_debug_ok() {
    let result = EvaluateResult::ok("42".into());
    let debug = format!("{:?}", result);
    assert!(
        debug.contains("EvaluateResult"),
        "Debug should contain type name"
    );
    assert!(debug.contains("42"), "Debug should contain value");
}

#[test]
fn evaluate_result_debug_err() {
    let result = EvaluateResult::err("fail".into());
    let debug = format!("{:?}", result);
    assert!(
        debug.contains("EvaluateResult"),
        "Debug should contain type name"
    );
    assert!(debug.contains("fail"), "Debug should contain error");
}

// ═══════════════════════════════════════════════════════════════════════
// Arc<Mutex<EvaluateResult>> shared channel pattern (as used by evaluate_in_node_realm)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn shared_result_channel_writes_value() {
    let shared = Arc::new(Mutex::new(EvaluateResult::default()));
    {
        let mut guard = shared.lock().unwrap();
        guard.value = Some("result from node realm".into());
    }
    let guard = shared.lock().unwrap();
    assert_eq!(guard.value, Some("result from node realm".to_string()));
    assert!(guard.error.is_none());
}

#[test]
fn shared_result_channel_writes_error() {
    let shared = Arc::new(Mutex::new(EvaluateResult::default()));
    {
        let mut guard = shared.lock().unwrap();
        guard.error = Some("evaluation failed".into());
    }
    let guard = shared.lock().unwrap();
    assert!(guard.value.is_none());
    assert_eq!(guard.error, Some("evaluation failed".to_string()));
}

#[test]
fn shared_result_channel_cross_thread() {
    let shared = Arc::new(Mutex::new(EvaluateResult::default()));
    let shared_clone = Arc::clone(&shared);

    let handle = std::thread::spawn(move || {
        let mut guard = shared_clone.lock().unwrap();
        guard.value = Some("cross-thread value".into());
    });

    handle.join().unwrap();

    let guard = shared.lock().unwrap();
    assert_eq!(guard.value, Some("cross-thread value".to_string()));
    assert!(guard.error.is_none());
}

#[test]
fn shared_result_channel_overwrites_default_with_ok() {
    let shared = Arc::new(Mutex::new(EvaluateResult::default()));
    *shared.lock().unwrap() = EvaluateResult::ok("42".into());

    let guard = shared.lock().unwrap();
    assert!(guard.is_ok());
    assert_eq!(guard.value, Some("42".to_string()));
}

#[test]
fn shared_result_channel_overwrites_default_with_err() {
    let shared = Arc::new(Mutex::new(EvaluateResult::default()));
    *shared.lock().unwrap() = EvaluateResult::err("null pointer".into());

    let guard = shared.lock().unwrap();
    assert!(guard.is_err());
    assert_eq!(guard.error, Some("null pointer".to_string()));
}

// ═══════════════════════════════════════════════════════════════════════
// Structural verification — source code contains EvaluateResult usage
// ═══════════════════════════════════════════════════════════════════════

/// Verify EvaluateResult struct exists in runtime_bridge.rs (REQ-SEC-002).
#[test]
fn evaluate_result_struct_exists() {
    let source = include_str!("../../src/runtime_bridge.rs");
    assert!(
        source.contains("pub struct EvaluateResult"),
        "REQ-SEC-002 REGRESSION: EvaluateResult struct must exist in runtime_bridge.rs"
    );
}

/// Verify EvaluateResult has value and error fields (REQ-SEC-002).
#[test]
fn evaluate_result_has_value_and_error_fields() {
    let source = include_str!("../../src/runtime_bridge.rs");
    assert!(
        source.contains("pub value: Option<String>"),
        "REQ-SEC-002 REGRESSION: EvaluateResult must have pub value: Option<String>"
    );
    assert!(
        source.contains("pub error: Option<String>"),
        "REQ-SEC-002 REGRESSION: EvaluateResult must have pub error: Option<String>"
    );
}

/// Verify evaluate_in_node_realm returns EvaluateResult and the Node Realm
/// evaluate channel is a single-write OnceLock (REQ-SEC-002).
/// OnceLock is used instead of Mutex because the result is written exactly once
/// (single-producer, single-consumer), making OnceLock both safer and more efficient.
/// (ISSUE #24 wave: evaluate_in_node_realm now RETURNS the result — the
/// OnceLock write lives in the registering callback, which also layers the
/// execution-control timeout override on top.)
#[test]
fn evaluate_in_node_realm_accepts_result_channel() {
    let source = include_str!("../../src/runtime_bridge.rs");
    assert!(
        source.contains("pub unsafe fn evaluate_in_node_realm(")
            && source.contains(") -> EvaluateResult {"),
        "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must return EvaluateResult"
    );
    assert!(
        source.contains("let _ = result_clone.set(eval_result);"),
        "REQ-SEC-002 REGRESSION: the Node Realm evaluate callback must write the result via OnceLock::set"
    );
}

/// Verify evaluate_in_node_realm no longer discards the return value (REQ-SEC-002).
#[test]
fn evaluate_in_node_realm_does_not_discard_result() {
    let source = include_str!("../../src/runtime_bridge.rs");

    // Find the evaluate_in_node_realm function body
    let func_start = source
        .find("pub unsafe fn evaluate_in_node_realm")
        .expect("evaluate_in_node_realm function not found");
    let func_body_start = source[func_start..]
        .find('{')
        .expect("function body start not found");
    let search_limit = source[func_start + func_body_start..]
        .find("unsafe fn create_node_realm_native")
        .unwrap_or(2000)
        .min(4000);
    let func_body =
        &source[func_start + func_body_start..func_start + func_body_start + search_limit];

    // Verify there is no "let _result" or "let _ =" discarding the evaluate_script return value
    assert!(
        !func_body.contains("let _result"),
        "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must NOT use 'let _result' to discard return value"
    );
    assert!(
        func_body.contains("eval_result"),
        "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must capture evaluate_script return value"
    );
    assert!(
        func_body.contains("return EvaluateResult"),
        "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must return the EvaluateResult (never discard it)"
    );
}

/// Verify evaluate_in_node_realm handles null node_global with error (REQ-SEC-002).
#[test]
fn evaluate_in_node_realm_reports_null_node_global_error() {
    let source = include_str!("../../src/runtime_bridge.rs");
    let func_start = source
        .find("pub unsafe fn evaluate_in_node_realm")
        .expect("evaluate_in_node_realm function not found");
    let func_body_start = source[func_start..]
        .find('{')
        .expect("function body start not found");
    let search_limit = source[func_start + func_body_start..]
        .find("unsafe fn create_node_realm_native")
        .unwrap_or(2000)
        .min(4000);
    let func_body =
        &source[func_start + func_body_start..func_start + func_body_start + search_limit];

    assert!(
        func_body.contains("EvaluateResult::err"),
        "REQ-SEC-002 REGRESSION: evaluate_in_node_realm must use EvaluateResult::err for error reporting"
    );
    assert!(
        func_body.contains("node_global is null"),
        "REQ-SEC-002: evaluate_in_node_realm must report null node_global error"
    );
}

/// Verify evaluate_in_node_realm handles null JSContext with error (REQ-SEC-002).
#[test]
fn evaluate_in_node_realm_reports_null_context_error() {
    let source = include_str!("../../src/runtime_bridge.rs");
    let func_start = source
        .find("pub unsafe fn evaluate_in_node_realm")
        .expect("evaluate_in_node_realm function not found");
    let func_body_start = source[func_start..]
        .find('{')
        .expect("function body start not found");
    let search_limit = source[func_start + func_body_start..]
        .find("unsafe fn create_node_realm_native")
        .unwrap_or(2000)
        .min(4000);
    let func_body =
        &source[func_start + func_body_start..func_start + func_body_start + search_limit];

    assert!(
        func_body.contains("JSContext pointer is null"),
        "REQ-SEC-002: evaluate_in_node_realm must report null JSContext error"
    );
}

/// Verify evaluate_in_node_realm serializes different JS value types (REQ-SEC-002).
#[test]
fn evaluate_in_node_realm_serializes_value_types() {
    let source = include_str!("../../src/runtime_bridge.rs");
    let func_start = source
        .find("pub unsafe fn evaluate_in_node_realm")
        .expect("evaluate_in_node_realm function not found");
    let func_body_start = source[func_start..]
        .find('{')
        .expect("function body start not found");
    // task#7: the error branch grew (real pending-exception text extraction),
    // so the scan window tracks the function-end marker instead of a fixed
    // byte cap that truncated the value-type serialization section.
    let search_limit = source[func_start + func_body_start..]
        .find("/// Bridge callback: create Node Realm")
        .unwrap_or(3000);
    let func_body =
        &source[func_start + func_body_start..func_start + func_body_start + search_limit];

    // Must handle: string, number, boolean, null, undefined, object
    assert!(
        func_body.contains("is_string()"),
        "REQ-SEC-002: evaluate_in_node_realm must handle string values"
    );
    assert!(
        func_body.contains("is_number()"),
        "REQ-SEC-002: evaluate_in_node_realm must handle number values"
    );
    assert!(
        func_body.contains("is_boolean()"),
        "REQ-SEC-002: evaluate_in_node_realm must handle boolean values"
    );
    assert!(
        func_body.contains("is_null()"),
        "REQ-SEC-002: evaluate_in_node_realm must handle null values"
    );
    assert!(
        func_body.contains("is_undefined()"),
        "REQ-SEC-002: evaluate_in_node_realm must handle undefined values"
    );
}

/// Verify EvaluateResult is exported from bao_browser crate (REQ-SEC-002).
#[test]
fn evaluate_result_exported_from_crate() {
    let source = include_str!("../../src/lib.rs");
    assert!(
        source.contains("EvaluateResult"),
        "REQ-SEC-002 REGRESSION: EvaluateResult must be exported in lib.rs"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// task#7: evaluate error path carries the REAL pending exception text
// (previously the wrapper discarded it into a generic
// "evaluate_script returned Err (JS exception thrown)" string, leaving
// callers — CDP Runtime.evaluate / CLI / tests — with no actionable text).
// ═══════════════════════════════════════════════════════════════════════

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = pool_page.evaluate_js("");
        if matches!(
            pool_page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
// @trace REQ-SEC-002 [level:e2e]
fn evaluate_js_error_path_carries_real_exception_text() {
    // servo Opts is a per-process singleton — single #[test] for all asserts.
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = pool
        .create_page(&PageConfig {
            url: Some("data:text/html,<!DOCTYPE html><html><body></body></html>".into()),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20000);

    // ① Runtime throw: the error text must carry the actual marker message
    //    and the Error constructor name (not the generic wrapper string).
    let error = page
        .evaluate_js("throw new Error('marker-req7');")
        .expect_err("a thrown JS exception must surface as Err");
    let text = error.to_string();
    assert!(
        text.contains("marker-req7"),
        "evaluate error text must contain the real exception message, got: {text}"
    );
    assert!(
        text.contains("Error:"),
        "evaluate error text must contain the Error subclass name, got: {text}"
    );
    assert!(
        !text.contains("JS exception thrown"),
        "generic wrapper text must be gone, got: {text}"
    );

    // ② Compile failure: the pending SyntaxError must stay distinguishable
    //    from a plain runtime Error by name.
    let error = page
        .evaluate_js("const 1 = 2;")
        .expect_err("a syntax-error script must surface as Err");
    let text = error.to_string();
    assert!(
        text.contains("SyntaxError"),
        "syntax-error text must be distinguishable by name, got: {text}"
    );

    // ③ Normal path zero-regression: successful evaluation still flows.
    let value = page
        .evaluate_js("6 * 7")
        .expect("normal evaluation must succeed");
    assert_eq!(value, "42");

    // ④ Primitive throw: string payloads stringify (no generic wrapper text).
    let error = page
        .evaluate_js("throw 'primitive-marker';")
        .expect_err("a thrown string must surface as Err");
    let text = error.to_string();
    assert!(
        text.contains("primitive-marker"),
        "thrown primitive must reach the error text, got: {text}"
    );
}
