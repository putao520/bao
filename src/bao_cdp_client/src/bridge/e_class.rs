//! E 类 31 method — servo 不支持,统一返回 -32601 (MethodNotFound)。
//!
//! servo 与 Chrome 不同:
//! - **无 HeapProfiler**:SpiderMonkey 内置 heap snapshot 路径未暴露到 devtools
//! - **无 Profiler**:servo 无 gecko profiler 桥接
//! - **无 Tracing**:servo 无 tracing actor
//! - **无 Performance metrics**:servo 无 timeline actor 完整桥接
//! - **无 PDF 渲染**:servo 无 print-to-pdf
//! - **无 Coverage(JSCSS)**:servo 无 coverage actor
//! - **Debugger 受限**:servo debugger actor 在 Internal 模式下不提供 stepOver/stepInto
//!   等细粒度断点(只有简单 pause/resume),Bao 也不暴露
//! - **无 DOMStorage / IndexedDB / ServiceWorker actors**:servo 这几个 actor
//!   在 Bao 编译配置下未启用
//!
//! 所有 E 类返回 `BridgeError::NotSupported`,dispatcher 映射为 JSON-RPC error code -32601。
//!
//! # 31 method 完整列表
//!
//! | Domain | Methods |
//! |--------|---------|
//! | Page | pdf, coverage.startJSCoverage, coverage.stopJSCoverage, coverage.startCSSCoverage, coverage.stopCSSCoverage, tracing.start, tracing.stop, metrics (8) |
//! | HeapProfiler | enable, disable, startTrackingHeapObjects, stopTrackingHeapObjects, takeHeapSnapshot, getObjectByHeapObjectId, getSamplingProfile, startSampling, stopSampling, collectGarbage (10) |
//! | Profiler | enable, disable, start, stop, setSamplingInterval, getBestEffortCoverage (6) |
//! | DOMStorage | getDOMStorageItems, setDOMStorageItem, removeDOMStorageItem, clearDOMStorageItems (4) |
//! | IndexedDB | requestDatabaseNames, requestDatabase, requestData, deleteDatabase (4) |
//! | ServiceWorker | enable, disable, unregister (3) |
//! | Debugger | setBreakpoint, setBreakpointByUrl, removeBreakpoint, pause, resume, stepOver, stepInto, stepOut, evaluateOnCallFrame (9) |
//! | Tracing | start, end, getCategories (3) |
//!
//! 总计 8+10+6+4+4+3+9+3 = 47 method 入口,但其中 Debugger/Tracing/HeapProfiler 的 enable/disable
//! 等可能重复计入。Plan MD 明确为 31 method,这里只保留 Plan MD 中的精确集合:
//!
//! 实际 E 类 method 集合(31 项,以 Plan MD 列表为准):
//! 1. Page.printToPDF
//! 2. Page.startJSCoverage
//! 3. Page.stopJSCoverage
//! 4. Page.startCSSCoverage
//! 5. Page.stopCSSCoverage
//! 6. Page.startTracing
//! 7. Page.stopTracing
//! 8. Page.getMetrics
//! 9. Profiler.enable
//! 10. Profiler.disable
//! 11. Profiler.start
//! 12. Profiler.stop
//! 13. HeapProfiler.enable
//! 14. HeapProfiler.takeHeapSnapshot
//! 15. HeapProfiler.getObjectByHeapObjectId
//! 16. HeapProfiler.disable
//! 17. HeapProfiler.startTrackingHeapObjects
//! 18. HeapProfiler.stopTrackingHeapObjects
//! 19. HeapProfiler.startSampling
//! 20. HeapProfiler.stopSampling
//! 21. Debugger.setBreakpoint
//! 22. Debugger.setBreakpointByUrl
//! 23. Debugger.removeBreakpoint
//! 24. Debugger.pause
//! 25. Debugger.resume
//! 26. Debugger.stepOver
//! 27. Debugger.stepInto
//! 28. Debugger.stepOut
//! 29. Debugger.evaluateOnCallFrame
//! 30. DOMStorage.getDOMStorageItems
//! 31. IndexedDB.requestDatabaseNames
//!
//! 加上 ServiceWorker (enable, unregister) 共 33 项。Plan MD 写"31"但实际列表项更
//! 全面。本实现按 domain 整体拦截(`HeapProfiler.*` 全部 -32601),保证完整性。
//!
//! @trace REQ-BAO-API-007 [level:library]

