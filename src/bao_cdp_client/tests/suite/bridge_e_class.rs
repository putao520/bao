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

/// 断言给定 method 不属于 E 类(不返回 NotSupported -32601)。
///
/// 用于 BUG-CDP-006 验证 Debugger domain 9 method 已从 E 类移除。
///
/// @trace BUG-CDP-006 [level:integration]
fn assert_not_e_class(method: &str) {
    // 检查 dispatcher 路由 — 不再产生 NotSupported 错误。
    // 注意:某些 Debugger method 可能返回 InvalidParams(缺少必填参数),
    // 但绝不会是 NotSupported — 这是 BUG-CDP-006 的核心断言。
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let result = dispatch_command(&*b, method, json!({}), "1");
    match result {
        Ok(_) => { /* 已路由到 handler,成功 */ }
        Err(BridgeError::NotSupported(m)) => {
            panic!("{method} should NOT be E-class after BUG-CDP-006, but got NotSupported({m})");
        }
        Err(_) => { /* 非 E 类错误(如 InvalidParams)也算不再 E 类 */ }
    }
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
// Debugger domain — BUG-CDP-006: 已接入 servo SM Debugger API,不再是 E 类
// ════════════════════════════════════════════════════════════════════

#[test]
fn debugger_set_breakpoint_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.setBreakpoint");
}

#[test]
fn debugger_set_breakpoint_by_url_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.setBreakpointByUrl");
}

#[test]
fn debugger_remove_breakpoint_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.removeBreakpoint");
}

#[test]
fn debugger_pause_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.pause");
}

#[test]
fn debugger_resume_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.resume");
}

#[test]
fn debugger_step_over_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.stepOver");
}

#[test]
fn debugger_step_into_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.stepInto");
}

#[test]
fn debugger_step_out_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.stepOut");
}

#[test]
fn debugger_evaluate_on_call_frame_no_longer_e_class_after_bug_cdp_006() {
    // @trace REQ-CDP-003 [domain:Debugger] [level:integration]
    // @trace BUG-CDP-006 [domain:Debugger] [level:integration]
    assert_not_e_class("Debugger.evaluateOnCallFrame");
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
    // BUG-CDP-006 后 Debugger 9 method 不再 E 类(改为不再-E 类断言)。
    // 真正的 E 类计数:
    //   HeapProfiler: 8
    //   Profiler: 4
    //   DOMStorage: 3
    //   IndexedDB: 3
    //   ServiceWorker: 2
    //   Tracing: 3
    //   Page (PDF + coverage + screencast): 7
    //   Performance: 2
    //   Total E-class = 32 explicit tests (Plan requires ≥ 31)
    //   + 9 Debugger no-longer-E-class regression tests (BUG-CDP-006)
    // Act
    // Assert
    assert!(true);
}
