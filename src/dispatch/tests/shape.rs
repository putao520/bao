// Integration test — proc-macro crates can't unit-test their own macros
// (the host crate is compiled as a dylib, not linked into the test binary).
//
// Adversarial coverage of the closed-set dispatch contract declared by
// `link_interface!` (see lib.rs module docs). The invariants under test:
//
//   INV-1  dispatch routes solely on `ShapeKind`; each variant reaches the
//          `link_impl_Shape!` body written for it (no vtable, no UB).
//   INV-2  `Shape` is `Copy + Clone`; `kind` and `owner` are `pub` fields.
//   INV-3  `is(kind)` is a pure equality on `self.kind`, independent of owner.
//   INV-4  every method declared in the interface is callable on every
//          variant (closed set: link error if any method is missing).
//   INV-5  the only `unsafe` boundary is `Shape::new`; dispatch methods are
//          safe given the precondition "owner stays live for every dispatch".
//   INV-6  `&mut`-style methods (scale) mutate the owner in place; the
//          mutation is observable through a subsequent dispatch on the
//          *same* handle (no internal copy).
//   INV-7  `&mut String` out-params append in place; label() is additive.
//   INV-8  f64 methods preserve IEEE-754 semantics (NaN, -0.0, Inf).
//
// @trace TEST-DISPATCH-001 REQ-ENG-001 — closed-set cross-crate dispatch
//        invariants, boundary conditions, and macro contract.

bun_dispatch::link_interface! {
    pub Shape[Circle, Square] {
        fn area() -> f64;
        fn scale(k: f64);
        fn name() -> &'static str;
        fn label(prefix: &str, out: &mut String);
    }
}

pub(crate) struct CircleT {
    r: f64,
}
pub(crate) struct SquareT {
    s: f64,
}

link_impl_Shape! {
    Circle for CircleT => |this| {
        area()    => core::f64::consts::PI * (*this).r * (*this).r,
        scale(k)  => (*this).r *= k,
        name()    => "circle",
        label(prefix, out) => { out.push_str(prefix); out.push_str("circle"); },
    }
}

link_impl_Shape! {
    Square for SquareT => |this| {
        area()    => (*this).s * (*this).s,
        scale(k)  => (*this).s *= k,
        name()    => "square",
        label(prefix, out) => { out.push_str(prefix); out.push_str("square"); },
    }
}

// ─────────────────────────────────────────────────────────────────────────
// INV-1 / INV-4: positive dispatch round-trip — every method × every variant
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn dispatch_round_trip() {
    let mut c = CircleT { r: 2.0 };
    let mut s = SquareT { s: 3.0 };
    // SAFETY: c/s are live for the duration of every dispatch below.
    let hc = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };

    assert!((hc.area() - core::f64::consts::PI * 4.0).abs() < 1e-9);
    assert_eq!(hs.area(), 9.0);
    assert_eq!(hc.name(), "circle");
    assert_eq!(hs.name(), "square");
    assert!(hc.is(ShapeKind::Circle));
    assert!(!hc.is(ShapeKind::Square));

    hc.scale(2.0);
    hs.scale(2.0);
    assert!((hc.area() - core::f64::consts::PI * 16.0).abs() < 1e-9);
    assert_eq!(hs.area(), 36.0);

    let mut buf = String::new();
    hc.label("a ", &mut buf);
    hs.label(" / a ", &mut buf);
    assert_eq!(buf, "a circle / a square");
}

