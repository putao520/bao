// @trace TEST-ENG-002-CTOR-FIN [req:REQ-ENG-002] [level:unit]
// Tests for constructor and finalizer code generation:
// - Constructor allocates native data via Box and stores in reserved slot
// - Finalizer retrieves native data from reserved slot and drops via Box::from_raw
// - JSClassOps binds the finalizer correctly
// - JSClass flags include JSCLASS_FOREGROUND_FINALIZE when finalize is enabled

use bao_engine::codegen::*;

fn make_class_with_ctor_and_finalize(name: &str) -> ClassDef {
    ClassDef {
        name: name.into(),
        construct: true,
        no_constructor: false,
        finalize: true,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    }
}

fn make_class_without_finalize(name: &str) -> ClassDef {
    ClassDef {
        name: name.into(),
        construct: true,
        no_constructor: false,
        finalize: false,
        configurable: true,
        has_pending_activity: false,
        proto: vec![],
        static_props: vec![],
    }
}

// ---- Constructor: native data allocation ----

#[test]
fn test_constructor_allocates_boxed_native_data() {
    let class = make_class_with_ctor_and_finalize("NativeObj");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(
        ctor.contains("Box::new"),
        "constructor must allocate native data with Box::new"
    );
    assert!(
        ctor.contains("Box::into_raw"),
        "constructor must convert Box to raw pointer via Box::into_raw"
    );
}

#[test]
fn test_constructor_stores_native_data_in_reserved_slot_0() {
    let class = make_class_with_ctor_and_finalize("SlotObj");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(
        ctor.contains("JS_SetReservedSlot(obj, 0,"),
        "constructor must store native data in reserved slot 0"
    );
    assert!(
        ctor.contains("PrivateValue"),
        "constructor must wrap raw pointer in PrivateValue"
    );
}

#[test]
fn test_constructor_creates_object_with_js_new_object_for_constructor() {
    let class = make_class_with_ctor_and_finalize("ConstructObj");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(
        ctor.contains("JS_NewObjectForConstructor"),
        "constructor must use JS_NewObjectForConstructor"
    );
    assert!(
        ctor.contains("CallArgs::from_vp"),
        "constructor must extract CallArgs from vp"
    );
}

#[test]
fn test_constructor_returns_object_value() {
    let class = make_class_with_ctor_and_finalize("RetVal");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(
        ctor.contains("ObjectValue(obj)"),
        "constructor must set return value to ObjectValue"
    );
    assert!(
        ctor.contains("args.rval().set"),
        "constructor must set rval"
    );
}

#[test]
fn test_constructor_handles_null_object() {
    let class = make_class_with_ctor_and_finalize("NullCheck");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(
        ctor.contains("is_null()"),
        "constructor must check for null object"
    );
    assert!(
        ctor.contains("return false"),
        "constructor must return false on null object"
    );
}

// ---- Finalizer: native data cleanup ----

#[test]
fn test_finalizer_has_correct_jsfinalizeop_signature() {
    let class = make_class_with_ctor_and_finalize("FinSig");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(
        fin.contains("gcx: *mut GCContext"),
        "finalizer must accept GCContext pointer (JSFinalizeOp signature)"
    );
    assert!(
        fin.contains("obj: *mut JSObject"),
        "finalizer must accept JSObject pointer (JSFinalizeOp signature)"
    );
}

#[test]
fn test_finalizer_reads_reserved_slot_0() {
    let class = make_class_with_ctor_and_finalize("SlotRead");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(
        fin.contains("JS_GetReservedSlot(obj, 0,"),
        "finalizer must read native data from reserved slot 0"
    );
}

#[test]
fn test_finalizer_drops_boxed_native_data() {
    let class = make_class_with_ctor_and_finalize("DropData");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(
        fin.contains("Box::from_raw"),
        "finalizer must reconstruct Box from raw pointer via Box::from_raw"
    );
    assert!(
        fin.contains("let _ ="),
        "finalizer must drop the Box (let _ = ... drops it)"
    );
}

#[test]
fn test_finalizer_checks_null_pointer() {
    let class = make_class_with_ctor_and_finalize("NullPtr");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(
        fin.contains("!ptr.is_null()"),
        "finalizer must check for null before dropping"
    );
}

#[test]
fn test_finalizer_uses_to_private_to_extract_pointer() {
    let class = make_class_with_ctor_and_finalize("PrivExtract");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(
        fin.contains("to_private()"),
        "finalizer must use to_private() to extract pointer from JS::Value"
    );
}

// ---- JSClassOps integration ----

#[test]
fn test_class_ops_generated_when_finalize_true() {
    let class = make_class_with_ctor_and_finalize("OpsGen");
    let bindings = generate_bindings(&class);
    let ops = bindings.class_ops_def.as_ref().expect("class_ops_def must be Some when finalize=true");
    assert!(ops.contains("JSClassOps"), "class_ops_def must contain JSClassOps struct");
    assert!(ops.contains("finalize: Some"), "class_ops must set finalize to Some(...)");
}

#[test]
fn test_class_ops_not_generated_when_finalize_false() {
    let class = make_class_without_finalize("NoOps");
    let bindings = generate_bindings(&class);
    assert!(
        bindings.class_ops_def.is_none(),
        "class_ops_def must be None when finalize=false"
    );
}

#[test]
fn test_class_ops_finalize_references_class_finalizer() {
    let class = make_class_with_ctor_and_finalize("OpsRef");
    let bindings = generate_bindings(&class);
    let ops = bindings.class_ops_def.unwrap();
    assert!(
        ops.contains("Some(OpsRef_finalize)"),
        "class_ops must reference the class-specific finalizer"
    );
}

