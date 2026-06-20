//! Smoke test for the bao_lints BCE-012 detector.
//!
//! Verifies the detector catches every BCE-012 signature (positive cases) and
//! ignores every documented safe exemption (negative cases). Format-immune —
//! the test fixtures use multi-line layout to defeat regex/grep detectors.

use std::path::PathBuf;

/// Run the scanner over an in-memory source string.
fn scan(src: &str) -> Vec<String> {
    let path = PathBuf::from("test_input.rs");
    bao_lints::detector::scan_source(&path, src)
        .into_iter()
        .map(|f| f.message)
        .collect()
}

// ─── Positive: BCE-012 violations (must be detected) ───────────────────────

#[test]
fn detects_handle_jsobject_ptr_non_null_inline_ref() {
    // The canonical BCE-012 case from BUG-KNOWLEDGE.md:
    //   Handle::<*mut JSObject> { ..., ptr: &local_var }
    let src = r#"
        fn f() {
            let local: *mut JSObject = std::ptr::null_mut();
            let _h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &local,
            };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("*mut JSObject")),
        "expected BCE-012 finding for Handle<*mut JSObject>, got: {:?}", findings
    );
}

#[test]
fn detects_handle_jsstring_ptr_non_null_ref() {
    let src = r#"
        fn f() {
            let s: *mut JSString = std::ptr::null_mut();
            let _h = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &s };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("*mut JSString")),
        "expected BCE-012 finding for Handle<*mut JSString>, got: {:?}", findings
    );
}

#[test]
fn detects_handle_value_inline_object_value() {
    // Inline `&ObjectValue(...)` form — must be detected even without backtracking.
    let src = r#"
        fn f() {
            let _h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &ObjectValue(obj),
            };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("ObjectValue")),
        "expected BCE-012 finding for inline ObjectValue, got: {:?}", findings
    );
}

#[test]
fn detects_handle_value_backtrack_to_string_value() {
    // `&IDENT` form where IDENT is bound to StringValue — backtracking must catch it.
    let src = r#"
        fn f() {
            let val = StringValue(js_str);
            let _h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("StringValue") && m.contains("val")),
        "expected BCE-012 finding for backtrack to StringValue, got: {:?}", findings
    );
}

#[test]
fn detects_handle_jsval_backtrack_to_object_value() {
    // `Handle::<JSVal>` with backtrack to ObjectValue (JSVal is the same type as Value).
    let src = r#"
        fn f() {
            let v = ObjectValue(obj);
            let _h = Handle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("ObjectValue")),
        "expected BCE-012 finding for JSVal + ObjectValue backtrack, got: {:?}", findings
    );
}

// ─── Negative: safe exemptions (must NOT be flagged) ───────────────────────

#[test]
fn ignores_null_mut_handle_jsobject() {
    // Safe exemption: `&null_mut()` — GC does not move null pointers.
    let src = r#"
        fn f() {
            let _h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &std::ptr::null_mut(),
            };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for null_mut, got: {:?}", findings
    );
}

#[test]
fn ignores_handle_value_boolean_primitive() {
    // Safe exemption: BooleanValue is a primitive (no GC pointer).
    let src = r#"
        fn f() {
            let v = BooleanValue(true);
            let _h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for BooleanValue, got: {:?}", findings
    );
}

#[test]
fn ignores_handle_value_int32_primitive() {
    let src = r#"
        fn f() {
            let n = Int32Value(42);
            let _h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &n };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for Int32Value, got: {:?}", findings
    );
}

#[test]
fn ignores_handle_value_undefined_primitive() {
    let src = r#"
        fn f() {
            let u = UndefinedValue();
            let _h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &u };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for UndefinedValue, got: {:?}", findings
    );
}

#[test]
fn ignores_mutable_handle_value() {
    // MutableHandle is an output param — uses `&mut` and is safe by design.
    let src = r#"
        fn f() {
            let mut v = UndefinedValue();
            let _h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut v };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for MutableHandle, got: {:?}", findings
    );
}

#[test]
fn ignores_unknown_payload_kind() {
    // Handle<*mut SomeOtherType> is out of BCE-012 scope.
    let src = r#"
        fn f() {
            let x: *mut Other = std::ptr::null_mut();
            let _h = Handle::<*mut Other> { _phantom_0: ::std::marker::PhantomData, ptr: &x };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().all(|m| !m.contains("BCE-012")),
        "expected no BCE-012 finding for unknown payload, got: {:?}", findings
    );
}

// ─── Format immunity ───────────────────────────────────────────────────────

#[test]
fn format_immunity_multiline_field_order() {
    // Field order shuffled + multi-line — must still be detected.
    let src = r#"
        fn f() {
            let o: *mut JSObject = std::ptr::null_mut();
            let _h = Handle::<*mut JSObject> {
                ptr: &o,
                _phantom_0: ::std::marker::PhantomData,
            };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("*mut JSObject")),
        "expected BCE-012 finding regardless of field order, got: {:?}", findings
    );
}

#[test]
fn format_immunity_path_prefix() {
    // `mozjs::jsapi::Handle` — fully qualified path, must still be detected.
    let src = r#"
        fn f() {
            let o: *mut JSObject = std::ptr::null_mut();
            let _h = mozjs::jsapi::Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &o,
            };
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("*mut JSObject")),
        "expected BCE-012 finding for fully-qualified Handle path, got: {:?}", findings
    );
}

#[test]
fn detects_handle_value_backtrack_in_nested_block() {
    // Backtrack across a nested block scope — locals must propagate.
    let src = r#"
        fn f() {
            let val = ObjectValue(obj);
            {
                let _h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            }
        }
    "#;
    let findings = scan(src);
    assert!(
        findings.iter().any(|m| m.contains("BCE-012") && m.contains("ObjectValue")),
        "expected BCE-012 finding across nested block, got: {:?}", findings
    );
}
