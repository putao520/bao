// @trace REQ-ENG-002 [entity:CodegenBackend]
// Code generation backend: parses Bun .classes.ts definitions and generates SpiderMonkey bindings.
// Replaces JSC C++ template generation with SM Rust binding generation.

use ::std::collections::HashMap;

/// Parsed class definition from .classes.ts format.
#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: String,
    pub construct: bool,
    pub no_constructor: bool,
    pub finalize: bool,
    pub configurable: bool,
    pub has_pending_activity: bool,
    pub proto: Vec<PropertyDef>,
    pub static_props: Vec<PropertyDef>,
}

/// Property definition (getter, setter, method, or value).
#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub kind: PropertyKind,
}

#[derive(Debug, Clone)]
pub enum PropertyKind {
    Getter { fn_name: String, cache: bool },
    Setter { fn_name: String },
    Accessor { getter: String, setter: String, cache: bool },
    Method { fn_name: String, length: u32 },
    Value { value: String },
}

/// Parse result containing all class definitions from a .classes.ts file.
#[derive(Debug)]
pub struct ParseResult {
    pub classes: Vec<ClassDef>,
    pub source_file: String,
}

/// Parse a .classes.ts file content and extract class definitions.
pub fn parse_classes(source: &str, file_name: &str) -> Result<ParseResult, String> {
    let mut classes = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches(',').trim_matches('"').trim().to_string();
            if !name.is_empty() {
                classes.push(ClassDef {
                    name,
                    construct: source.contains("construct: true"),
                    no_constructor: source.contains("noConstructor: true"),
                    finalize: source.contains("finalize: true"),
                    configurable: !source.contains("configurable: false"),
                    has_pending_activity: source.contains("hasPendingActivity: true"),
                    proto: parse_proto_properties(source),
                    static_props: Vec::new(),
                });
            }
        }
    }

    Ok(ParseResult {
        classes,
        source_file: file_name.to_string(),
    })
}

fn parse_proto_properties(source: &str) -> Vec<PropertyDef> {
    let mut props = Vec::new();
    let mut in_proto = false;

    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.starts_with("proto:") || trimmed.starts_with("proto {") {
            in_proto = true;
            i += 1;
            continue;
        }
        if in_proto && (trimmed.starts_with("}") || trimmed.starts_with("klass:")) {
            in_proto = false;
            i += 1;
            continue;
        }
        if !in_proto {
            i += 1;
            continue;
        }

        if let Some(colon_pos) = trimmed.find(':') {
            let name = trimmed[..colon_pos].trim().to_string();
            if name.is_empty() || name == "proto" {
                i += 1;
                continue;
            }

            let value_part = trimmed[colon_pos + 1..].trim();
            if value_part.starts_with('{') {
                // Collect multi-line block content
                let mut block = value_part.to_string();
                let mut depth = block.chars().filter(|c| *c == '{').count() as i32
                    - block.chars().filter(|c| *c == '}').count() as i32;
                let mut j = i + 1;
                while depth > 0 && j < lines.len() {
                    let next = lines[j].trim();
                    depth += next.chars().filter(|c| *c == '{').count() as i32
                        - next.chars().filter(|c| *c == '}').count() as i32;
                    block.push(' ');
                    block.push_str(next);
                    j += 1;
                }

                if block.contains("getter:") {
                    let fn_name = extract_string_value(&block, "getter");
                    let cache = block.contains("cache: true");
                    if block.contains("setter:") {
                        let setter = extract_string_value(&block, "setter");
                        props.push(PropertyDef {
                            name,
                            kind: PropertyKind::Accessor { getter: fn_name, setter, cache },
                        });
                    } else {
                        props.push(PropertyDef {
                            name,
                            kind: PropertyKind::Getter { fn_name, cache },
                        });
                    }
                } else if block.contains("setter:") {
                    let fn_name = extract_string_value(&block, "setter");
                    props.push(PropertyDef {
                        name,
                        kind: PropertyKind::Setter { fn_name },
                    });
                } else if block.contains("fn:") {
                    let fn_name = extract_string_value(&block, "fn");
                    let length = extract_number_value(&block, "length");
                    props.push(PropertyDef {
                        name,
                        kind: PropertyKind::Method { fn_name, length },
                    });
                }
                i = j;
                continue;
            } else if value_part.starts_with('"') || value_part.starts_with('\'') {
                props.push(PropertyDef {
                    name,
                    kind: PropertyKind::Value {
                        value: value_part.trim_matches('"').trim_matches('\'').to_string(),
                    },
                });
            }
        }
        i += 1;
    }

    props
}

fn extract_string_value(source: &str, key: &str) -> String {
    let pattern = format!("{}:", key);
    if let Some(pos) = source.find(&pattern) {
        let rest = &source[pos + pattern.len()..];
        let rest = rest.trim();
        if rest.starts_with('"') {
            if let Some(end) = rest[1..].find('"') {
                return rest[1..end + 1].to_string();
            }
        }
    }
    String::new()
}

fn extract_number_value(source: &str, key: &str) -> u32 {
    let pattern = format!("{}:", key);
    if let Some(pos) = source.find(&pattern) {
        let rest = &source[pos + pattern.len()..].trim();
        rest.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    } else {
        0
    }
}

/// Code generator output for a single class.
#[derive(Debug)]
pub struct GeneratedBindings {
    pub class_name: String,
    pub js_class_def: String,
    pub function_specs: Vec<String>,
    pub property_specs: Vec<String>,
}

