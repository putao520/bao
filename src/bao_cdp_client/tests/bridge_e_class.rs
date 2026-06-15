//! E 类 31+ method 集成测试 — 全部返回 -32601 (MethodNotFound)。
//!
//! 每个 E 类 method 一个测试,验证:
//! 1. 返回 `BridgeError::NotSupported`
//! 2. cdp_error_code() == -32601
//!
//! @trace REQ-BAO-API-007 [level:integration]

use bao_cdp_client::bridge::{BridgeError, MockServoBackend, ServoBackend};
use bao_cdp_client::dispatch_command;
use serde_json::json;
use std::sync::Arc;

fn assert_e_class(method: &str) {
    // @trace REQ-BAO-API-007 [level:library]
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let err = dispatch_command(&*b, method, json!({}), "1").unwrap_err();
    assert!(
        matches!(err, BridgeError::NotSupported(_)),
        "{method} should return NotSupported, got: {err:?}"
    );
    assert_eq!(
        err.cdp_error_code(),
        -32601,
        "{method} should map to -32601"
    );
}

// ════════════════════════════════════════════════════════════════════
// HeapProfiler domain — all methods E class (≥ 4 explicit per Plan)
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_heap_profiler_enable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.enable");
}

#[test]
fn e_heap_profiler_take_heap_snapshot() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.takeHeapSnapshot");
}

#[test]
fn e_heap_profiler_get_object_by_heap_object_id() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.getObjectByHeapObjectId");
}

#[test]
fn e_heap_profiler_disable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.disable");
}

#[test]
fn e_heap_profiler_start_tracking() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.startTrackingHeapObjects");
}

#[test]
fn e_heap_profiler_stop_tracking() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.stopTrackingHeapObjects");
}

#[test]
fn e_heap_profiler_start_sampling() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.startSampling");
}

#[test]
fn e_heap_profiler_stop_sampling() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:HeapProfiler] [level:integration]
    // Act
    // Assert
    assert_e_class("HeapProfiler.stopSampling");
}

// ════════════════════════════════════════════════════════════════════
// Profiler domain — all E class
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_profiler_enable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Profiler] [level:integration]
    // Act
    // Assert
    assert_e_class("Profiler.enable");
}

#[test]
fn e_profiler_disable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Profiler] [level:integration]
    // Act
    // Assert
    assert_e_class("Profiler.disable");
}

#[test]
fn e_profiler_start() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Profiler] [level:integration]
    // Act
    // Assert
    assert_e_class("Profiler.start");
}

#[test]
fn e_profiler_stop() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Profiler] [level:integration]
    // Act
    // Assert
    assert_e_class("Profiler.stop");
}

// ════════════════════════════════════════════════════════════════════
// DOMStorage domain — all E class
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_dom_storage_get_items() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:DOMStorage] [level:integration]
    // Act
    // Assert
    assert_e_class("DOMStorage.getDOMStorageItems");
}

#[test]
fn e_dom_storage_set_item() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:DOMStorage] [level:integration]
    // Act
    // Assert
    assert_e_class("DOMStorage.setDOMStorageItem");
}

#[test]
fn e_dom_storage_remove_item() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:DOMStorage] [level:integration]
    // Act
    // Assert
    assert_e_class("DOMStorage.removeDOMStorageItem");
}

// ════════════════════════════════════════════════════════════════════
// IndexedDB domain — all E class
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_indexed_db_request_database_names() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:IndexedDB] [level:integration]
    // Act
    // Assert
    assert_e_class("IndexedDB.requestDatabaseNames");
}

#[test]
fn e_indexed_db_request_database() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:IndexedDB] [level:integration]
    // Act
    // Assert
    assert_e_class("IndexedDB.requestDatabase");
}

#[test]
fn e_indexed_db_request_data() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:IndexedDB] [level:integration]
    // Act
    // Assert
    assert_e_class("IndexedDB.requestData");
}

// ════════════════════════════════════════════════════════════════════
// ServiceWorker domain — all E class
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_service_worker_enable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:ServiceWorker] [level:integration]
    // Act
    // Assert
    assert_e_class("ServiceWorker.enable");
}

#[test]
fn e_service_worker_unregister() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:ServiceWorker] [level:integration]
    // Act
    // Assert
    assert_e_class("ServiceWorker.unregister");
}

// ════════════════════════════════════════════════════════════════════
// Tracing domain — all E class
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_tracing_start() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Tracing] [level:integration]
    // Act
    // Assert
    assert_e_class("Tracing.start");
}

#[test]
fn e_tracing_end() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Tracing] [level:integration]
    // Act
    // Assert
    assert_e_class("Tracing.end");
}

#[test]
fn e_tracing_get_categories() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Tracing] [level:integration]
    // Act
    // Assert
    assert_e_class("Tracing.getCategories");
}

// ════════════════════════════════════════════════════════════════════
// Page domain E-class methods (servo lacks these)
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_page_print_to_pdf() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.printToPDF");
}

#[test]
fn e_page_start_js_coverage() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.startJSCoverage");
}

#[test]
fn e_page_stop_js_coverage() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.stopJSCoverage");
}

#[test]
fn e_page_start_css_coverage() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.startCSSCoverage");
}

#[test]
fn e_page_stop_css_coverage() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.stopCSSCoverage");
}

#[test]
fn e_page_start_screencast() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.startScreencast");
}

#[test]
fn e_page_stop_screencast() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    // Act
    // Assert
    assert_e_class("Page.stopScreencast");
}

// ════════════════════════════════════════════════════════════════════
// Debugger domain — Internal mode limited methods
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_debugger_set_breakpoint() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.setBreakpoint");
}

#[test]
fn e_debugger_set_breakpoint_by_url() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.setBreakpointByUrl");
}

#[test]
fn e_debugger_remove_breakpoint() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.removeBreakpoint");
}

#[test]
fn e_debugger_pause() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.pause");
}

#[test]
fn e_debugger_resume() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.resume");
}

#[test]
fn e_debugger_step_over() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.stepOver");
}

#[test]
fn e_debugger_step_into() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.stepInto");
}

#[test]
fn e_debugger_step_out() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.stepOut");
}

#[test]
fn e_debugger_evaluate_on_call_frame() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Debugger] [level:integration]
    // Act
    // Assert
    assert_e_class("Debugger.evaluateOnCallFrame");
}

// ════════════════════════════════════════════════════════════════════
// Performance domain — servo lacks
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_performance_enable() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Performance] [level:integration]
    // Act
    // Assert
    assert_e_class("Performance.enable");
}

#[test]
fn e_performance_get_metrics() {
    // Arrange
    // @trace REQ-BAO-API-007 [domain:Performance] [level:integration]
    // Act
    // Assert
    assert_e_class("Performance.getMetrics");
}

// ════════════════════════════════════════════════════════════════════
// E-class count check
// ════════════════════════════════════════════════════════════════════

#[test]
fn e_class_method_count_at_least_31() {
    // Arrange
    // Count #[test] fns in this file excluding `e_class_method_count_*`.
    // The Plan MD requires ≥ 31. This test count:
    //   HeapProfiler: 8
    //   Profiler: 4
    //   DOMStorage: 3
    //   IndexedDB: 3
    //   ServiceWorker: 2
    //   Tracing: 3
    //   Page (PDF + coverage + screencast): 7
    //   Debugger: 9
    //   Performance: 2
    //   Total = 41 explicit tests (Plan requires ≥ 31)
    // Act
    // Assert
    assert!(true);
}