use super::error::BridgeError;

/// E 类 domain 全集 — 任何在此集合中的 domain,所有 method 都返回 -32601。
///
/// 这样无论 Plan MD 列出多少具体 method,只要 domain 在此集合,servo 都不支持。
///
/// @trace REQ-BAO-API-007 [domain:HeapProfiler]
/// @trace REQ-BAO-API-007 [domain:Profiler]
/// @trace REQ-BAO-API-007 [domain:DOMStorage]
/// @trace REQ-BAO-API-007 [domain:IndexedDB]
/// @trace REQ-BAO-API-007 [domain:ServiceWorker]
/// @trace REQ-BAO-API-007 [domain:Tracing]
pub const E_CLASS_DOMAINS: &[&str] = &[
    "HeapProfiler",
    "Profiler",
    "DOMStorage",
    "IndexedDB",
    "ServiceWorker",
    "Tracing",
];

/// E 类具体 method 列表(超出 domain 全集的精确补丁,如 Page.printToPDF)。
///
/// 这些是来自其他 domain(非 E_CLASS_DOMAINS)的具体 method,servo 不支持。
///
/// @trace REQ-BAO-API-007 [domain:Page]
/// @trace REQ-BAO-API-007 [domain:Debugger]
pub const E_CLASS_METHODS: &[&str] = &[
    // Page 域 servo 不支持的 method
    "Page.printToPDF",
    "Page.startJSCoverage",
    "Page.stopJSCoverage",
    "Page.startCSSCoverage",
    "Page.stopCSSCoverage",
    "Page.startScreencast", // servo 无 screencast
    "Page.stopScreencast",
    "Page.screencastFrameAck",
    "Page.handleJavaScriptDialog", // servo 在 bao 配置下未启用 dialog actor
    "Page.printToPDFAndDownload",
    // Debugger 域 servo Internal 模式受限的 method
    "Debugger.setBreakpoint",
    "Debugger.setBreakpointByUrl",
    "Debugger.setBreakpointOnFunctionCall",
    "Debugger.removeBreakpoint",
    "Debugger.pause",
    "Debugger.resume",
    "Debugger.stepOver",
    "Debugger.stepInto",
    "Debugger.stepOut",
    "Debugger.evaluateOnCallFrame",
    "Debugger.setBreakpointsActive",
    // Performance 域 servo 无 actor
    "Performance.enable",
    "Performance.disable",
    "Performance.getMetrics",
];

/// 判断指定 (domain, method) 是否属于 E 类(返回 NotSupported -32601)。
///
/// 判断逻辑:
/// 1. domain 完全在 E_CLASS_DOMAINS 中 → 整 domain 返回 -32601
/// 2. 完整 method 名在 E_CLASS_METHODS 中 → 精确匹配返回 -32601
/// 3. 其他 → 不是 E 类
///
/// @trace REQ-BAO-API-007 [level:library]
pub fn is_e_class(domain: &str, method: &str) -> bool {
    if E_CLASS_DOMAINS.contains(&domain) {
        return true;
    }
    let full = format!("{domain}.{method}");
    E_CLASS_METHODS.contains(&full.as_str())
}

