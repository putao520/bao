// @trace TEST-ENG-009-CODEGEN-EDGE [req:REQ-ENG-002] [level:unit]
// Codegen edge cases: empty input, malformed input, property type coverage,
// generate_all multiple classes, generate_module output format, boundary conditions.

use bao_engine::codegen::*;
use std::collections::HashMap;

// ---- parse_classes edge cases ----

#[test]
fn test_parse_empty_source() {
    let result = parse_classes("", "empty.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_whitespace_only() {
    let result = parse_classes("   \n  \t  \n  ", "ws.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_no_name_field() {
    let src = r#"construct: true
finalize: true"#;
    let result = parse_classes(src, "noname.ts");
    assert!(result.is_ok());
    assert!(result.unwrap().classes.is_empty());
}

#[test]
fn test_parse_name_with_no_properties() {
    let src = r#"name: "EmptyClass""#;
    let result = parse_classes(src, "empty_class.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    assert_eq!(result.classes[0].name, "EmptyClass");
    assert!(result.classes[0].proto.is_empty());
    assert!(result.classes[0].static_props.is_empty());
}

#[test]
fn test_parse_multiple_names() {
    let src = r#"name: "ClassA"
name: "ClassB"
name: "ClassC""#;
    let result = parse_classes(src, "multi.ts").unwrap();
    assert_eq!(result.classes.len(), 3);
    assert_eq!(result.classes[0].name, "ClassA");
    assert_eq!(result.classes[1].name, "ClassB");
    assert_eq!(result.classes[2].name, "ClassC");
}

#[test]
fn test_parse_preserves_source_file() {
    let result = parse_classes("", "my/module.ts").unwrap();
    assert_eq!(result.source_file, "my/module.ts");
}

#[test]
fn test_parse_construct_flags() {
    let src = r#"name: "Foo"
construct: true"#;
    let result = parse_classes(src, "f.ts").unwrap();
    // construct: true appears in source, so construct flag should be true
    assert!(result.classes[0].construct);
}

#[test]
fn test_parse_no_constructor_flag() {
    let src = r#"name: "Foo"
noConstructor: true"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(result.classes[0].no_constructor);
}

#[test]
fn test_parse_finalize_flag() {
    let src = r#"name: "Foo"
finalize: true"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(result.classes[0].finalize);
}

#[test]
fn test_parse_configurable_default() {
    let src = r#"name: "Foo""#;
    let result = parse_classes(src, "f.ts").unwrap();
    // Default is configurable = true (only false if configurable: false present)
    assert!(result.classes[0].configurable);
}

#[test]
fn test_parse_configurable_false() {
    let src = r#"name: "Foo"
configurable: false"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(!result.classes[0].configurable);
}

#[test]
fn test_parse_has_pending_activity() {
    let src = r#"name: "Foo"
hasPendingActivity: true"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(result.classes[0].has_pending_activity);
}

// ---- Property parsing ----

#[test]
fn test_parse_getter_property() {
    let src = r#"name: "Foo"
proto: {
  bar: {
    getter: "getBar"
    cache: true
  }
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert_eq!(result.classes[0].proto.len(), 1);
    let prop = &result.classes[0].proto[0];
    assert_eq!(prop.name, "bar");
    match &prop.kind {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "getBar");
            assert!(*cache);
        }
        _ => panic!("Expected Getter, got {:?}", prop.kind),
    }
}

#[test]
fn test_parse_setter_property() {
    let src = r#"name: "Foo"
proto: {
  bar: {
    setter: "setBar"
  }
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert_eq!(result.classes[0].proto.len(), 1);
    match &result.classes[0].proto[0].kind {
        PropertyKind::Setter { fn_name } => assert_eq!(fn_name, "setBar"),
        _ => panic!("Expected Setter"),
    }
}

#[test]
fn test_parse_accessor_property() {
    let src = r#"name: "Foo"
proto: {
  bar: {
    getter: "getBar"
    setter: "setBar"
    cache: false
  }
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Accessor { getter, setter, cache } => {
            assert_eq!(getter, "getBar");
            assert_eq!(setter, "setBar");
            assert!(!*cache);
        }
        _ => panic!("Expected Accessor"),
    }
}

#[test]
fn test_parse_method_property() {
    let src = r#"name: "Foo"
proto: {
  doSomething: {
    fn: "do_something"
    length: 2
  }
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "do_something");
            assert_eq!(*length, 2);
        }
        _ => panic!("Expected Method"),
    }
}

#[test]
fn test_parse_value_property() {
    let src = r#"name: "Foo"
klass: {
  version: "1.0.0"
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert_eq!(result.classes[0].static_props.len(), 1);
    match &result.classes[0].static_props[0].kind {
        PropertyKind::Value { value } => assert_eq!(value, "1.0.0"),
        _ => panic!("Expected Value"),
    }
}

#[test]
fn test_parse_value_property_single_quotes() {
    let src = r#"name: "Foo"
klass: {
  tag: 'beta'
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    match &result.classes[0].static_props[0].kind {
        PropertyKind::Value { value } => assert_eq!(value, "beta"),
        _ => panic!("Expected Value"),
    }
}

#[test]
fn test_parse_multiple_proto_properties() {
    let src = r#"name: "Foo"
proto: {
  getter1: {
    getter: "getG1"
    cache: true
  }
  method1: {
    fn: "do_m1"
    length: 0
  }
  setter1: {
    setter: "setS1"
  }
}"#;
    let result = parse_classes(src, "f.ts").unwrap();
    assert!(result.classes[0].proto.len() >= 2);
}

// ---- generate_bindings ----

#[test]
fn test_generate_bindings_basic_class() {
    let class_def = ClassDef {
        name: "TestObj".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert_eq!(bindings.class_name, "TestObj");
    assert!(bindings.js_class_def.contains("TestObj"));
    assert!(bindings.constructor_fn.is_some());
    assert!(bindings.finalize_fn.is_some());
    assert!(!bindings.init_class_fn.is_empty());
}

#[test]
fn test_generate_bindings_no_construct() {
    let class_def = ClassDef {
        name: "NoCtor".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.constructor_fn.is_none());
    assert!(bindings.finalize_fn.is_none());
}

#[test]
fn test_generate_bindings_with_proto_methods() {
    let class_def = ClassDef {
        name: "WithMethods".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "calc".into(),
                kind: PropertyKind::Method { fn_name: "calc_impl".into(), length: 1 },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(!bindings.function_specs.is_empty() || !bindings.property_specs.is_empty());
}

#[test]
fn test_generate_bindings_with_static_props() {
    let class_def = ClassDef {
        name: "WithStatic".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![
            PropertyDef {
                name: "VERSION".into(),
                kind: PropertyKind::Value { value: "2.0".into() },
            },
        ],
    };
    let bindings = generate_bindings(&class_def);
    // Value-type properties are not emitted as specs by collect_specs
    // They are parsed but currently skipped in code generation
    assert_eq!(bindings.static_function_specs.len(), 0);
    assert_eq!(bindings.static_property_specs.len(), 0);
}

#[test]
fn test_generate_bindings_init_fn_contains_class_name() {
    let class_def = ClassDef {
        name: "MyWidget".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.init_class_fn.contains("init_MyWidget"));
    assert!(bindings.init_class_fn.contains("JS_InitClass"));
}

#[test]
fn test_generate_bindings_js_class_def_contains_static() {
    let class_def = ClassDef {
        name: "X".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.js_class_def.contains("static"));
    assert!(bindings.js_class_def.contains("JSClass"));
}

#[test]
fn test_generate_bindings_accessor_property() {
    let class_def = ClassDef {
        name: "Acc".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![PropertyDef {
            name: "value".into(),
            kind: PropertyKind::Accessor {
                getter: "get_value".into(),
                setter: "set_value".into(),
                cache: true,
            },
        }],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    assert!(bindings.property_specs.len() >= 1);
}

// ---- generate_all ----

#[test]
fn test_generate_all_empty() {
    let result: HashMap<String, GeneratedBindings> = generate_all(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_generate_all_single_class() {
    let defs = vec![ClassDef {
        name: "Single".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    }];
    let result = generate_all(&defs);
    assert_eq!(result.len(), 1);
    assert!(result.contains_key("Single"));
}

#[test]
fn test_generate_all_multiple_classes() {
    let defs: Vec<ClassDef> = (0..5)
        .map(|i| ClassDef {
            name: format!("Class{}", i),
            construct: i % 2 == 0,
            no_constructor: i % 2 == 1,
            finalize: i % 3 == 0,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        })
        .collect();
    let result = generate_all(&defs);
    assert_eq!(result.len(), 5);
    for i in 0..5 {
        assert!(result.contains_key(&format!("Class{}", i)));
    }
}

// ---- generate_module ----

#[test]
fn test_generate_module_empty() {
    let output = generate_module(&[], "empty_mod");
    assert!(output.contains("empty_mod"));
}

#[test]
fn test_generate_module_single_binding() {
    let class_def = ClassDef {
        name: "Widget".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class_def);
    let output = generate_module(&[bindings], "widget_mod");
    assert!(output.contains("widget_mod"));
    assert!(output.contains("Widget"));
}

#[test]
fn test_generate_module_multiple_bindings() {
    let bindings: Vec<GeneratedBindings> = (0..3)
        .map(|i| {
            generate_bindings(&ClassDef {
                name: format!("Mod{}", i),
                construct: true,
                no_constructor: false,
                finalize: false,
                configurable: true,
                has_pending_activity: false,
                proto: vec![],
                static_props: vec![],
            })
        })
        .collect();
    let output = generate_module(&bindings, "multi_mod");
    assert!(output.contains("Mod0"));
    assert!(output.contains("Mod1"));
    assert!(output.contains("Mod2"));
}

// ---- PropertyKind Debug/Clone ----

#[test]
fn test_property_kind_getter_clone() {
    let pk = PropertyKind::Getter { fn_name: "get_x".into(), cache: true };
    let cloned = pk.clone();
    match cloned {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "get_x");
            assert!(cache);
        }
        _ => panic!("Expected Getter"),
    }
}

#[test]
fn test_property_kind_method_clone() {
    let pk = PropertyKind::Method { fn_name: "do_it".into(), length: 3 };
    let cloned = pk.clone();
    match cloned {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "do_it");
            assert_eq!(length, 3);
        }
        _ => panic!("Expected Method"),
    }
}

#[test]
fn test_property_kind_accessor_debug() {
    let pk = PropertyKind::Accessor { getter: "g".into(), setter: "s".into(), cache: false };
    let debug = format!("{:?}", pk);
    assert!(debug.contains("Accessor"));
}

#[test]
fn test_class_def_clone() {
    let cd = ClassDef {
        name: "Clone".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: true,
        proto: vec![PropertyDef {
            name: "x".into(),
            kind: PropertyKind::Getter { fn_name: "get_x".into(), cache: false },
        }],
        static_props: vec![],
    };
    let cloned = cd.clone();
    assert_eq!(cloned.name, "Clone");
    assert_eq!(cloned.proto.len(), 1);
    assert!(cloned.has_pending_activity);
}

#[test]
fn test_generated_bindings_debug() {
    let bindings = generate_bindings(&ClassDef {
        name: "Debug".into(),
        construct: false,
        no_constructor: true,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    });
    let debug = format!("{:?}", bindings);
    assert!(debug.contains("Debug"));
    assert!(debug.contains("class_name"));
}

// ---- Full roundtrip: parse → generate → module ----

#[test]
fn test_full_roundtrip_simple_class() {
    let src = r#"name: "Buffer"
construct: true
finalize: true
proto: {
  length: {
    getter: "get_length"
    cache: true
  }
  toString: {
    fn: "to_string"
    length: 0
  }
}"#;
    let parsed = parse_classes(src, "buffer.ts").unwrap();
    assert_eq!(parsed.classes.len(), 1);
    let class = &parsed.classes[0];
    assert!(class.construct);
    assert!(class.finalize);

    let bindings = generate_bindings(class);
    assert_eq!(bindings.class_name, "Buffer");
    assert!(bindings.constructor_fn.is_some());
    assert!(bindings.finalize_fn.is_some());

    let module = generate_module(&[bindings], "buffer_module");
    assert!(module.contains("Buffer"));
    assert!(module.contains("buffer_module"));
}

#[test]
fn test_full_roundtrip_class_with_accessor() {
    let src = r#"name: "Stream"
construct: true
proto: {
  data: {
    getter: "get_data"
    setter: "set_data"
    cache: false
  }
}"#;
    let parsed = parse_classes(src, "stream.ts").unwrap();
    let class = &parsed.classes[0];
    assert_eq!(class.proto.len(), 1);
    match &class.proto[0].kind {
        PropertyKind::Accessor { getter, setter, cache } => {
            assert_eq!(getter, "get_data");
            assert_eq!(setter, "set_data");
            assert!(!*cache);
        }
        _ => panic!("Expected Accessor"),
    }
}

#[test]
fn test_full_roundtrip_static_value() {
    let src = r#"name: "Config"
noConstructor: true
klass: {
  VERSION: "3.0.0"
  NAME: 'Bao'
}"#;
    let parsed = parse_classes(src, "config.ts").unwrap();
    let class = &parsed.classes[0];
    assert!(class.no_constructor);
    assert!(class.static_props.len() >= 1);

    let bindings = generate_bindings(class);
    assert!(bindings.constructor_fn.is_none());
}
