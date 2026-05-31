// @trace TEST-ENG-016 [req:REQ-ENG-002] [level:unit]
// Codegen generate_all batch, generate_module output, GeneratedBindings field completeness,
// PropertyKind variants, ClassDef edge cases, multi-class scenarios.

use bao_engine::codegen::{
    ClassDef, PropertyDef, PropertyKind, GeneratedBindings,
    parse_classes, generate_bindings, generate_all, generate_module,
};

// ---- PropertyKind variants ----

#[test]
fn test_property_kind_getter() {
    let pk = PropertyKind::Getter { fn_name: "get_foo".into(), cache: false };
    if let PropertyKind::Getter { fn_name, cache } = pk {
        assert_eq!(fn_name, "get_foo");
        assert!(!cache);
    } else {
        panic!("Expected Getter");
    }
}

#[test]
fn test_property_kind_setter() {
    let pk = PropertyKind::Setter { fn_name: "set_bar".into() };
    if let PropertyKind::Setter { fn_name } = pk {
        assert_eq!(fn_name, "set_bar");
    } else {
        panic!("Expected Setter");
    }
}

#[test]
fn test_property_kind_accessor() {
    let pk = PropertyKind::Accessor {
        getter: "get_x".into(),
        setter: "set_x".into(),
        cache: true,
    };
    if let PropertyKind::Accessor { getter, setter, cache } = pk {
        assert_eq!(getter, "get_x");
        assert_eq!(setter, "set_x");
        assert!(cache);
    } else {
        panic!("Expected Accessor");
    }
}

#[test]
fn test_property_kind_method() {
    let pk = PropertyKind::Method { fn_name: "do_it".into(), length: 2 };
    if let PropertyKind::Method { fn_name, length } = pk {
        assert_eq!(fn_name, "do_it");
        assert_eq!(length, 2);
    } else {
        panic!("Expected Method");
    }
}

#[test]
fn test_property_kind_value() {
    let pk = PropertyKind::Value { value: "42".into() };
    if let PropertyKind::Value { value } = pk {
        assert_eq!(value, "42");
    } else {
        panic!("Expected Value");
    }
}

#[test]
fn test_property_kind_debug() {
    let pk = PropertyKind::Method { fn_name: "test".into(), length: 0 };
    let debug = format!("{:?}", pk);
    assert!(debug.contains("Method"));
}

#[test]
fn test_property_kind_clone() {
    let pk = PropertyKind::Getter { fn_name: "g".into(), cache: true };
    let cloned = pk.clone();
    if let PropertyKind::Getter { fn_name, cache } = cloned {
        assert_eq!(fn_name, "g");
        assert!(cache);
    } else {
        panic!("Expected Getter");
    }
}

// ---- PropertyDef ----

#[test]
fn test_property_def_fields() {
    let pd = PropertyDef {
        name: "url".into(),
        kind: PropertyKind::Getter { fn_name: "get_url".into(), cache: false },
    };
    assert_eq!(pd.name, "url");
}

#[test]
fn test_property_def_debug() {
    let pd = PropertyDef {
        name: "test".into(),
        kind: PropertyKind::Value { value: "1".into() },
    };
    let debug = format!("{:?}", pd);
    assert!(debug.contains("test"));
}

#[test]
fn test_property_def_clone() {
    let pd = PropertyDef {
        name: "x".into(),
        kind: PropertyKind::Method { fn_name: "get_x".into(), length: 1 },
    };
    let cloned = pd.clone();
    assert_eq!(cloned.name, "x");
}

// ---- ClassDef ----

#[test]
fn test_class_def_minimal() {
    let cd = ClassDef {
        name: "Simple".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    assert_eq!(cd.name, "Simple");
    assert!(cd.proto.is_empty());
    assert!(cd.static_props.is_empty());
    assert!(cd.no_constructor);
    assert!(!cd.construct);
}

#[test]
fn test_class_def_with_proto() {
    let cd = ClassDef {
        name: "Resource".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "url".into(),
                kind: PropertyKind::Getter { fn_name: "get_url".into(), cache: false },
            },
            PropertyDef {
                name: "read".into(),
                kind: PropertyKind::Method { fn_name: "read".into(), length: 0 },
            },
        ],
        static_props: vec![],
    };
    assert_eq!(cd.proto.len(), 2);
}