#[test]
fn test_class_ops_all_other_fields_none() {
    let class = make_class_with_ctor_and_finalize("OpsClean");
    let bindings = generate_bindings(&class);
    let ops = bindings.class_ops_def.unwrap();
    // All fields except finalize should be None
    assert!(ops.contains("addProperty: None"));
    assert!(ops.contains("delProperty: None"));
    assert!(ops.contains("enumerate: None"));
    assert!(ops.contains("newEnumerate: None"));
    assert!(ops.contains("resolve: None"));
    assert!(ops.contains("mayResolve: None"));
    assert!(ops.contains("call: None"));
    assert!(ops.contains("construct: None"));
    assert!(ops.contains("trace: None"));
}

// ---- JSClass flags ----

#[test]
fn test_js_class_flags_foreground_finalize_when_finalize_true() {
    let class = make_class_with_ctor_and_finalize("FlagFinalize");
    let bindings = generate_bindings(&class);
    assert!(
        bindings.js_class_def.contains("JSCLASS_FOREGROUND_FINALIZE"),
        "JSClass flags must include JSCLASS_FOREGROUND_FINALIZE when finalize=true"
    );
}

#[test]
fn test_js_class_flags_zero_when_finalize_false() {
    let class = make_class_without_finalize("FlagNoFinalize");
    let bindings = generate_bindings(&class);
    assert!(
        bindings.js_class_def.contains("flags: 0u32"),
        "JSClass flags must be 0 when finalize=false"
    );
}

#[test]
fn test_js_class_cops_set_when_finalize_true() {
    let class = make_class_with_ctor_and_finalize("CopsSet");
    let bindings = generate_bindings(&class);
    assert!(
        bindings.js_class_def.contains("cOps: &CopsSet_ClassOps"),
        "JSClass cOps must reference the ClassOps static when finalize=true"
    );
}

#[test]
fn test_js_class_cops_null_when_finalize_false() {
    let class = make_class_without_finalize("CopsNull");
    let bindings = generate_bindings(&class);
    assert!(
        bindings.js_class_def.contains("cOps: std::ptr::null()"),
        "JSClass cOps must be null when finalize=false"
    );
}

// ---- Slot consistency: constructor and finalizer use the same slot ----

#[test]
fn test_constructor_and_finalizer_use_same_reserved_slot() {
    let class = make_class_with_ctor_and_finalize("SlotMatch");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    let fin = bindings.finalize_fn.unwrap();

    // Constructor writes to slot 0, finalizer reads from slot 0
    assert!(
        ctor.contains("JS_SetReservedSlot(obj, 0,"),
        "constructor writes to slot 0"
    );
    assert!(
        fin.contains("JS_GetReservedSlot(obj, 0,"),
        "finalizer reads from slot 0"
    );
}

// ---- Module output integration ----

#[test]
fn test_module_emits_class_ops_before_js_class() {
    let class = make_class_with_ctor_and_finalize("OrderMod");
    let bindings = generate_bindings(&class);
    let module = generate_module(&[bindings], "order_module");

    let ops_pos = module.find("OrderMod_ClassOps").expect("ClassOps must be in module");
    let class_pos = module.find("static OrderMod_Class: JSClass").expect("JSClass must be in module");
    assert!(
        ops_pos < class_pos,
        "ClassOps must appear before JSClass in module output"
    );
}

#[test]
fn test_module_contains_both_class_ops_and_finalizer() {
    let class = make_class_with_ctor_and_finalize("FinOrder");
    let bindings = generate_bindings(&class);
    let module = generate_module(&[bindings], "finorder_module");

    // ClassOps references the finalizer by name; Rust statics allow forward references
    assert!(
        module.contains("FinOrder_finalize"),
        "module must contain the finalizer function"
    );
    assert!(
        module.contains("FinOrder_ClassOps"),
        "module must contain the ClassOps static"
    );
    assert!(
        module.contains("Some(FinOrder_finalize)"),
        "ClassOps must bind the finalizer"
    );
}

#[test]
fn test_module_no_class_ops_for_non_finalize_class() {
    let class = make_class_without_finalize("NoOpsMod");
    let bindings = generate_bindings(&class);
    let module = generate_module(&[bindings], "noops_module");

    assert!(
        !module.contains("NoOpsMod_ClassOps"),
        "module must not contain ClassOps for non-finalize class"
    );
    assert!(
        !module.contains("NoOpsMod_finalize"),
        "module must not contain finalizer for non-finalize class"
    );
}

// ---- No TODO/FIXME/stub remains ----

#[test]
fn test_no_todo_in_constructor() {
    let class = make_class_with_ctor_and_finalize("CtorClean");
    let bindings = generate_bindings(&class);
    let ctor = bindings.constructor_fn.unwrap();
    assert!(!ctor.contains("TODO"), "constructor must not contain TODO");
    assert!(!ctor.contains("FIXME"), "constructor must not contain FIXME");
    assert!(!ctor.contains("stub"), "constructor must not contain stub");
}

#[test]
fn test_no_todo_in_finalizer() {
    let class = make_class_with_ctor_and_finalize("FinClean");
    let bindings = generate_bindings(&class);
    let fin = bindings.finalize_fn.unwrap();
    assert!(!fin.contains("TODO"), "finalizer must not contain TODO");
    assert!(!fin.contains("FIXME"), "finalizer must not contain FIXME");
    assert!(!fin.contains("stub"), "finalizer must not contain stub");
}