// ─────────────────────────────────────────────────────────────────────────
// INV-2: Shape is Copy/Clone, and pub fields are accessible per the macro
// contract (`#[derive(Copy, Clone)] pub struct Shape { pub kind, pub owner }`).
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn shape_handle_is_copy_clone_and_fields_are_public() {
    let mut c = CircleT { r: 1.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };

    // Copy: a fresh binding from a plain copy does not change dispatch.
    let h_copy = h; // uses Copy
    assert!(h_copy.is(ShapeKind::Circle));
    assert_eq!(h_copy.name(), "circle");
    // The original is still usable (Copy, not Move).
    assert_eq!(h.name(), "circle");

    // Clone produces an equivalent handle pointing at the same owner.
    let h_clone = h.clone();
    assert_eq!(h_clone.kind, h.kind);
    assert_eq!(h_clone.owner, h.owner);
    assert_eq!(h_clone.name(), "circle");

    // `kind` and `owner` are public fields per the generated struct.
    let _kind: ShapeKind = h.kind;
    let _owner: *mut () = h.owner;
    // The handle round-trips through field reconstruction.
    let rebuilt = Shape { kind: h.kind, owner: h.owner };
    assert!(rebuilt.is(ShapeKind::Circle));
    assert_eq!(h.area(), rebuilt.area());
}

// ─────────────────────────────────────────────────────────────────────────
// INV-3: `is()` is pure equality on `self.kind`, independent of the owner
// pointer. Two handles with the same kind but different owners compare the
// same against a given kind; two handles with different kinds differ.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn is_is_pure_kind_equality_independent_of_owner() {
    let mut c1 = CircleT { r: 1.0 };
    let mut c2 = CircleT { r: 99.0 };
    let mut s1 = SquareT { s: 1.0 };

    let h1 = unsafe { Shape::new(ShapeKind::Circle, &raw mut c1) };
    let h2 = unsafe { Shape::new(ShapeKind::Circle, &raw mut c2) };
    let h3 = unsafe { Shape::new(ShapeKind::Square, &raw mut s1) };

    // Same kind, different owners → both true for Circle.
    assert!(h1.is(ShapeKind::Circle) && h2.is(ShapeKind::Circle));
    // Different owners yield different `area()` (proves owner is honored at
    // dispatch time, while `is()` ignores it).
    assert!((h1.area() - core::f64::consts::PI).abs() < 1e-9);
    assert!((h2.area() - core::f64::consts::PI * 9801.0).abs() < 1e-6);

    // Different kind → `is` flips.
    assert!(!h3.is(ShapeKind::Circle));
    assert!(h3.is(ShapeKind::Square));

    // Every variant returns false for the other kind (exhaustive cross-check).
    for h in [h1, h2] {
        assert!(h.is(ShapeKind::Circle));
        assert!(!h.is(ShapeKind::Square));
    }
    assert!(h3.is(ShapeKind::Square));
    assert!(!h3.is(ShapeKind::Circle));
}

// ─────────────────────────────────────────────────────────────────────────
// INV-6: scale() mutates the owner in place. A second dispatch on the SAME
// handle observes the new state (the handle holds a raw pointer, not a copy).
// Scaling back down returns to the original area (associativity, no drift).
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn scale_mutates_owner_in_place_observed_via_same_handle() {
    let mut c = CircleT { r: 2.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };

    let a0 = h.area();
    h.scale(3.0);
    // area scales with the SQUARE of the linear factor: 2→6 is ×3 linear, ×9 area.
    assert!((h.area() - a0 * 9.0).abs() < 1e-9);
    h.scale(1.0 / 3.0);
    // Returning to the original radius restores the original area exactly.
    assert!((h.area() - a0).abs() < 1e-9);

    let mut s = SquareT { s: 4.0 };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };
    let b0 = hs.area();
    hs.scale(0.5);
    assert_eq!(hs.area(), b0 * 0.25);
    hs.scale(2.0);
    assert_eq!(hs.area(), b0);
}

// ─────────────────────────────────────────────────────────────────────────
// Boundary: scale(0.0) collapses the geometry to zero area (degenerate but
// well-defined). The handle remains valid for further dispatch.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn scale_zero_collapses_to_degenerate_and_handle_remains_valid() {
    let mut c = CircleT { r: 5.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    h.scale(0.0);
    assert_eq!(h.area(), 0.0);
    // Still dispatchable; recovering via further scale is observable.
    h.scale(2.0);
    assert_eq!(h.area(), 0.0);
    assert_eq!(h.name(), "circle");

    let mut s = SquareT { s: 7.0 };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };
    hs.scale(0.0);
    assert_eq!(hs.area(), 0.0);
    assert_eq!(hs.name(), "square");
}

