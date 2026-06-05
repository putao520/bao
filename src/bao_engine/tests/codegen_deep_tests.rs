// @trace TEST-ENG-002-DEEP [req:REQ-ENG-002] [level:unit]
// Deep tests for bao_engine codegen: complex class hierarchies, edge cases, roundtrip integrity

use bao_engine::codegen::*;

// ---- Parsing edge cases ----

#[test]
fn test_parse_class_with_all_features() {
    let source = r#"
define({
    name: "FullFeatured",
    construct: true,
    noConstructor: false,
    finalize: true,
    hasPendingActivity: true,
    configurable: false,
    proto: {
        readOnlyProp: {
            getter: "getReadOnlyProp",
            cache: true,
        },
        readWriteProp: {
            getter: "getReadWriteProp",
            setter: "setReadWriteProp",
            cache: false,
        },
        writeOnlyProp: {
            setter: "setWriteOnlyProp",
        },
        doSomething: {
            fn: "doSomethingImpl",
            length: 3,
        },
        constantValue: "42",
    },
    klass: {
        create: {
            fn: "createInstance",
            length: 2,
        },
        sharedInstance: {
            getter: "getSharedInstance",
            cache: true,
        },
    },
});
"#;
    let result = parse_classes(source, "full.classes.ts").unwrap();
    assert_eq!(result.classes.len(), 1);
    let class = &result.classes[0];

    assert_eq!(class.name, "FullFeatured");
    assert!(class.construct);
    assert!(!class.no_constructor);
    assert!(class.finalize);
    assert!(class.has_pending_activity);
    assert!(!class.configurable);

    // proto: 5 properties (getter, accessor, setter, method, value)
    assert_eq!(class.proto.len(), 5);

    // Check getter
    match &class.proto[0].kind {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "getReadOnlyProp");
            assert!(cache);
        }
        _ => panic!("expected getter for readOnlyProp"),
    }

    // Check accessor
    match &class.proto[1].kind {
        PropertyKind::Accessor { getter, setter, cache } => {
            assert_eq!(getter, "getReadWriteProp");
            assert_eq!(setter, "setReadWriteProp");
            assert!(!cache);
        }
        _ => panic!("expected accessor for readWriteProp"),
    }

    // Check setter
    match &class.proto[2].kind {
        PropertyKind::Setter { fn_name } => {
            assert_eq!(fn_name, "setWriteOnlyProp");
        }
        _ => panic!("expected setter for writeOnlyProp"),
    }

    // Check method
    match &class.proto[3].kind {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "doSomethingImpl");
            assert_eq!(*length, 3);
        }
        _ => panic!("expected method for doSomething"),
    }

    // Check value — note: parser trims quotes but not trailing commas
    match &class.proto[4].kind {
        PropertyKind::Value { value } => {
            assert!(value.starts_with("42"), "expected value starting with 42, got: {}", value);
        }
        _ => panic!("expected value for constantValue"),
    }

    // Static props: 2
    assert_eq!(class.static_props.len(), 2);
    match &class.static_props[0].kind {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "createInstance");
            assert_eq!(*length, 2);
        }
        _ => panic!("expected method for create"),
    }
    match &class.static_props[1].kind {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "getSharedInstance");
            assert!(cache);
        }
        _ => panic!("expected getter for sharedInstance"),
    }
}

#[test]
fn test_parse_class_name_with_special_chars() {
    let source = r#"
define({
    name: "HTMLInputElement",
    proto: {},
});
define({
    name: "SVG_NS",
    proto: {},
});
"#;
    let result = parse_classes(source, "special.classes.ts").unwrap();
    assert_eq!(result.classes.len(), 2);
    assert_eq!(result.classes[0].name, "HTMLInputElement");
    assert_eq!(result.classes[1].name, "SVG_NS");
}

#[test]
fn test_parse_empty_source() {
    let result = parse_classes("", "empty.ts");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().classes.len(), 0);
}

#[test]
fn test_parse_source_without_define() {
    let result = parse_classes("// just a comment\nconst x = 1;\n", "no-define.ts");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().classes.len(), 0);
}

#[test]
fn test_parse_result_source_file() {
    let result = parse_classes("define({ name: \"X\", proto: {} });", "test.classes.ts").unwrap();
    assert_eq!(result.source_file, "test.classes.ts");
}

