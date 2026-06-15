//! Log domain conformance 审计 — Log.entryAdded 事件。
//!
//! 对照 CDP 官方规范(Log domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Log/
//!
//! bao 不直接实现 Log domain method,但 servo Console 事件翻译为 Log.entryAdded 事件。
//! 此文件验证事件 schema conformance。
//!
//! @trace REQ-CDP-001 [domain:Log] [level:integration]
//! @trace REQ-BAO-API-003 [event:Log.entryAdded] [level:integration]

use bao_cdp_client::{translate_event, ConsoleLevel, ServoEvent};

// ─────────────────────────────────────────────────────────────────────────
// Log.entryAdded event — CDP spec: {entry: {source, level, text, url?, timestamp, ...}}
// Entry.source ∈ {xml, javascript, network, storage, appcache, rendering,
//   security, deprecation, worker, violation, intervention, recommendation, other}
// Entry.level ∈ {verbose, info, warning, error}
// https://chromedevtools.github.io/devtools-protocol/tot/Log/#event-entryAdded
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn log_entry_added_event_schema_conformance() {
    // Arrange — servo Console 事件 → Log.entryAdded
    // @trace REQ-CDP-001 [domain:Log] [level:integration]
    // @trace REQ-BAO-API-003 [event:Log.entryAdded] [level:integration]
    let servo_event = ServoEvent::Console {
        target_id: "1".to_string(),
        level: ConsoleLevel::Info,
        text: "hello".to_string(),
        url: Some("https://x/page.js".to_string()),
        line: Some(10),
        column: Some(5),
    };

    // Act
    let events = translate_event(servo_event);

    // Assert — exactly one CdpEvent
    assert_eq!(events.len(), 1, "Console → exactly 1 Log.entryAdded");
    let ev = &events[0];

    // CDP spec: method = "Log.entryAdded"
    assert_eq!(
        ev.method, "Log.entryAdded",
        "CDP spec: servo Console → Log.entryAdded"
    );

    // CDP spec: entry object
    let entry = &ev.params["entry"];
    assert!(
        entry.is_object(),
        "CDP spec: entry must be object, got: {:?}",
        entry
    );
    assert!(entry["source"].is_string(), "LogEntry.source must be string");
    assert!(entry["level"].is_string(), "LogEntry.level must be string");
    assert!(entry["text"].is_string(), "LogEntry.text must be string");
    assert!(
        entry["timestamp"].is_i64() || entry["timestamp"].is_u64(),
        "LogEntry.timestamp must be integer (ms)"
    );
}

#[test]
fn log_entry_added_source_matches_cdp_enum() {
    // Arrange — CDP 规范: source ∈ {xml, javascript, network, storage, appcache,
    // rendering, security, deprecation, worker, violation, intervention, recommendation, other}
    // bao Console 事件固定映射到 "javascript"
    // @trace REQ-CDP-001 [domain:Log] [level:integration]
    let servo_event = ServoEvent::Console {
        target_id: "1".into(),
        level: ConsoleLevel::Info,
        text: "x".into(),
        url: None,
        line: None,
        column: None,
    };
    let events = translate_event(servo_event);
    let source = events[0].params["entry"]["source"].as_str().unwrap();

    // Assert
    let valid_sources = [
        "xml", "javascript", "network", "storage", "appcache", "rendering",
        "security", "deprecation", "worker", "violation", "intervention",
        "recommendation", "other",
    ];
    assert!(
        valid_sources.contains(&source),
        "CDP spec: source must be valid LogEntrySource, got: {}",
        source
    );
}

#[test]
fn log_entry_added_level_mapping_conformance() {
    // Arrange — CDP 规范: level ∈ {verbose, info, warning, error}
    // bao ConsoleLevel: Verbose, Info, Warning, Error, Debug
    // 注意:bao ConsoleLevel::Debug → "debug"(非 CDP 规范值)→ 在 conformance_log_level_deviation 中记录
    // @trace REQ-CDP-001 [domain:Log] [level:integration]
    for level in [
        ConsoleLevel::Verbose,
        ConsoleLevel::Info,
        ConsoleLevel::Warning,
        ConsoleLevel::Error,
    ] {
        let servo_event = ServoEvent::Console {
            target_id: "1".into(),
            level,
            text: "x".into(),
            url: None,
            line: None,
            column: None,
        };
        let events = translate_event(servo_event);
        let actual = events[0].params["entry"]["level"].as_str().unwrap();
        let valid_levels = ["verbose", "info", "warning", "error"];
        assert!(
            valid_levels.contains(&actual),
            "CDP spec: level must be valid LogEntryLevel, got: {}",
            actual
        );
    }
}

#[test]
fn log_entry_added_debug_level_deviation_documented() {
    // Arrange — CDP 规范: EntryLevel ∈ {verbose, info, warning, error} 无 "debug"
    // bao ConsoleLevel::Debug → to_cdp_str 返回 "debug"(非规范值)
    // 此测试断言"当前偏差",修复后会 fail → 提示更新报告
    // @trace REQ-CDP-001 [domain:Log] [level:integration]
    let servo_event = ServoEvent::Console {
        target_id: "1".into(),
        level: ConsoleLevel::Debug,
        text: "x".into(),
        url: None,
        line: None,
        column: None,
    };
    let events = translate_event(servo_event);
    let actual = events[0].params["entry"]["level"].as_str().unwrap();

    // Assert — 记录偏差:ConsoleLevel::Debug 输出 "debug" 而非 "verbose"
    assert_eq!(
        actual, "debug",
        "DEV-NOTE: bao emits `debug` for ConsoleLevel::Debug (CDP spec EntryLevel doesn't \
         include `debug`; should be `verbose`). If this fails, bao has aligned — update \
         CONFORMANCE_REPORT."
    );
}

#[test]
fn log_entry_added_carries_session_id() {
    // Arrange — CDP 规范: flat-mode 事件应携带 sessionId
    // bao 把 target_id 作为 session_id 传递
    // @trace REQ-BAO-API-003 [event:Log.entryAdded] [level:integration]
    let servo_event = ServoEvent::Console {
        target_id: "page-42".into(),
        level: ConsoleLevel::Info,
        text: "x".into(),
        url: None,
        line: None,
        column: None,
    };
    let events = translate_event(servo_event);

    // Assert
    assert_eq!(
        events[0].session_id.as_deref(),
        Some("page-42"),
        "bao convention: target_id passed as sessionId"
    );
}

#[test]
fn log_entry_added_missing_location_defaults_to_zero() {
    // Arrange — CDP 规范: lineNumber / columnNumber 可选,缺失默认 0
    // @trace REQ-CDP-001 [domain:Log] [level:integration]
    let servo_event = ServoEvent::Console {
        target_id: "1".into(),
        level: ConsoleLevel::Info,
        text: "no location".into(),
        url: None,
        line: None,
        column: None,
    };
    let events = translate_event(servo_event);
    let entry = &events[0].params["entry"];

    // Assert — lineNumber / columnNumber 应为 0(默认值)
    assert_eq!(
        entry["lineNumber"].as_i64(),
        Some(0),
        "CDP spec: missing lineNumber defaults to 0"
    );
    assert_eq!(
        entry["columnNumber"].as_i64(),
        Some(0),
        "CDP spec: missing columnNumber defaults to 0"
    );
}
