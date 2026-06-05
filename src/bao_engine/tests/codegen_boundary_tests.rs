// @trace TEST-ENG-002-CODEGEN-BND [req:REQ-ENG-002] [level:unit]
// Codegen boundary tests: accessor parsing, generate_module output, collect_specs categorization,
// multi-class isolation, edge cases in extract helpers.

use bao_engine::codegen::*;

// ---- Accessor (getter + setter) parsing ----

#[test]
fn test_parse_accessor_getter_setter() {
    let src = r#"
name: "MyClass"
proto: {
  prop: {
    getter: "get_prop",
    setter: "set_prop",
    cache: true,
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    let props = &result.classes[0].proto;
    assert_eq!(props.len(), 1);
    match &props[0].kind {
        PropertyKind::Accessor { getter, setter, cache } => {
            assert_eq!(getter, "get_prop");
            assert_eq!(setter, "set_prop");
            assert!(*cache);
        }
        other => panic!("Expected Accessor, got {:?}", other),
    }
}

#[test]
fn test_parse_accessor_without_cache() {
    let src = r#"
name: "MyClass"
proto: {
  prop: {
    getter: "get_prop",
    setter: "set_prop",
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Accessor { cache, .. } => assert!(!*cache),
        other => panic!("Expected Accessor, got {:?}", other),
    }
}

#[test]
fn test_parse_setter_only() {
    let src = r#"
name: "MyClass"
proto: {
  prop: {
    setter: "set_prop",
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Setter { fn_name } => assert_eq!(fn_name, "set_prop"),
        other => panic!("Expected Setter, got {:?}", other),
    }
}

#[test]
fn test_parse_getter_only() {
    let src = r#"
name: "MyClass"
proto: {
  prop: {
    getter: "get_prop",
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "get_prop");
            assert!(!*cache);
        }
        other => panic!("Expected Getter, got {:?}", other),
    }
}

#[test]
fn test_parse_method_with_length() {
    let src = r#"
name: "MyClass"
proto: {
  doStuff: {
    fn: "my_do_stuff",
    length: 3,
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "my_do_stuff");
            assert_eq!(*length, 3);
        }
        other => panic!("Expected Method, got {:?}", other),
    }
}

#[test]
fn test_parse_value_property() {
    let src = r#"
name: "MyClass"
proto: {
  version: "1.0.0",
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Value { value } => assert!(value.starts_with("1.0.0")),
        other => panic!("Expected Value, got {:?}", other),
    }
}

#[test]
fn test_parse_value_single_quotes() {
    let src = r#"
name: "MyClass"
proto: {
  tag: 'beta',
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Value { value } => assert!(value.starts_with("beta")),
        other => panic!("Expected Value, got {:?}", other),
    }
}

// ---- klass (static props) parsing ----

#[test]
fn test_parse_klass_static_props() {
    let src = r#"
name: "MyClass"
klass: {
  staticMethod: {
    fn: "my_static_method",
    length: 2,
  },
  staticProp: {
    getter: "get_static_prop",
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    assert_eq!(result.classes[0].static_props.len(), 2);
}

#[test]
fn test_parse_both_proto_and_klass() {
    let src = r#"
name: "MyClass"
proto: {
  instanceMethod: {
    fn: "instance_fn",
    length: 0,
  },
}
klass: {
  staticMethod: {
    fn: "static_fn",
    length: 1,
  },
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    assert_eq!(result.classes[0].proto.len(), 1);
    assert_eq!(result.classes[0].static_props.len(), 1);
}

#[test]
fn test_parse_empty_proto_block() {
    let src = r#"
name: "MyClass"
proto: {
}
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    assert!(result.classes[0].proto.is_empty());
}

#[test]
fn test_parse_no_proto_or_klass() {
    let src = r#"
name: "MyClass"
"#;
    let result = parse_classes(src, "test.ts").unwrap();
    assert!(result.classes[0].proto.is_empty());
    assert!(result.classes[0].static_props.is_empty());
}

// ---- generate_bindings output validation ----

#[test]
fn test_generate_bindings_class_name_in_output() {
    let class_def = ClassDef {
        name: "TestWidget".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.class_name, "TestWidget");
    assert!(bindings.js_class_def.contains("TestWidget"));
    assert!(bindings.init_class_fn.contains("init_TestWidget"));
}

#[test]
fn test_generate_bindings_with_constructor() {
    let class_def = ClassDef {
        name: "WithCtor".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.constructor_fn.is_some());
    let ctor = bindings.constructor_fn.unwrap();
    assert!(ctor.contains("WithCtor_constructor"));
    assert!(ctor.contains("JS_NewObjectForConstructor"));
    assert!(ctor.contains("JS_SetReservedSlot"));
    assert!(ctor.contains("PrivateValue"));
}

#[test]
fn test_generate_bindings_no_constructor_when_flag_off() {
    let class_def = ClassDef {
        name: "NoCtor".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.constructor_fn.is_none());
}

#[test]
fn test_generate_bindings_no_constructor_when_noConstructor() {
    let class_def = ClassDef {
        name: "NoCtor2".into(),
        construct: true,
        no_constructor: true,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.constructor_fn.is_none());
}

#[test]
fn test_generate_bindings_with_finalize() {
    let class_def = ClassDef {
        name: "WithFinalize".into(),
        construct: false,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.finalize_fn.is_some());
    let fin = bindings.finalize_fn.unwrap();
    assert!(fin.contains("WithFinalize_finalize"));
}

#[test]
fn test_generate_bindings_with_method_generates_function_spec() {
    let class_def = ClassDef {
        name: "MethodClass".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "doIt".into(),
            kind: PropertyKind::Method { fn_name: "do_it_fn".into(), length: 2 },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.function_specs.len(), 1);
    assert!(bindings.function_specs[0].contains("doIt"));
    assert!(bindings.function_specs[0].contains("do_it_fn"));
    assert!(bindings.function_specs[0].contains("nargs: 2"));
}

#[test]
fn test_generate_bindings_with_getter_generates_property_spec() {
    let class_def = ClassDef {
        name: "GetterClass".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "value".into(),
            kind: PropertyKind::Getter { fn_name: "get_value".into(), cache: false },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.property_specs.len(), 1);
    assert!(bindings.property_specs[0].contains("get_value"));
}

#[test]
fn test_generate_bindings_with_accessor_generates_both() {
    let class_def = ClassDef {
        name: "AccessorClass".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "data".into(),
            kind: PropertyKind::Accessor {
                getter: "get_data".into(),
                setter: "set_data".into(),
                cache: true,
            },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.property_specs.len(), 1);
    let spec = &bindings.property_specs[0];
    assert!(spec.contains("get_data"));
    assert!(spec.contains("set_data"));
}

#[test]
fn test_generate_bindings_value_not_in_specs() {
    let class_def = ClassDef {
        name: "ValueClass".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "version".into(),
            kind: PropertyKind::Value { value: "1.0".into() },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.function_specs.is_empty());
    assert!(bindings.property_specs.is_empty());
}

// ---- generate_module output ----

#[test]
fn test_generate_module_contains_header() {
    let bindings = generate_bindings(&ClassDef {
        name: "Mod".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    });
    let module = generate_module(&[bindings], "test_module");
    assert!(module.contains("@trace REQ-ENG-002"));
    assert!(module.contains("test_module"));
    assert!(module.contains("use mozjs"));
}

#[test]
fn test_generate_module_contains_init_fn() {
    let bindings = generate_bindings(&ClassDef {
        name: "Init".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    });
    let module = generate_module(&[bindings], "mod");
    assert!(module.contains("init_Init"));
    assert!(module.contains("JS_InitClass"));
}

#[test]
fn test_generate_module_multiple_classes() {
    let b1 = generate_bindings(&ClassDef {
        name: "ClassA".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    });
    let b2 = generate_bindings(&ClassDef {
        name: "ClassB".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    });
    let module = generate_module(&[b1, b2], "multi");
    assert!(module.contains("ClassA"));
    assert!(module.contains("ClassB"));
    assert!(module.contains("ClassB_constructor"));
    assert!(module.contains("ClassB_finalize"));
}

// ---- generate_all batch ----

#[test]
fn test_generate_all_returns_map() {
    let classes = vec![
        ClassDef {
            name: "A".into(),
            construct: false,
            no_constructor: false,
            finalize: false,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
        ClassDef {
            name: "B".into(),
            construct: true,
            no_constructor: false,
            finalize: false,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
    ];
    let map = generate_all(&classes);
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("A"));
    assert!(map.contains_key("B"));
    assert!(map["B"].constructor_fn.is_some());
}

#[test]
fn test_generate_all_empty_input() {
    let map = generate_all(&[]);
    assert!(map.is_empty());
}

// ---- ParseResult ----

#[test]
fn test_parse_result_source_file() {
    let src = r#"name: "Test""#;
    let result = parse_classes(src, "my_file.ts").unwrap();
    assert_eq!(result.source_file, "my_file.ts");
}

#[test]
fn test_parse_result_empty_source() {
    let result = parse_classes("", "empty.ts").unwrap();
    assert!(result.classes.is_empty());
}

// ---- PropertyKind Clone/Debug ----

#[test]
fn test_property_kind_clone() {
    let kind = PropertyKind::Method { fn_name: "test".into(), length: 3 };
    let cloned = kind.clone();
    match cloned {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "test");
            assert_eq!(length, 3);
        }
        _ => panic!("Expected Method"),
    }
}

#[test]
fn test_property_kind_debug_all_variants() {
    let variants = vec![
        format!("{:?}", PropertyKind::Getter { fn_name: "g".into(), cache: false }),
        format!("{:?}", PropertyKind::Setter { fn_name: "s".into() }),
        format!("{:?}", PropertyKind::Accessor { getter: "g".into(), setter: "s".into(), cache: true }),
        format!("{:?}", PropertyKind::Method { fn_name: "m".into(), length: 0 }),
        format!("{:?}", PropertyKind::Value { value: "v".into() }),
    ];
    assert!(variants[0].contains("Getter"));
    assert!(variants[1].contains("Setter"));
    assert!(variants[2].contains("Accessor"));
    assert!(variants[3].contains("Method"));
    assert!(variants[4].contains("Value"));
}

// ---- ClassDef flags from separate source strings ----

#[test]
fn test_class_def_flags_isolated() {
    // Each class in its own source to avoid source-wide flag contamination
    let src_construct = r#"
name: "WithConstruct"
construct: true
"#;
    let src_no_construct = r#"
name: "NoConstruct"
"#;
    let r1 = parse_classes(src_construct, "a.ts").unwrap();
    let r2 = parse_classes(src_no_construct, "b.ts").unwrap();
    assert!(r1.classes[0].construct);
    assert!(!r2.classes[0].construct);
}

#[test]
fn test_class_def_finalize_flag() {
    let src = r#"
name: "Fin"
finalize: true
"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(result.classes[0].finalize);
}

#[test]
fn test_class_def_configurable_default_true() {
    let src = r#"name: "Conf""#;
    let result = parse_classes(src, "c.ts").unwrap();
    assert!(result.classes[0].configurable);
}

#[test]
fn test_class_def_configurable_false() {
    let src = r#"
name: "Conf"
configurable: false
"#;
    let result = parse_classes(src, "c.ts").unwrap();
    assert!(!result.classes[0].configurable);
}

#[test]
fn test_class_def_has_pending_activity() {
    let src = r#"
name: "Activity"
hasPendingActivity: true
"#;
    let result = parse_classes(src, "a.ts").unwrap();
    assert!(result.classes[0].has_pending_activity);
}
