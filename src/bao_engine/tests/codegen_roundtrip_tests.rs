// @trace TEST-ENG-008 [req:REQ-ENG-002] [level:unit]
// Codegen roundtrip tests: parse→generate→verify consistency,
// parse_classes roundtrip, generate_all batch, generate_module,
// PropertyKind variants, ClassDef fields, GeneratedBindings structure.

use bao_engine::codegen::*;

// ---- parse_classes roundtrip ----

#[test]
fn test_parse_roundtrip_single_class() {
    let source = r#"name: "FileSystem"
construct: true
finalize: true
proto:
  readFileSync(path): [Method]
  writeFileSync(path, data): [Method]"#;
    let result = parse_classes(source, "fs.classes.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    assert_eq!(result.classes[0].name, "FileSystem");
    assert!(result.classes[0].construct);
}

#[test]
fn test_parse_roundtrip_preserves_source_file() {
    let source = r#"name: "Foo""#;
    let result = parse_classes(source, "foo.classes.ts").unwrap();
    assert_eq!(result.source_file, "foo.classes.ts");
}

#[test]
fn test_parse_multiple_names() {
    let source = r#"name: "ClassA"
construct: true
name: "ClassB"
construct: true"#;
    let result = parse_classes(source, "multi.classes.ts").unwrap();
    assert_eq!(result.classes.len(), 2);
    assert_eq!(result.classes[0].name, "ClassA");
    assert_eq!(result.classes[1].name, "ClassB");
}

// ---- generate_bindings roundtrip ----

#[test]
fn test_generate_bindings_contains_class_name() {
    let class_def = ClassDef {
        name: "TestWidget".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.class_name, "TestWidget");
    assert!(bindings.init_class_fn.contains("TestWidget"));
}

#[test]
fn test_generate_bindings_method_in_specs() {
    let class_def = ClassDef {
        name: "MyClass".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "doSomething".into(),
            kind: PropertyKind::Method { fn_name: "do_something".into(), length: 0 },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.function_specs.iter().any(|s| s.contains("doSomething")));
}

#[test]
fn test_generate_bindings_getter_in_specs() {
    let class_def = ClassDef {
        name: "Props".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "length".into(),
            kind: PropertyKind::Getter { fn_name: "get_length".into(), cache: false },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.property_specs.iter().any(|s| s.contains("length")));
}

#[test]
fn test_generate_bindings_static_method() {
    let class_def = ClassDef {
        name: "Utils".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![PropertyDef {
            name: "create".into(),
            kind: PropertyKind::Method { fn_name: "create".into(), length: 1 },
        }],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.static_function_specs.iter().any(|s| s.contains("create")));
}

// ---- generate_all batch ----

#[test]
fn test_generate_all_multiple_classes() {
    let class_defs = vec![
        ClassDef {
            name: "Alpha".into(),
            construct: true, no_constructor: false, finalize: false,
            configurable: false, has_pending_activity: false,
            proto: vec![], static_props: vec![],
        },
        ClassDef {
            name: "Beta".into(),
            construct: true, no_constructor: false, finalize: false,
            configurable: false, has_pending_activity: false,
            proto: vec![], static_props: vec![],
        },
    ];
    let all = generate_all(&class_defs);
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("Alpha"));
    assert!(all.contains_key("Beta"));
}

#[test]
fn test_generate_all_empty() {
    let all = generate_all(&[]);
    assert!(all.is_empty());
}

#[test]
fn test_generate_all_single() {
    let class_defs = vec![ClassDef {
        name: "Solo".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    }];
    let all = generate_all(&class_defs);
    assert_eq!(all.len(), 1);
    assert!(all.contains_key("Solo"));
}

// ---- generate_module ----