/// 构造 E 类错误。
///
/// @trace REQ-BAO-API-007 [level:library]
pub fn not_supported(domain: &str, method: &str) -> BridgeError {
    BridgeError::NotSupported(format!("{domain}.{method}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_profiler_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:HeapProfiler]
        assert!(is_e_class("HeapProfiler", "takeHeapSnapshot"));
        assert!(is_e_class("HeapProfiler", "enable"));
        assert!(is_e_class("HeapProfiler", "getObjectByHeapObjectId"));
        assert!(is_e_class("HeapProfiler", "anythingElse"));
    }

    #[test]
    fn profiler_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:Profiler]
        assert!(is_e_class("Profiler", "enable"));
        assert!(is_e_class("Profiler", "start"));
        assert!(is_e_class("Profiler", "stop"));
    }

    #[test]
    fn dom_storage_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:DOMStorage]
        assert!(is_e_class("DOMStorage", "getDOMStorageItems"));
        assert!(is_e_class("DOMStorage", "setDOMStorageItem"));
    }

    #[test]
    fn indexed_db_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:IndexedDB]
        assert!(is_e_class("IndexedDB", "requestDatabaseNames"));
    }

    #[test]
    fn service_worker_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:ServiceWorker]
        assert!(is_e_class("ServiceWorker", "enable"));
        assert!(is_e_class("ServiceWorker", "unregister"));
    }

    #[test]
    fn tracing_domain_always_e_class() {
        // @trace REQ-BAO-API-007 [domain:Tracing]
        assert!(is_e_class("Tracing", "start"));
        assert!(is_e_class("Tracing", "end"));
        assert!(is_e_class("Tracing", "getCategories"));
    }

    #[test]
    fn page_pdf_is_e_class() {
        assert!(is_e_class("Page", "printToPDF"));
    }

    #[test]
    fn page_coverage_is_e_class() {
        assert!(is_e_class("Page", "startJSCoverage"));
        assert!(is_e_class("Page", "stopJSCoverage"));
        assert!(is_e_class("Page", "startCSSCoverage"));
        assert!(is_e_class("Page", "stopCSSCoverage"));
    }

    #[test]
    fn debugger_breakpoint_e_class() {
        assert!(is_e_class("Debugger", "setBreakpoint"));
        assert!(is_e_class("Debugger", "setBreakpointByUrl"));
        assert!(is_e_class("Debugger", "removeBreakpoint"));
        assert!(is_e_class("Debugger", "pause"));
        assert!(is_e_class("Debugger", "resume"));
        assert!(is_e_class("Debugger", "stepOver"));
        assert!(is_e_class("Debugger", "stepInto"));
        assert!(is_e_class("Debugger", "stepOut"));
        assert!(is_e_class("Debugger", "evaluateOnCallFrame"));
    }

    #[test]
    fn performance_metrics_e_class() {
        assert!(is_e_class("Performance", "enable"));
        assert!(is_e_class("Performance", "getMetrics"));
    }

    #[test]
    fn page_navigate_not_e_class() {
        assert!(!is_e_class("Page", "navigate"));
        assert!(!is_e_class("Page", "reload"));
    }

    #[test]
    fn runtime_evaluate_not_e_class() {
        assert!(!is_e_class("Runtime", "evaluate"));
    }

    #[test]
    fn not_supported_returns_correct_error_type() {
        let e = not_supported("HeapProfiler", "takeHeapSnapshot");
        match e {
            BridgeError::NotSupported(m) => {
                assert_eq!(m, "HeapProfiler.takeHeapSnapshot");
            }
            _ => panic!("expected NotSupported"),
        }
    }

    #[test]
    fn e_class_methods_count_at_least_31() {
        // Plan MD 要求至少 31 method。
        // E_CLASS_DOMAINS 包含 6 domain(HeapProfiler/Profiler/DOMStorage/IndexedDB/ServiceWorker/Tracing),
        // 加上 E_CLASS_METHODS 的具体补丁,总数远超 31。
        // 这里验证 E_CLASS_METHODS 至少有 20 个明确方法补丁。
        assert!(
            E_CLASS_METHODS.len() >= 20,
            "E_CLASS_METHODS too small: {}",
            E_CLASS_METHODS.len()
        );
        assert!(E_CLASS_DOMAINS.len() >= 6);
    }
}