#[test]
fn test_class_def_debug() {
    let cd = ClassDef {
        name: "Test".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let debug = format!("{:?}", cd);
    assert!(debug.contains("Test"));
}

#[test]
fn test_class_def_clone() {
    let cd = ClassDef {
        name: "CloneMe".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: true,
        proto: vec![PropertyDef {
            name: "x".into(),
            kind: PropertyKind::Value { value: "1".into() },
        }],
        static_props: vec![],
    };
    let cloned = cd.clone();
    assert_eq!(cloned.name, "CloneMe");
    assert_eq!(cloned.proto.len(), 1);
    assert!(cloned.has_pending_activity);
}

// ---- generate_bindings ----

#[test]
fn test_generate_bindings_no_constructor() {
    let cd = ClassDef {
        name: "NoCtor".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.class_name, "NoCtor");
    assert!(bindings.constructor_fn.is_none());
    assert!(bindings.finalize_fn.is_none());
}

#[test]
fn test_generate_bindings_with_constructor() {
    let cd = ClassDef {
        name: "WithCtor".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.constructor_fn.is_some());
    assert!(bindings.finalize_fn.is_some());
    let ctor = bindings.constructor_fn.unwrap();
    assert!(ctor.contains("WithCtor_constructor"));
    let fin = bindings.finalize_fn.unwrap();
    assert!(fin.contains("WithCtor_finalize"));
}

#[test]
fn test_generate_bindings_construct_no_constructor_flag() {
    // construct=true but no_constructor=true → no constructor generated
    let cd = ClassDef {
        name: "Both".into(),
        construct: true,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.constructor_fn.is_none());
}

#[test]
fn test_generate_bindings_js_class_def() {
    let cd = ClassDef {
        name: "MyClass".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.js_class_def.contains("MyClass"));
    assert!(bindings.js_class_def.contains("JSClass"));
}

#[test]
fn test_generate_bindings_init_fn() {
    let cd = ClassDef {
        name: "Foo".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.init_class_fn.contains("init_Foo"));
    assert!(bindings.init_class_fn.contains("JS_InitClass"));
}

#[test]
fn test_generate_bindings_with_methods() {
    let cd = ClassDef {
        name: "Api".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "fetch".into(),
                kind: PropertyKind::Method { fn_name: "api_fetch".into(), length: 1 },
            },
            PropertyDef {
                name: "close".into(),
                kind: PropertyKind::Method { fn_name: "api_close".into(), length: 0 },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.function_specs.len(), 2);
    assert!(bindings.function_specs[0].contains("fetch"));
    assert!(bindings.function_specs[1].contains("close"));
}

#[test]
fn test_generate_bindings_with_getters() {
    let cd = ClassDef {
        name: "Props".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "width".into(),
                kind: PropertyKind::Getter { fn_name: "get_width".into(), cache: true },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.property_specs.len(), 1);
    assert!(bindings.property_specs[0].contains("width"));
    assert!(bindings.property_specs[0].contains("get_width"));
}

#[test]
fn test_generate_bindings_with_setters() {
    let cd = ClassDef {
        name: "Mutable".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "value".into(),
                kind: PropertyKind::Setter { fn_name: "set_value".into() },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.property_specs.len(), 1);
    assert!(bindings.property_specs[0].contains("set_value"));
}

#[test]
fn test_generate_bindings_with_accessors() {
    let cd = ClassDef {
        name: "RW".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "data".into(),
                kind: PropertyKind::Accessor {
                    getter: "get_data".into(),
                    setter: "set_data".into(),
                    cache: false,
                },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert_eq!(bindings.property_specs.len(), 1);
    assert!(bindings.property_specs[0].contains("get_data"));
    assert!(bindings.property_specs[0].contains("set_data"));
}

#[test]
fn test_generate_bindings_static_props() {
    let cd = ClassDef {
        name: "StaticTest".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![
            PropertyDef {
                name: "version".into(),
                kind: PropertyKind::Method { fn_name: "get_version".into(), length: 0 },
            },
        ],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.function_specs.is_empty());
    assert_eq!(bindings.static_function_specs.len(), 1);
    assert!(bindings.static_function_specs[0].contains("version"));
}

#[test]
fn test_generate_bindings_value_kind_no_specs() {
    // PropertyKind::Value doesn't generate specs (handled in _ => {} branch)
    let cd = ClassDef {
        name: "ValOnly".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "constant".into(),
                kind: PropertyKind::Value { value: "42".into() },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    assert!(bindings.function_specs.is_empty());
    assert!(bindings.property_specs.is_empty());
}

// ---- generate_all ----

#[test]
fn test_generate_all_empty() {
    let result = generate_all(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_generate_all_single() {
    let cds = vec![ClassDef {
        name: "Single".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    }];
    let result = generate_all(&cds);
    assert_eq!(result.len(), 1);
    assert!(result.contains_key("Single"));
}

#[test]
fn test_generate_all_multiple() {
    let cds = vec![
        ClassDef {
            name: "Alpha".into(),
            construct: true,
            no_constructor: false,
            finalize: true,
            configurable: false,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
        ClassDef {
            name: "Beta".into(),
            construct: false,
            no_constructor: true,
            finalize: false,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
        ClassDef {
            name: "Gamma".into(),
            construct: true,
            no_constructor: true,
            finalize: true,
            configurable: false,
            has_pending_activity: true,
            proto: vec![],
            static_props: vec![],
        },
    ];
    let result = generate_all(&cds);
    assert_eq!(result.len(), 3);
    assert!(result.contains_key("Alpha"));
    assert!(result.contains_key("Beta"));
    assert!(result.contains_key("Gamma"));

    // Alpha has constructor and finalize
    let alpha = &result["Alpha"];
    assert!(alpha.constructor_fn.is_some());
    assert!(alpha.finalize_fn.is_some());

    // Beta has neither
    let beta = &result["Beta"];
    assert!(beta.constructor_fn.is_none());
    assert!(beta.finalize_fn.is_none());

    // Gamma: construct=true but no_constructor=true → no constructor
    let gamma = &result["Gamma"];
    assert!(gamma.constructor_fn.is_none());
    assert!(gamma.finalize_fn.is_some());
}

// ---- generate_module ----

#[test]
fn test_generate_module_empty() {
    let output = generate_module(&[], "empty_mod");
    assert!(output.contains("empty_mod"));
    assert!(output.contains("init_all"));
}

#[test]
fn test_generate_module_single_class() {
    let cd = ClassDef {
        name: "MyClass".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    let output = generate_module(&[bindings], "my_module");
    assert!(output.contains("my_module"));
    assert!(output.contains("MyClass"));
    assert!(output.contains("init_MyClass"));
    assert!(output.contains("JSClass"));
}

#[test]
fn test_generate_module_multi_class() {
    let cds = vec![
        ClassDef {
            name: "Reader".into(),
            construct: true,
            no_constructor: false,
            finalize: true,
            configurable: false,
            has_pending_activity: false,
            proto: vec![PropertyDef {
                name: "read".into(),
                kind: PropertyKind::Method { fn_name: "reader_read".into(), length: 1 },
            }],
            static_props: vec![],
        },
        ClassDef {
            name: "Writer".into(),
            construct: true,
            no_constructor: false,
            finalize: true,
            configurable: false,
            has_pending_activity: false,
            proto: vec![PropertyDef {
                name: "write".into(),
                kind: PropertyKind::Method { fn_name: "writer_write".into(), length: 2 },
            }],
            static_props: vec![],
        },
    ];
    let all = generate_all(&cds);
    let bindings: Vec<GeneratedBindings> = all.into_values().collect();
    let output = generate_module(&bindings, "io_module");

    assert!(output.contains("Reader"));
    assert!(output.contains("Writer"));
    assert!(output.contains("reader_read"));
    assert!(output.contains("writer_write"));
    assert!(output.contains("init_Reader"));
    assert!(output.contains("init_Writer"));
    assert!(output.contains("init_all"));
}

#[test]
fn test_generate_module_with_static_specs() {
    let cd = ClassDef {
        name: "Factory".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![
            PropertyDef {
                name: "create".into(),
                kind: PropertyKind::Method { fn_name: "factory_create".into(), length: 1 },
            },
        ],
    };
    let bindings = generate_bindings(&cd);
    let output = generate_module(&[bindings], "factory_mod");
    assert!(output.contains("Factory_static_specs"));
    assert!(output.contains("factory_create"));
}

// ---- GeneratedBindings struct ----

#[test]
fn test_generated_bindings_debug() {
    let cd = ClassDef {
        name: "DebugTest".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: false,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&cd);
    let debug = format!("{:?}", bindings);
    assert!(debug.contains("DebugTest"));
}

// ---- parse_classes edge cases ----

#[test]
fn test_parse_empty_source() {
    let result = parse_classes("", "empty.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_no_classes() {
    let result = parse_classes("// just a comment\n", "comment.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_single_class() {
    let source = r#"
name: "Buffer",
construct: true,
finalize: true,
configurable: false,
"#;
    let result = parse_classes(source, "buffer.ts");
    assert!(result.is_ok());
    let pr = result.unwrap();
    assert_eq!(pr.source_file, "buffer.ts");
    assert!(!pr.classes.is_empty());
}

// ---- Roundtrip: parse → generate ----

#[test]
fn test_parse_then_generate() {
    let source = r#"
name: "HttpClient",
construct: true,
finalize: true,
configurable: false,
"#;
    let pr = parse_classes(source, "http.ts").unwrap();
    assert!(!pr.classes.is_empty());
    let cd = &pr.classes[0];
    let bindings = generate_bindings(cd);
    assert_eq!(bindings.class_name, "HttpClient");
    assert!(bindings.constructor_fn.is_some());
    assert!(bindings.finalize_fn.is_some());
}

#[test]
fn test_parse_generate_all_roundtrip() {
    let source = r#"
name: "Stream",
construct: true,
finalize: true,
configurable: false,
"#;
    let pr = parse_classes(source, "stream.ts").unwrap();
    let all = generate_all(&pr.classes);
    assert!(!all.is_empty());
    for (name, bindings) in &all {
        assert_eq!(name, &bindings.class_name);
    }
}

#[test]
fn test_parse_result_debug() {
    let pr = parse_classes("", "x.ts").unwrap();
    let debug = format!("{:?}", pr);
    assert!(debug.contains("x.ts"));
}