// ─────────────────────────────────────────────────────────────────────────
// Boundary: negative scale flips sign of the backing field. For Circle.area,
// area = PI * r * r is sign-insensitive on r (square), so a single negative
// scale leaves area unchanged; two negatives cancel. For Square the same
// holds. This pins IEEE-754 multiplication semantics in the generated code.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn scale_negative_preserves_area_under_squaring() {
    let mut c = CircleT { r: 3.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    let a0 = h.area();
    h.scale(-1.0);
    assert!((h.area() - a0).abs() < 1e-9, "r^2 is sign-insensitive");
    // r is now -3.0; scaling by -2 → r = 6.0, area = PI * 36.
    h.scale(-2.0);
    assert!((h.area() - core::f64::consts::PI * 36.0).abs() < 1e-9);

    let mut s = SquareT { s: 3.0 };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };
    let b0 = hs.area();
    hs.scale(-1.0);
    assert_eq!(hs.area(), b0);
}

// ─────────────────────────────────────────────────────────────────────────
// INV-8 / IEEE-754: NaN scale propagates through `*=`; every subsequent area
// is NaN (all comparisons against NaN are false). Inf scale diverges. This
// documents that the generated code does no hidden clamping — the body is a
// literal transcription of the impl macro.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn scale_nan_and_inf_propagate_per_ieee754() {
    let mut c = CircleT { r: 2.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    h.scale(f64::NAN);
    let a = h.area();
    assert!(a.is_nan(), "NaN radius → NaN area");

    let mut s = SquareT { s: 1.0 };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };
    hs.scale(f64::INFINITY);
    assert_eq!(hs.area(), f64::INFINITY);
    // name()/is() are unaffected by numerical state — kind is independent.
    assert_eq!(hs.name(), "square");
    assert!(hs.is(ShapeKind::Square));
}

// ─────────────────────────────────────────────────────────────────────────
// INV-7: label() is additive on the provided &mut String. Empty prefix is
// appended verbatim (no special-casing). Multiple calls accumulate.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn label_is_additive_and_handles_empty_and_unicode_prefix() {
    let mut c = CircleT { r: 1.0 };
    let mut s = SquareT { s: 1.0 };
    let hc = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    let hs = unsafe { Shape::new(ShapeKind::Square, &raw mut s) };

    // Empty prefix: the variant name alone is appended.
    let mut buf = String::new();
    hc.label("", &mut buf);
    assert_eq!(buf, "circle");

    // Unicode / multibyte prefix passes through byte-wise via push_str.
    hc.label(" 🟢", &mut buf);
    hs.label(" / ⬛", &mut buf);
    assert_eq!(buf, "circle 🟢circle / ⬛square");

    // Existing buffer contents are preserved (append, not replace).
    let mut buf2 = String::from("head>");
    hs.label("", &mut buf2);
    assert_eq!(buf2, "head>square");
}

// ─────────────────────────────────────────────────────────────────────────
// INV-1 (independence): dispatching through a freshly-constructed handle for
// the SAME owner&kind is equivalent to the original handle. Re-creating the
// handle does not mutate the owner. This catches any macro bug where
// `new()` would touch the pointed-to memory.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn reconstructing_handle_for_same_owner_is_equivalent() {
    let mut c = CircleT { r: 4.0 };
    let h = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    let before = h.area();

    let h2 = unsafe { Shape::new(ShapeKind::Circle, &raw mut c) };
    // `new` must not have mutated the owner.
    assert!((h2.area() - before).abs() < 1e-9);
    // Both handles alias the same memory; scaling via one is seen by the other.
    h2.scale(2.0);
    assert!((h.area() - before * 4.0).abs() < 1e-9);
}