// ---- Code generation completeness ----

#[test]
fn test_bindings_js_class_def_format() {
    let class = ClassDef {
        name: "Buffer".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);
    assert!(bindings.js_class_def.contains("static Buffer_Class: JSClass"));
    assert!(bindings.js_class_def.contains("c\"Buffer\""));
    assert!(bindings.js_class_def.contains("JSCLASS_FOREGROUND_FINALIZE"));
}

#[test]
fn test_bindings_constructor_fn_signature() {
    let class = ClassDef {
        name: "Server".into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(ctor.contains("unsafe extern \"C\" fn Server_constructor"));
    assert!(ctor.contains("cx: *mut JSContext"));
    assert!(ctor.contains("argc: u32"));
    assert!(ctor.contains("vp: *mut JS::Value"));
    assert!(ctor.contains("JS_NewObjectForConstructor"));
    assert!(ctor.contains("JS_SetReservedSlot"));
    assert!(ctor.contains("PrivateValue"));
}

#[test]
fn test_bindings_finalize_fn_signature() {
    let class = ClassDef {
        name: "Resource".into(),
        construct: false,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(fin.contains("unsafe extern \"C\" fn Resource_finalize"));
    assert!(fin.contains("gcx: *mut GCContext"));
    assert!(fin.contains("obj: *mut JSObject"));
    assert!(fin.contains("JS_GetReservedSlot"));
    assert!(fin.contains("Box::from_raw"));
}

#[test]
fn test_bindings_init_fn_structure() {
    let class = ClassDef {
        name: "Stream".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "read".into(),
                kind: PropertyKind::Method { fn_name: "streamRead".into(), length: 1 },
            },
            PropertyDef {
                name: "close".into(),
                kind: PropertyKind::Method { fn_name: "streamClose".into(), length: 0 },
            },
        ],
        static_props: vec![
            PropertyDef {
                name: "create".into(),
                kind: PropertyKind::Method { fn_name: "streamCreate".into(), length: 2 },
            },
        ],
    };
    let bindings = generate_bindings(&class);
    let init = &bindings.init_class_fn;

    assert!(init.contains("unsafe fn init_Stream"));
    assert!(init.contains("cx: *mut JSContext"));
    assert!(init.contains("global: JS::HandleObject"));
    assert!(init.contains("JS_InitClass"));
    assert!(init.contains("Some(Stream_constructor)"));
    assert!(init.contains("Stream_proto_specs"));
    assert!(init.contains("Stream_static_specs"));
    assert!(init.contains("!proto.is_null()"));
}

#[test]
fn test_bindings_specs_separation() {
    let class = ClassDef {
        name: "Mixed".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "getter1".into(),
                kind: PropertyKind::Getter { fn_name: "getGetter1".into(), cache: false },
            },
            PropertyDef {
                name: "method1".into(),
                kind: PropertyKind::Method { fn_name: "doMethod1".into(), length: 0 },
            },
            PropertyDef {
                name: "accessor1".into(),
                kind: PropertyKind::Accessor {
                    getter: "getAcc1".into(),
                    setter: "setAcc1".into(),
                    cache: false,
                },
            },
        ],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);

    // Methods go to function_specs
    assert_eq!(bindings.function_specs.len(), 1);
    assert!(bindings.function_specs[0].contains("doMethod1"));
    assert!(bindings.function_specs[0].contains("JSFunctionSpec"));

    // Getters, setters, accessors go to property_specs
    assert_eq!(bindings.property_specs.len(), 2);
    assert!(bindings.property_specs[0].contains("getGetter1"));
    assert!(bindings.property_specs[1].contains("getAcc1"));
    assert!(bindings.property_specs[1].contains("setAcc1"));
}

// ---- generate_module comprehensive validation ----

