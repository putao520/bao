// @trace TEST-ENG-017 [req:REQ-ENG-001,REQ-ENG-002] [level:unit]
// JsError construction, Display, std::error::Error, field access.
// ParseResult fields, parse_classes edge cases, proto/klass block parsing,
// multi-class source, configurable/hasPendingActivity flags.

use bao_engine::codegen::{
    ClassDef, PropertyDef, PropertyKind, ParseResult,
    parse_classes, generate_bindings, generate_all,
};

// ---- JsError ----

#[test]
fn test_js_error_construction() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "Unexpected token".into(),
        filename: "test.js".into(),
        line: 42,
        column: 10,
        stack: None,
    };
    assert_eq!(err.message, "Unexpected token");
    assert_eq!(err.filename, "test.js");
    assert_eq!(err.line, 42);
    assert_eq!(err.column, 10);
    assert!(err.stack.is_none());
}

#[test]
fn test_js_error_display_basic() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "SyntaxError".into(),
        filename: "app.js".into(),
        line: 10,
        column: 5,
        stack: None,
    };
    let msg = format!("{}", err);
    assert!(msg.contains("app.js:10:5: SyntaxError"));
}

#[test]
fn test_js_error_display_with_stack() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "TypeError".into(),
        filename: "main.js".into(),
        line: 1,
        column: 1,
        stack: Some("at foo (main.js:1:1)\nat bar (main.js:5:3)".into()),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("main.js:1:1: TypeError"));
    assert!(msg.contains("\nat foo"));
}

#[test]
fn test_js_error_display_stack_appended() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "err".into(),
        filename: "f".into(),
        line: 0,
        column: 0,
        stack: Some("stack trace here".into()),
    };
    let msg = format!("{}", err);
    assert!(msg.ends_with("stack trace here"));
}

#[test]
fn test_js_error_debug() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "test".into(),
        filename: "a.js".into(),
        line: 1,
        column: 1,
        stack: None,
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("JsError"));
    assert!(debug.contains("test"));
}

#[test]
fn test_js_error_is_std_error() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "x".into(),
        filename: "y".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    let _: Box<dyn std::error::Error> = Box::new(err);
}

#[test]
fn test_js_error_empty_fields() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: String::new(),
        filename: String::new(),
        line: 0,
        column: 0,
        stack: Some(String::new()),
    };
    let msg = format!("{}", err);
    assert!(msg.contains(":0:0:"));
}

#[test]
fn test_js_error_large_line_column() {
    use bao_engine::error::JsError;
    let err = JsError {
        message: "big".into(),
        filename: "huge.js".into(),
        line: u32::MAX,
        column: u32::MAX,
        stack: None,
    };
    let msg = format!("{}", err);
    assert!(msg.contains(&u32::MAX.to_string()));
}

// ---- parse_classes edge cases ----