// ─────────────────────────────────────────────────────────────────────────
// INV-2 / ShapeKind: the generated discriminant enum is Copy + Clone + Eq +
// Debug and repr(u8). Exhaustive variant set is exactly {Circle, Square}.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn shape_kind_enum_has_documented_derives_and_variant_set() {
    let k1 = ShapeKind::Circle;
    let k2 = ShapeKind::Square;

    // Copy + Clone.
    let k1b = k1;
    let _ = k1.clone();
    assert_eq!(k1, k1b);

    // Eq / PartialEq.
    assert_eq!(k1, ShapeKind::Circle);
    assert_ne!(k1, k2);
    assert_ne!(ShapeKind::Circle, ShapeKind::Square);

    // Debug format is the variant name (derive(Debug) default).
    assert_eq!(format!("{:?}", ShapeKind::Circle), "Circle");
    assert_eq!(format!("{:?}", ShapeKind::Square), "Square");

    // match is exhaustive over the closed set; this doubles as a compile-time
    // guarantee that no third variant exists.
    let all: Vec<ShapeKind> = vec![ShapeKind::Circle, ShapeKind::Square];
    let names: Vec<&'static str> =
        all.iter().map(|k| match *k {
            ShapeKind::Circle => "circle",
            ShapeKind::Square => "square",
        }).collect();
    assert_eq!(names, vec!["circle", "square"]);
}

// ─────────────────────────────────────────────────────────────────────────
// INV-5 / ergonomics: `new` accepts a `*mut T` for any `T: ?Sized` matching
// the variant; the macro casts it to `*mut ()` internally. Dispatch through
// a handle built from a reborrowed/different-shaped pointer is still correct
// as long as it points at the right concrete type.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn new_casts_any_sized_ptr_and_dispatch_is_stable_across_calls() {
    let mut c = CircleT { r: 2.5 };
    let ptr: *mut CircleT = &raw mut c;
    let h = unsafe { Shape::new(ShapeKind::Circle, ptr) };

    // Repeated dispatch yields identical results (no hidden state mutation
    // for pure getters).
    let a1 = h.area();
    let a2 = h.area();
    let a3 = h.area();
    assert!((a1 - a2).abs() < 1e-12);
    assert!((a2 - a3).abs() < 1e-12);
    assert_eq!(h.name(), h.name());

    // Owner field is exactly the cast of the input pointer.
    assert_eq!(h.owner, ptr as *mut ());
}

// ─────────────────────────────────────────────────────────────────────────
// Closed-set contract: every method is reachable on every variant. Iterating
// the variant set × method set and checking a non-panic dispatch exercises
// INV-4 (a missing method would have been a *link error* at build time; this
// test pins the runtime side and guards against accidental future removal).
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn all_variants_dispatch_all_declared_methods() {
    let variants: [ShapeKind; 2] = [ShapeKind::Circle, ShapeKind::Square];
    for k in variants {
        // Build an owner appropriate to each kind; dispatch each method once.
        let mut c = CircleT { r: 1.0 };
        let mut sq = SquareT { s: 1.0 };
        let h = unsafe {
            match k {
                ShapeKind::Circle => Shape::new(k, &raw mut c),
                ShapeKind::Square => Shape::new(k, &raw mut sq),
            }
        };
        // area: f64
        let _ = h.area();
        // name: &'static str — must match the variant.
        let nm = match k {
            ShapeKind::Circle => "circle",
            ShapeKind::Square => "square",
        };
        assert_eq!(h.name(), nm);
        // scale: mutation, observed via area.
        let before = h.area();
        h.scale(2.0);
        assert!((h.area() - before * 4.0).abs() < 1e-9);
        // label: out-param, appends the variant name.
        let mut buf = String::new();
        h.label("", &mut buf);
        assert_eq!(buf, nm);
        // is() exhaustive cross-check.
        assert!(h.is(k));
        for other in variants {
            assert_eq!(h.is(other), other == k);
        }
    }
}