#[test]
fn test_generate_module_contains_module_name() {
    let bindings = generate_bindings(&ClassDef {
        name: "Test".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    let module = generate_module(&[bindings], "test_module");
    assert!(module.contains("test_module"));
}

#[test]
fn test_generate_module_multiple_bindings() {
    let b1 = generate_bindings(&ClassDef {
        name: "A".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    let b2 = generate_bindings(&ClassDef {
        name: "B".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    let module = generate_module(&[b1, b2], "multi");
    assert!(module.contains("A"));
    assert!(module.contains("B"));
}

#[test]
fn test_generate_module_empty_bindings() {
    let module = generate_module(&[], "empty");
    assert!(!module.is_empty());
}

// ---- ClassDef field validation ----

#[test]
fn test_class_def_all_flags_false() {
    let def = ClassDef {
        name: "Plain".into(),
        construct: false, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    };
    assert!(!def.construct);
    assert!(!def.finalize);
    assert!(!def.configurable);
}

#[test]
fn test_class_def_all_flags_true() {
    let def = ClassDef {
        name: "Full".into(),
        construct: true, no_constructor: false, finalize: true,
        configurable: true, has_pending_activity: true,
        proto: vec![], static_props: vec![],
    };
    assert!(def.construct);
    assert!(def.finalize);
    assert!(def.configurable);
    assert!(def.has_pending_activity);
}

#[test]
fn test_class_def_clone() {
    let def = ClassDef {
        name: "Clonable".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "method".into(),
            kind: PropertyKind::Method { fn_name: "method".into(), length: 1 },
        }],
        static_props: vec![],
    };
    let cloned = def.clone();
    assert_eq!(cloned.name, "Clonable");
    assert_eq!(cloned.proto.len(), 1);
}

#[test]
fn test_class_def_debug() {
    let def = ClassDef {
        name: "Debug".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    };
    let debug = format!("{:?}", def);
    assert!(debug.contains("Debug"));
}

// ---- PropertyKind variants ----

#[test]
fn test_property_kind_getter() {
    let kind = PropertyKind::Getter { fn_name: "get_val".into(), cache: true };
    let debug = format!("{:?}", kind);
    assert!(debug.contains("get_val"));
}

#[test]
fn test_property_kind_setter() {
    let kind = PropertyKind::Setter { fn_name: "set_val".into() };
    let debug = format!("{:?}", kind);
    assert!(debug.contains("set_val"));
}

#[test]
fn test_property_kind_accessor() {
    let kind = PropertyKind::Accessor {
        getter: "get_x".into(),
        setter: "set_x".into(),
        cache: false,
    };
    let debug = format!("{:?}", kind);
    assert!(debug.contains("get_x"));
    assert!(debug.contains("set_x"));
}

#[test]
fn test_property_kind_method() {
    let kind = PropertyKind::Method { fn_name: "run".into(), length: 2 };
    let debug = format!("{:?}", kind);
    assert!(debug.contains("run"));
}

#[test]
fn test_property_kind_value() {
    let kind = PropertyKind::Value { value: "42".into() };
    let debug = format!("{:?}", kind);
    assert!(debug.contains("42"));
}

#[test]
fn test_property_kind_clone() {
    let kind = PropertyKind::Method { fn_name: "test".into(), length: 0 };
    let cloned = kind.clone();
    if let PropertyKind::Method { fn_name, length } = cloned {
        assert_eq!(fn_name, "test");
        assert_eq!(length, 0);
    } else {
        panic!("Expected Method variant");
    }
}

// ---- PropertyDef ----

#[test]
fn test_property_def_debug() {
    let def = PropertyDef {
        name: "prop".into(),
        kind: PropertyKind::Getter { fn_name: "get_prop".into(), cache: false },
    };
    let debug = format!("{:?}", def);
    assert!(debug.contains("prop"));
}

#[test]
fn test_property_def_clone() {
    let def = PropertyDef {
        name: "x".into(),
        kind: PropertyKind::Value { value: "1".into() },
    };
    let cloned = def.clone();
    assert_eq!(cloned.name, "x");
}

// ---- GeneratedBindings structure ----

#[test]
fn test_generated_bindings_debug() {
    let bindings = generate_bindings(&ClassDef {
        name: "Widget".into(),
        construct: true, no_constructor: false, finalize: true,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    let debug = format!("{:?}", bindings);
    assert!(debug.contains("Widget"));
}

#[test]
fn test_generated_bindings_finalize_when_requested() {
    let bindings = generate_bindings(&ClassDef {
        name: "Res".into(),
        construct: true, no_constructor: false, finalize: true,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    assert!(bindings.finalize_fn.is_some());
}

#[test]
fn test_generated_bindings_no_finalize_when_not_requested() {
    let bindings = generate_bindings(&ClassDef {
        name: "NoFin".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    assert!(bindings.finalize_fn.is_none());
}

#[test]
fn test_generated_bindings_constructor_when_construct() {
    let bindings = generate_bindings(&ClassDef {
        name: "Ctor".into(),
        construct: true, no_constructor: false, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    assert!(bindings.constructor_fn.is_some());
}

#[test]
fn test_generated_bindings_no_constructor() {
    let bindings = generate_bindings(&ClassDef {
        name: "NoCtor".into(),
        construct: false, no_constructor: true, finalize: false,
        configurable: false, has_pending_activity: false,
        proto: vec![], static_props: vec![],
    });
    assert!(bindings.constructor_fn.is_none());
}

// ---- Parse error cases ----

#[test]
fn test_parse_empty_source() {
    let result = parse_classes("", "empty.classes.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_whitespace_only() {
    let result = parse_classes("   \n\t  \n  ", "ws.classes.ts");
    assert!(result.is_ok());
}

// ---- Full roundtrip: parse → generate → module ----

#[test]
fn test_full_roundtrip_parse_generate_module() {
    // Build ClassDef manually (parser format is config-style, not TS)
    let class = ClassDef {
        name: "HTMLElement".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "getAttribute".into(),
                kind: PropertyKind::Method { fn_name: "get_attribute".into(), length: 1 },
            },
            PropertyDef {
                name: "setAttribute".into(),
                kind: PropertyKind::Method { fn_name: "set_attribute".into(), length: 2 },
            },
        ],
        static_props: vec![],
    };

    let bindings = generate_bindings(&class);
    assert_eq!(bindings.class_name, "HTMLElement");
    assert!(bindings.constructor_fn.is_some());
    assert!(bindings.finalize_fn.is_some());
    assert_eq!(bindings.function_specs.len(), 2);

    let module = generate_module(&[bindings], "html_bindings");
    assert!(module.contains("HTMLElement"));
    assert!(module.contains("html_bindings"));
}

#[test]
fn test_roundtrip_preserves_class_name_case() {
    let source = r#"name: "MyHTTPClient""#;
    let parsed = parse_classes(source, "case.classes.ts").unwrap();
    assert_eq!(parsed.classes[0].name, "MyHTTPClient");
    let bindings = generate_bindings(&parsed.classes[0]);
    assert!(bindings.init_class_fn.contains("MyHTTPClient"));
}