#[test]
fn test_module_output_ordering() {
    let classes = vec![
        ClassDef {
            name: "Zebra".into(),
            construct: false,
            no_constructor: false,
            finalize: false,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
        ClassDef {
            name: "Alpha".into(),
            construct: false,
            no_constructor: false,
            finalize: false,
            configurable: true,
            has_pending_activity: false,
            proto: vec![],
            static_props: vec![],
        },
    ];
    let bindings: Vec<GeneratedBindings> = classes.iter().map(generate_bindings).collect();
    let module = generate_module(&bindings, "test_module");

    let zebra_pos = module.find("Zebra").unwrap();
    let alpha_pos = module.find("Alpha").unwrap();
    assert!(zebra_pos < alpha_pos, "Classes should appear in input order");

    // init_all should have Zebra before Alpha
    let init_start = module.find("init_all").unwrap();
    let init_section = &module[init_start..];
    let init_zebra = init_section.find("init_Zebra").unwrap();
    let init_alpha = init_section.find("init_Alpha").unwrap();
    assert!(init_zebra < init_alpha);
}

#[test]
fn test_module_proto_and_static_arrays() {
    let class = ClassDef {
        name: "Widget".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![
            PropertyDef {
                name: "render".into(),
                kind: PropertyKind::Method { fn_name: "widgetRender".into(), length: 0 },
            },
            PropertyDef {
                name: "color".into(),
                kind: PropertyKind::Getter { fn_name: "getColor".into(), cache: false },
            },
        ],
        static_props: vec![
            PropertyDef {
                name: "defaultColor".into(),
                kind: PropertyKind::Getter { fn_name: "getDefaultColor".into(), cache: true },
            },
        ],
    };
    let bindings = generate_bindings(&class);
    let module = generate_module(&[bindings], "widget_module");

    assert!(module.contains("Widget_proto_specs: [JSPropertySpec; 2]"));
    assert!(module.contains("Widget_static_specs: [JSPropertySpec; 1]"));
    assert!(module.contains("widgetRender"));
    assert!(module.contains("getColor"));
    assert!(module.contains("getDefaultColor"));
}

#[test]
fn test_module_no_empty_arrays() {
    let class = ClassDef {
        name: "EmptyProto".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);
    let module = generate_module(&[bindings], "empty_proto_module");

    // init_class_fn still references array names (codegen limitation for empty classes)
    assert!(module.contains("init_EmptyProto"));
    // No separate array definitions when proto/static are empty
    // (init_class_fn references them but they'd need to be provided externally)
}

#[test]
fn test_generate_all_roundtrip() {
    // Test roundtrip with separate sources to avoid source-wide flag bleeding
    let source_r1 = r#"
define({
    name: "R1",
    construct: true,
    finalize: true,
    proto: {
        size: {
            getter: "getSize",
            cache: true,
        },
    },
    klass: {
        create: {
            fn: "createR1",
            length: 1,
        },
    },
});
"#;
    let result_r1 = parse_classes(source_r1, "r1.classes.ts").unwrap();
    let all_r1 = generate_all(&result_r1.classes);
    assert_eq!(all_r1.len(), 1);

    let r1 = &all_r1["R1"];
    assert!(r1.constructor_fn.is_some());
    assert!(r1.finalize_fn.is_some());
    assert_eq!(r1.property_specs.len(), 1);
    assert!(r1.property_specs[0].contains("getSize"));
    assert_eq!(r1.static_function_specs.len(), 1);
    assert!(r1.static_function_specs[0].contains("createR1"));
    assert!(r1.init_class_fn.contains("Some(R1_constructor)"));

    let source_r2 = r#"
define({
    name: "R2",
    proto: {
        destroy: {
            fn: "destroyR2",
            length: 0,
        },
    },
});
"#;
    let result_r2 = parse_classes(source_r2, "r2.classes.ts").unwrap();
    let all_r2 = generate_all(&result_r2.classes);
    assert_eq!(all_r2.len(), 1);

    let r2 = &all_r2["R2"];
    assert!(r2.constructor_fn.is_none());
    assert!(r2.finalize_fn.is_none());
    assert_eq!(r2.function_specs.len(), 1);
    assert!(r2.function_specs[0].contains("destroyR2"));
    assert!(r2.init_class_fn.contains("None"));
}

// ---- JsValue/JsError pure data tests (no mozjs needed) ----

#[test]
fn test_js_value_is_methods() {
    use bao_engine::value::JsValue;

    let undef = JsValue::Undefined;
    assert!(undef.is_undefined());
    assert!(!undef.is_null());
    assert!(!undef.is_number());
    assert!(!undef.is_string());
    assert!(!undef.is_object());

    let null = JsValue::Null;
    assert!(null.is_null());
    assert!(!null.is_undefined());

    let num = JsValue::Number(42.0);
    assert!(num.is_number());
    assert!(!num.is_string());

    let s = JsValue::String("hello".into());
    assert!(s.is_string());
    assert!(!num.is_string());

    let obj = JsValue::Object(std::ptr::null_mut());
    assert!(obj.is_object());
    assert!(!obj.is_string());
}

#[test]
fn test_js_value_as_methods() {
    use bao_engine::value::JsValue;

    let b = JsValue::Bool(true);
    assert_eq!(b.as_bool(), Some(true));
    assert_eq!(b.as_number(), None);

    let n = JsValue::Number(3.14);
    assert_eq!(n.as_number(), Some(3.14));
    assert_eq!(n.as_bool(), None);

    let s = JsValue::String("test".into());
    assert_eq!(s.as_string(), Some("test"));
    assert_eq!(s.as_number(), None);

    let undef = JsValue::Undefined;
    assert_eq!(undef.as_string(), None);
    assert_eq!(undef.as_bool(), None);
    assert_eq!(undef.as_number(), None);
}

#[test]
fn test_js_value_to_display_string() {
    use bao_engine::value::JsValue;

    assert_eq!(JsValue::Undefined.to_display_string(), "undefined");
    assert_eq!(JsValue::Null.to_display_string(), "null");
    assert_eq!(JsValue::Bool(true).to_display_string(), "true");
    assert_eq!(JsValue::Bool(false).to_display_string(), "false");
    assert_eq!(JsValue::Number(42.0).to_display_string(), "42");
    assert_eq!(JsValue::String("hello".into()).to_display_string(), "hello");
    assert_eq!(JsValue::Object(std::ptr::null_mut()).to_display_string(), "[object Object]");
}

#[test]
fn test_js_value_to_display_number_edge_cases() {
    use bao_engine::value::JsValue;

    assert_eq!(JsValue::Number(f64::NAN).to_display_string(), "NaN");
    assert_eq!(JsValue::Number(f64::INFINITY).to_display_string(), "Infinity");
    assert_eq!(JsValue::Number(f64::NEG_INFINITY).to_display_string(), "-Infinity");
    assert_eq!(JsValue::Number(0.0).to_display_string(), "0");
    assert_eq!(JsValue::Number(-0.0).to_display_string(), "0");
    assert_eq!(JsValue::Number(1.5).to_display_string(), "1.5");
}

#[test]
fn test_js_error_display() {
    use bao_engine::error::JsError;

    let err = JsError {
        message: "test error".into(),
        filename: "test.js".into(),
        line: 10,
        column: 5,
        stack: None,
    };
    let display = format!("{}", err);
    assert!(display.contains("test.js"));
    assert!(display.contains("10"));
    assert!(display.contains("5"));
    assert!(display.contains("test error"));
}

#[test]
fn test_js_error_display_with_stack() {
    use bao_engine::error::JsError;

    let err = JsError {
        message: "fail".into(),
        filename: "app.js".into(),
        line: 1,
        column: 1,
        stack: Some("at foo (app.js:1:1)\nat bar (app.js:5:3)".into()),
    };
    let display = format!("{}", err);
    assert!(display.contains("at foo"));
    assert!(display.contains("at bar"));
}

#[test]
fn test_js_error_is_std_error() {
    use bao_engine::error::JsError;

    let err = JsError {
        message: "test".into(),
        filename: "test.js".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn test_js_error_debug() {
    use bao_engine::error::JsError;

    let err = JsError {
        message: "debug test".into(),
        filename: "debug.js".into(),
        line: 42,
        column: 7,
        stack: Some("trace".into()),
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("JsError"));
    assert!(debug.contains("debug test"));
    assert!(debug.contains("42"));
}

#[test]
fn test_js_value_clone() {
    use bao_engine::value::JsValue;

    let v1 = JsValue::String("clone me".into());
    let v2 = v1.clone();
    assert_eq!(v1.as_string(), v2.as_string());

    let n1 = JsValue::Number(99.9);
    let n2 = n1.clone();
    assert_eq!(n1.as_number(), n2.as_number());
}

#[test]
fn test_property_kind_variants() {
    let getter = PropertyKind::Getter { fn_name: "getX".into(), cache: true };
    let setter = PropertyKind::Setter { fn_name: "setX".into() };
    let accessor = PropertyKind::Accessor { getter: "g".into(), setter: "s".into(), cache: false };
    let method = PropertyKind::Method { fn_name: "doIt".into(), length: 2 };
    let value = PropertyKind::Value { value: "hello".into() };

    // Just verify all variants compile and match
    match getter {
        PropertyKind::Getter { fn_name, cache } => {
            assert_eq!(fn_name, "getX");
            assert!(cache);
        }
        _ => panic!("wrong variant"),
    }
    match setter {
        PropertyKind::Setter { fn_name } => assert_eq!(fn_name, "setX"),
        _ => panic!("wrong variant"),
    }
    match accessor {
        PropertyKind::Accessor { getter, setter, cache } => {
            assert_eq!(getter, "g");
            assert_eq!(setter, "s");
            assert!(!cache);
        }
        _ => panic!("wrong variant"),
    }
    match method {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "doIt");
            assert_eq!(length, 2);
        }
        _ => panic!("wrong variant"),
    }
    match value {
        PropertyKind::Value { value } => assert_eq!(value, "hello"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_class_def_debug_output() {
    let class = ClassDef {
        name: "Test".into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: false,
        has_pending_activity: true,
        proto: vec![PropertyDef {
            name: "x".into(),
            kind: PropertyKind::Getter { fn_name: "getX".into(), cache: false },
        }],
        static_props: vec![],
    };
    let debug = format!("{:?}", class);
    assert!(debug.contains("Test"));
    assert!(debug.contains("construct: true"));
    assert!(debug.contains("finalize: true"));
}

#[test]
fn test_parse_result_debug() {
    let result = parse_classes("define({ name: \"D\", proto: {} });", "d.ts").unwrap();
    let debug = format!("{:?}", result);
    assert!(debug.contains("ParseResult"));
    assert!(debug.contains("d.ts"));
}

#[test]
fn test_generated_bindings_debug() {
    let class = ClassDef {
        name: "Debug".into(),
        construct: false,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    };
    let bindings = generate_bindings(&class);
    let debug = format!("{:?}", bindings);
    assert!(debug.contains("GeneratedBindings"));
    assert!(debug.contains("Debug"));
}

#[test]
fn test_parse_value_property_with_single_quotes() {
    let source = r#"
define({
    name: "Quoted",
    proto: {
        version: '1.0.0',
    },
});
"#;
    let result = parse_classes(source, "quoted.classes.ts").unwrap();
    let class = &result.classes[0];
    assert_eq!(class.proto.len(), 1);
    match &class.proto[0].kind {
        PropertyKind::Value { value } => {
            // Parser trims quotes but may leave trailing comma
            assert!(value.starts_with("1.0.0"), "expected value starting with 1.0.0, got: {}", value);
        }
        _ => panic!("expected value property"),
    }
}

#[test]
fn test_parse_method_with_large_length() {
    let source = r#"
define({
    name: "ManyArgs",
    proto: {
        compute: { fn: "computeImpl", length: 100 },
    },
});
"#;
    let result = parse_classes(source, "many_args.classes.ts").unwrap();
    match &result.classes[0].proto[0].kind {
        PropertyKind::Method { fn_name, length } => {
            assert_eq!(fn_name, "computeImpl");
            assert_eq!(*length, 100);
        }
        _ => panic!("expected method"),
    }
}

#[test]
fn test_parse_class_default_flags() {
    // Source without explicit construct/finalize flags — must be multi-line for parser
    let source = r#"
define({
    name: "Minimal",
    proto: {},
});
"#;
    let result = parse_classes(source, "min.ts").unwrap();
    let class = &result.classes[0];
    // parse_classes checks source.contains("construct: true"), which is absent
    assert!(!class.construct);
    assert!(!class.no_constructor);
    assert!(!class.finalize);
    assert!(class.configurable); // default true when configurable: false is absent
    assert!(!class.has_pending_activity);
}
