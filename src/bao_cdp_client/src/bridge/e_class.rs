//! E 类 31 method — servo 不支持,统一返回 -32601 (MethodNotFound)。
//!
//! servo 与 Chrome 不同:
//! - **无 HeapProfiler**:SpiderMonkey 内置 heap snapshot 路径未暴露到 devtools
//! - **无 Profiler**:servo 无 gecko profiler 桥接
//! - **无 Tracing**:servo 无 tracing actor
//! - **无 Performance metrics**:servo 无 timeline actor 完整桥接
//! - **无 PDF 渲染**:servo 无 print-to-pdf
//! - **无 Coverage(JSCSS)**:servo 无 coverage actor
//! - **无 DOMStorage / IndexedDB / ServiceWorker actors**:servo 这几个 actor
//!   在 Bao 编译配置下未启用
//! - **Debugger 已接入(BUG-CDP-006)**:Debugger domain 9 method 已接入 servo SM
//!   Debugger API,不再属于 E 类。详见 `debugger_handlers`。
//!
//! 所有 E 类返回 `BridgeError::NotSupported`,dispatcher 映射为 JSON-RPC error code -32601。
//!
//! # E 类覆盖范围
//!
//! | Domain | Methods |
//! |--------|---------|
//! | Page | printToPDF, start/stopJSCoverage, start/stopCSSCoverage, start/stopScreencast, screencastFrameAck, handleJavaScriptDialog, printToPDFAndDownload |
//! | HeapProfiler | enable, disable, startTrackingHeapObjects, stopTrackingHeapObjects, takeHeapSnapshot, getObjectByHeapObjectId, getSamplingProfile, startSampling, stopSampling, collectGarbage (domain 全集拦截) |
//! | Profiler | enable, disable, start, stop, setSamplingInterval, getBestEffortCoverage (domain 全集拦截) |
//! | DOMStorage | getDOMStorageItems, setDOMStorageItem, removeDOMStorageItem, clearDOMStorageItems (domain 全集拦截) |
//! | IndexedDB | requestDatabaseNames, requestDatabase, requestData, deleteDatabase (domain 全集拦截) |
//! | ServiceWorker | enable, disable, unregister (domain 全集拦截) |
//! | Tracing | start, end, getCategories (domain 全集拦截) |
//! | Performance | enable, disable, getMetrics |
//!
//! @trace REQ-BAO-API-007 [level:library]
//! @trace BUG-CDP-006 [domain:Debugger]

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
    fn debugger_breakpoint_no_longer_e_class_after_bug_cdp_006() {
        // BUG-CDP-006: Debugger 域 9 method 不再是 E 类,已接入 servo SM Debugger API。
        // @trace BUG-CDP-006 [domain:Debugger]
        assert!(!is_e_class("Debugger", "setBreakpoint"));
        assert!(!is_e_class("Debugger", "setBreakpointByUrl"));
        assert!(!is_e_class("Debugger", "removeBreakpoint"));
        assert!(!is_e_class("Debugger", "pause"));
        assert!(!is_e_class("Debugger", "resume"));
        assert!(!is_e_class("Debugger", "stepOver"));
        assert!(!is_e_class("Debugger", "stepInto"));
        assert!(!is_e_class("Debugger", "stepOut"));
        assert!(!is_e_class("Debugger", "evaluateOnCallFrame"));
        assert!(!is_e_class("Debugger", "setBreakpointsActive"));
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
    fn e_class_methods_count_at_least_10() {
        // Plan MD 原要求 31 method。BUG-CDP-006 把 Debugger 9 method 从 E 类移出
        // 后(接入 servo SM Debugger API),E_CLASS_METHODS 收缩。
        // E_CLASS_DOMAINS 仍包含 6 domain(HeapProfiler/Profiler/DOMStorage/
        // IndexedDB/ServiceWorker/Tracing),整体覆盖远超 31(每个 domain 全域拦截)。
        // E_CLASS_METHODS 现保留 Page 域补丁(10+ 项) + Performance(3 项)。
        // @trace BUG-CDP-006 [domain:Debugger]
        assert!(
            E_CLASS_METHODS.len() >= 10,
            "E_CLASS_METHODS too small: {}",
            E_CLASS_METHODS.len()
        );
        assert!(E_CLASS_DOMAINS.len() >= 6);
    }
}