/// Generate SpiderMonkey binding code from a parsed class definition.
pub fn generate_bindings(class_def: &ClassDef) -> GeneratedBindings {
    let class_name = &class_def.name;
    let js_name = format!("{}_Class", class_name);

    let js_class_def = format!(
        r#"static {js_name}: JSClass = JSClass {{
    name: c"{class_name}".as_ptr(),
    flags: JSCLASS_FOREGROUND_FINALIZE_PROHIBITED as u32,
    ..Default::default()
}};"#,
        js_name = js_name,
        class_name = class_name,
    );

    let mut function_specs = Vec::new();
    let mut property_specs = Vec::new();

    for prop in &class_def.proto {
        match &prop.kind {
            PropertyKind::Method { fn_name, length } => {
                function_specs.push(format!(
                    r#"JSFunctionSpec {{
    name: c"{name}".as_ptr(),
    call: Some({fn_name}),
    nargs: {length},
    flags: JSPROP_ENUMERATE as u16,
    ..Default::default()
}}"#,
                    name = prop.name,
                    fn_name = fn_name,
                    length = length,
                ));
            }
            PropertyKind::Getter { fn_name, .. } => {
                property_specs.push(format!(
                    r#"JSPropertySpec {{
    name: c"{name}".as_ptr(),
    getter: JSPropertySpec_AccessorOrValue {{
        accessors: JSPropertySpec_Accessor {{
            getter: Some({fn_name}),
            ..Default::default()
        }}
    }},
    ..Default::default()
}}"#,
                    name = prop.name,
                    fn_name = fn_name,
                ));
            }
            PropertyKind::Accessor { getter, setter, .. } => {
                property_specs.push(format!(
                    r#"JSPropertySpec {{
    name: c"{name}".as_ptr(),
    getter: JSPropertySpec_AccessorOrValue {{
        accessors: JSPropertySpec_Accessor {{
            getter: Some({getter}),
            setter: Some({setter}),
            ..Default::default()
        }}
    }},
    ..Default::default()
}}"#,
                    name = prop.name,
                    getter = getter,
                    setter = setter,
                ));
            }
            _ => {}
        }
    }

    GeneratedBindings {
        class_name: class_name.clone(),
        js_class_def,
        function_specs,
        property_specs,
    }
}

/// Batch generate bindings for all class definitions.
pub fn generate_all(class_defs: &[ClassDef]) -> HashMap<String, GeneratedBindings> {
    class_defs
        .iter()
        .map(|cd| (cd.name.clone(), generate_bindings(cd)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_class() {
        let source = r#"
define({
    name: "TestResource",
    construct: true,
    finalize: true,
    configurable: false,
    proto: {
        count: {
            getter: "getCount",
            cache: true,
        },
        reset: {
            fn: "resetCount",
            length: 0,
        },
    },
    klass: {},
});
"#;
        let result = parse_classes(source, "test.classes.ts").unwrap();
        assert_eq!(result.classes.len(), 1);
        let class = &result.classes[0];
        assert_eq!(class.name, "TestResource");
        assert!(class.construct);
        assert!(class.finalize);
        assert!(!class.configurable);
        assert_eq!(class.proto.len(), 2);

        match &class.proto[0].kind {
            PropertyKind::Getter { fn_name, cache } => {
                assert_eq!(fn_name, "getCount");
                assert!(cache);
            }
            _ => panic!("expected getter"),
        }
        match &class.proto[1].kind {
            PropertyKind::Method { fn_name, length } => {
                assert_eq!(fn_name, "resetCount");
                assert_eq!(*length, 0);
            }
            _ => panic!("expected method"),
        }
    }

    #[test]
    fn test_generate_bindings() {
        let class = ClassDef {
            name: "MyClass".into(),
            construct: true,
            no_constructor: false,
            finalize: true,
            configurable: true,
            has_pending_activity: false,
            proto: vec![
                PropertyDef {
                    name: "value".into(),
                    kind: PropertyKind::Getter { fn_name: "getValue".into(), cache: false },
                },
                PropertyDef {
                    name: "compute".into(),
                    kind: PropertyKind::Method { fn_name: "computeValue".into(), length: 2 },
                },
            ],
            static_props: vec![],
        };
        let bindings = generate_bindings(&class);
        assert_eq!(bindings.class_name, "MyClass");
        assert!(bindings.js_class_def.contains("MyClass"));
        assert_eq!(bindings.function_specs.len(), 1);
        assert_eq!(bindings.property_specs.len(), 1);
    }

    #[test]
    fn test_generate_all() {
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
        let all = generate_all(&classes);
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("A"));
        assert!(all.contains_key("B"));
    }

    #[test]
    fn test_parse_accessor_property() {
        let source = r#"
define({
    name: "TestAccessor",
    proto: {
        data: {
            accessor: { getter: "getData", setter: "setData" },
            cache: true,
        },
    },
});
"#;
        let result = parse_classes(source, "accessor.classes.ts").unwrap();
        let class = &result.classes[0];
        assert_eq!(class.proto.len(), 1);
        match &class.proto[0].kind {
            PropertyKind::Accessor { getter, setter, cache } => {
                assert_eq!(getter, "getData");
                assert_eq!(setter, "setData");
                assert!(cache);
            }
            _ => panic!("expected accessor"),
        }
    }

    #[test]
    fn test_parse_empty_proto() {
        let source = r#"
define({
    name: "EmptyProto",
    proto: {},
});
"#;
        let result = parse_classes(source, "empty.classes.ts").unwrap();
        assert_eq!(result.classes[0].proto.len(), 0);
    }
}