#[test]
fn test_parse_whitespace_only() {
    let result = parse_classes("   \n  \n  \t  ", "ws.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_comment_lines() {
    let source = "// This is a comment\n/* block comment */\n# hash comment\n";
    let result = parse_classes(source, "comment.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_name_without_quotes() {
    let source = "name: MyObject";
    let result = parse_classes(source, "bare.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    assert_eq!(result.classes[0].name, "MyObject");
}

#[test]
fn test_parse_name_with_double_quotes() {
    let source = r#"name: "QuotedClass","#;
    let result = parse_classes(source, "quoted.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    assert_eq!(result.classes[0].name, "QuotedClass");
}

#[test]
fn test_parse_construct_flag() {
    let source = r#"
name: "TestClass",
construct: true,
finalize: true,
configurable: false,
"#;
    let result = parse_classes(source, "test.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    let cd = &result.classes[0];
    assert!(cd.construct);
    assert!(cd.finalize);
    assert!(!cd.configurable);
}

#[test]
fn test_parse_no_constructor_flag() {
    let source = r#"
name: "NoCtor",
noConstructor: true,
"#;
    let result = parse_classes(source, "noctor.ts").unwrap();
    let cd = &result.classes[0];
    assert!(cd.no_constructor);
}

#[test]
fn test_parse_configurable_default() {
    // configurable defaults to true when "configurable: false" is NOT present
    let source = r#"name: "DefaultCfg""#;
    let result = parse_classes(source, "cfg.ts").unwrap();
    assert!(result.classes[0].configurable);
}

#[test]
fn test_parse_configurable_false() {
    let source = r#"
name: "FixedCfg",
configurable: false,
"#;
    let result = parse_classes(source, "cfg2.ts").unwrap();
    assert!(!result.classes[0].configurable);
}

#[test]
fn test_parse_has_pending_activity() {
    let source = r#"
name: "Active",
hasPendingActivity: true,
"#;
    let result = parse_classes(source, "active.ts").unwrap();
    assert!(result.classes[0].has_pending_activity);
}

#[test]
fn test_parse_has_pending_activity_default() {
    let source = r#"name: "Inactive""#;
    let result = parse_classes(source, "inactive.ts").unwrap();
    assert!(!result.classes[0].has_pending_activity);
}

#[test]
fn test_parse_result_source_file() {
    let result = parse_classes("", "my_file.ts").unwrap();
    assert_eq!(result.source_file, "my_file.ts");
}

#[test]
fn test_parse_result_debug() {
    let pr = ParseResult {
        classes: vec![],
        source_file: "debug.ts".into(),
    };
    let debug = format!("{:?}", pr);
    assert!(debug.contains("ParseResult"));
    assert!(debug.contains("debug.ts"));
}

// ---- parse_classes: proto block ----

#[test]
fn test_parse_proto_getter() {
    let source = r#"
name: "WithProto",
proto: {
    url: { getter: "get_url", cache: false }
}
"#;
    let result = parse_classes(source, "proto.ts").unwrap();
    let cd = &result.classes[0];
    assert_eq!(cd.proto.len(), 1);
    assert_eq!(cd.proto[0].name, "url");
}

#[test]
fn test_parse_proto_method() {
    let source = r#"
name: "WithMethod",
proto: {
    fetch: { fn: "do_fetch", length: 2 }
}
"#;
    let result = parse_classes(source, "method.ts").unwrap();
    let cd = &result.classes[0];
    assert_eq!(cd.proto.len(), 1);
    assert_eq!(cd.proto[0].name, "fetch");
}

#[test]
fn test_parse_proto_setter() {
    let source = r#"
name: "WithSetter",
proto: {
    value: { setter: "set_value" }
}
"#;
    let result = parse_classes(source, "setter.ts").unwrap();
    let cd = &result.classes[0];
    assert_eq!(cd.proto.len(), 1);
    assert_eq!(cd.proto[0].name, "value");
}

#[test]
fn test_parse_proto_multiple_props() {
    let source = r#"
name: "MultiProp",
proto: {
    a: { getter: "ga", cache: false }
    b: { fn: "mb", length: 0 }
    c: { setter: "sc" }
}
"#;
    let result = parse_classes(source, "multi.ts").unwrap();
    assert_eq!(result.classes[0].proto.len(), 3);
}

// ---- parse_classes: static (klass) block ----

#[test]
fn test_parse_klass_static_props() {
    let source = r#"
name: "WithStatic",
klass: {
    version: { getter: "get_version", cache: false }
}
"#;
    let result = parse_classes(source, "static.ts").unwrap();
    let cd = &result.classes[0];
    assert_eq!(cd.static_props.len(), 1);
    assert_eq!(cd.static_props[0].name, "version");
}

#[test]
fn test_parse_no_proto_no_static() {
    let source = r#"name: "Bare""#;
    let result = parse_classes(source, "bare.ts").unwrap();
    let cd = &result.classes[0];
    assert!(cd.proto.is_empty());
    assert!(cd.static_props.is_empty());
}

// ---- parse_classes: multi-class source ----

#[test]
fn test_parse_multiple_classes() {
    let source = r#"
name: "Alpha",
construct: true,
finalize: true,
configurable: false,
name: "Beta",
noConstructor: true,
construct: false,
"#;
    let result = parse_classes(source, "multi.ts").unwrap();
    assert_eq!(result.classes.len(), 2);
    assert_eq!(result.classes[0].name, "Alpha");
    assert_eq!(result.classes[1].name, "Beta");
}

#[test]
fn test_parse_many_names() {
    let mut source = String::new();
    for i in 0..20 {
        source.push_str(&format!("name: \"Class{}\",\n", i));
    }
    let result = parse_classes(&source, "many.ts").unwrap();
    assert_eq!(result.classes.len(), 20);
    for (i, cd) in result.classes.iter().enumerate() {
        assert_eq!(cd.name, format!("Class{}", i));
    }
}

// ---- generate_bindings with proto ----

#[test]
fn test_bindings_with_mixed_property_kinds() {
    let cd = ClassDef {
        name: "Mixed".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "width".into(),
                kind: PropertyKind::Getter { fn_name: "get_w".into(), cache: true },
            },
            PropertyDef {
                name: "resize".into(),
                kind: PropertyKind::Method { fn_name: "do_resize".into(), length: 2 },
            },
            PropertyDef {
                name: "value".into(),
                kind: PropertyKind::Setter { fn_name: "set_v".into() },
            },
            PropertyDef {
                name: "data".into(),
                kind: PropertyKind::Accessor {
                    getter: "get_d".into(),
                    setter: "set_d".into(),
                    cache: false,
                },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.function_specs.len(), 1); // Method
    assert!(bindings.function_specs[0].contains("resize"));
    assert!(bindings.property_specs.len() >= 2); // Getter, Setter, Accessor
}

// ---- generate_all consistency ----

#[test]
fn test_generate_all_key_matches_name() {
    let cds = vec![
        ClassDef {
            name: "X".into(),
            construct: false, no_constructor: true, finalize: false,
            configurable: false, has_pending_activity: false,
            proto: vec![], static_props: vec![],
        },
        ClassDef {
            name: "Y".into(),
            construct: false, no_constructor: true, finalize: false,
            configurable: false, has_pending_activity: false,
            proto: vec![], static_props: vec![],
        },
    ];
    let all = generate_all(&cds);
    for (key, bindings) in &all {
        assert_eq!(key, &bindings.class_name);
    }
}
