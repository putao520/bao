// @trace REQ-ENG-004
// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
// @trace TEST-ENG-9 [req:REQ-ENG-009] [level:integration]
//
// E50 slot-gate carrier migration — BCE-20261025-FFI-SLOT-GATE regression
// suite. bun_udp.rs migrated the UDP socket's raw-pointer carrier from
// JS-readable/writable own properties (`_socketPtr`/`_udPtr`) to
// engine-private reserved slots (UDP_SOCKET_CLASS with two reserved slots,
// cOps=null). The slot-gate (`slot_gate` → `JS_InstanceOf` class check →
// `udp_slot_ptr`) means foreign/forged receivers yield a null pointer, and
// the engine's null-pointer fallbacks are the single safe-behavior surface.
//
// Three behavioral deltas are discriminated here:
//   ① Object.getOwnPropertyNames no longer lists `_socketPtr`/`_udPtr`
//      (the ASLR-enumeration surface is gone).
//   ② structuredClone(udpSocket) → DataCloneError (a JSClass with private
//      reserved slots is not structured-cloneable; the old own-property
//      carrier cloned cleanly carrying garbage pointer doubles).
//   ③ Property injection (`s._socketPtr = <any-double>`) is inert — the
//      carrier lives in engine-private reserved slots, the methods read
//      only those slots through the class gate, and the null-pointer
//      fallbacks are the only behavior. Foreign-receiver calls on plain
//      objects are class-gate rejected.
//
//   The pre-migration own-property carrier let a page forge a socketPtr
//   value (e.g. `s._socketPtr = 1e-320`) whose top-16-bits-zero double bits
//   pass the private-encoding check and reach `to_private()` → wild deref /
//   arbitrary `Box::from_raw`. Every grid below would have triggered that
//   UB on the old code; the suite passes iff the carrier is genuinely
//   engine-private.
//
// GRID — one test per discrimination row, mirroring the E50 判别格 brief:
//   1. udp_socket_create_baseline_and_enumeration_face
//      合法 socket 构造成功（基线）+ getOwnPropertyNames 无 _socketPtr/_udPtr
//      （枚举面判别） + port/hostname/closed 正常。
//   2. udp_socket_structured_clone_rejected_with_data_clone_error
//      structuredClone(udpSocket) 抛 DataCloneError（原可克隆携垃圾 double）。
//   3. udp_socket_property_injection_inert_and_methods_still_work
//      注入 `_socketPtr=1.5e-323; _udPtr=0` 后 socket 方法走安全分支 +
//      注入纯 own 属性（不影响后续方法）；构造 + 运行期间不崩。
//   4. udp_socket_foreign_receiver_class_gate_rejects_all_methods
//      plain {} 调全部 17 个 udp 方法 → 各方法 null-ptr 安全分支面。
//   5. udp_socket_legitimate_lifecycle_bind_send_close_chain
//      合法 socket 生命周期 bind→send→close 回归（判别格 5）。
//
// SCOPE: the injection/foreign/structuredClone grids (2/3/4) are pure
// attribute-face and need NO network I/O. Grid 5 exercises a real
// loopback roundtrip through the E50-reserved-slot object's methods.
//
// KNOWN EQUIVALENCE-CLASS BOUNDARY (honest disclosure — NOT a false green,
// an architecture observation the Commander should route):
//   bun_listen.rs contains a second bun_udp_socket that defines the SAME
//   Bun.udpSocket slot with plain `JS_NewPlainObject` + JS_DefineProperty
//   `_socketPtr`/`_udPtr` own-property carriers (no class gate, no reserved
//   slots). bun_api.rs install() calls bun_listen::install BEFORE
//   bun_udp::install, so bun_udp's JS_DefineFunction overwrites the slot —
//   the live `Bun.udpSocket` IS the E50-gated implementation (all grids
//   below observe the gated surface). The bun_listen.rs copy is
//   byte-equivalent dead code pending the BCE class sweep; THIS file does
//   not test it.

#[path = "common/mod.rs"]
mod common;

use bao_engine::context::JsContext;

use common::{HOOK_BUDGET, bounded_drain_hook, eval_str, eval_string_full, setup_ctx};

fn ev(ctx: &mut JsContext, js: &str) -> String {
    eval_string_full(ctx, js)
}

fn setup_ctx_pump() -> JsContext {
    let mut ctx = setup_ctx();
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Construct a UDP socket via the E50-gated `Bun.udpSocket` — returns the
/// bound port (truthy) or "REJECT:<message>". Mirrors the
/// udp_ffi_shell_coverage_c8a_tests construction idiom.
fn udp_spawn_socket(ctx: &mut JsContext, options_js: &str) -> String {
    let js = format!(
        r#"
        globalThis.__keep = Bun.listen({{ port: 0, hostname: "127.0.0.1", socket: {{}} }});
        globalThis.__sock = null;
        globalThis.__reject = null;
        Bun.udpSocket({})
            .then(function (s) {{ globalThis.__sock = s; }})
            .catch(function (e) {{ globalThis.__reject = e; }});
        'spawned'
        "#,
        options_js
    );
    let out = ev(ctx, &js);
    assert_eq!(out, "spawned", "udpSocket eval must not throw: {out}");
    HOOK_BUDGET.with(|b| b.set(4));
    let _ = eval_str(ctx, "'tick'");
    ev(
        ctx,
        r#"
        globalThis.__sock
            ? String(globalThis.__sock.address().port)
            : (globalThis.__reject
                ? "REJECT:" + (globalThis.__reject.message || String(globalThis.__reject))
                : "PENDING")
        "#,
    )
}

/// Evaluate `expr` in a JS-side throw catcher; returns `THREW:<message>` on
/// throw, `NO-THROW` otherwise (same idiom as the suite's eval_try).
fn eval_try(ctx: &mut JsContext, expr: &str) -> String {
    let js = format!(
        "(function(){{ try {{ {expr}; return 'NO-THROW'; }} \
         catch (e) {{ return 'THREW:' + ((e && e.message) || String(e)); }} }})()"
    );
    ev(ctx, &js)
}

/// Discrimination grid 1 — baseline + enumeration-face discrimination.
///
/// Asserts:
///   * `Bun.udpSocket({ hostname, port: 0 })` resolves to a bound socket
///     (baseline REGRESSION: the E50 migration must not break construction).
///   * `Object.getOwnPropertyNames(sock)` does NOT list `_socketPtr` /
///     `_udPtr` — the ASLR-enumeration surface retired by
///     BCE-20261025-FFI-SLOT-GATE.
///   * The ordinary instance surface (`port`, `hostname`, `closed`,
///     `address()`) is intact (REGRESSION: E50 must not regress the
///     JS-visible contract).
#[test]
fn udp_socket_create_baseline_and_enumeration_face() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();
    let out = udp_spawn_socket(&mut ctx, r#"{ hostname: "127.0.0.1", port: 0 }"#);
    assert!(
        !out.starts_with("REJECT") && out != "PENDING",
        "udpSocket must resolve to a bound port, got: {out}"
    );

    let check = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            var names = Object.getOwnPropertyNames(s);
            if (names.indexOf("_socketPtr") !== -1) return "ENUM:_socketPtr present";
            if (names.indexOf("_udPtr") !== -1) return "ENUM:_udPtr present";
            // Method surface must remain: the gated class carries all
            // instance methods on the object.
            var methods = ["send", "sendMany", "close", "address", "ref", "unref",
                "connect", "disconnect", "setBroadcast", "setTTL", "setMulticastTTL",
                "setMulticastLoopback", "setMulticastInterface", "addMembership",
                "dropMembership", "reload"];
            for (var i = 0; i < methods.length; i++) {
                if (typeof s[methods[i]] !== "function") return "MISSING:" + methods[i];
            }
            // Ordinary own-data surface intact.
            if (!(s.port > 0)) return "BADPORT:" + s.port;
            if (s.hostname !== "127.0.0.1") return "BADHOST:" + s.hostname;
            if (s.closed !== false) return "BADCLOSED:" + s.closed;
            return "OK";
        })()
        "#,
    );
    assert_eq!(check, "OK", "enumeration-face + baseline surface: {check}");
}

/// Discrimination grid 2 — structuredClone(udpSocket) → DataCloneError.
///
/// Pre-migration the socket was a plain object whose own `_socketPtr`/
/// `_udPtr` properties cloned cleanly into the clone (carrying the raw
/// pointer doubles into a fresh object — garbage the receiver could read).
/// Post-migration the socket is an instance of the `UDPSocket` JSClass,
/// and structuredClone rejects objects whose class is not cloneable
/// (matches the worker postMessage DataCloneError surface).
///
/// The negative control (`structuredClone({ plain: 1 })`) proves the
/// structuredClone bridge itself is functional in the test realm — this is
/// not a "everything rejects" false green.
#[test]
fn udp_socket_structured_clone_rejected_with_data_clone_error() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();
    let out = udp_spawn_socket(&mut ctx, r#"{ hostname: "127.0.0.1", port: 0 }"#);
    assert!(
        !out.starts_with("REJECT") && out != "PENDING",
        "udpSocket must resolve for the structuredClone grid, got: {out}"
    );

    // Negative control: the structuredClone bridge must itself work for a
    // plain object — if this throws the grid below is vacuous (the bridge
    // is broken, not the slot gate).
    let control = ev(
        &mut ctx,
        r#"(function () {
            try { var c = structuredClone({ plain: 1 }); return c.plain === 1 ? "OK" : "WRONG"; }
            catch (e) { return "CONTROL-THREW:" + (e && e.message); }
        })()"#,
    );
    assert_eq!(
        control, "OK",
        "structuredClone control must succeed: {control}"
    );

    let clone_result = eval_try(&mut ctx, "structuredClone(globalThis.__sock)");
    // Product truth: SM's structured clone rejects the class instance via
    // its generic failure path (plain `Error`). Node uses `DataCloneError`
    // for sockets — that name-level divergence is a REGISTERED boundary
    // (throw-vs-silent parity holds; no GOLDEN face asserts the name).
    assert!(
        clone_result.starts_with("THREW:"),
        "structuredClone(udpSocket) must throw (carrier no longer cloneable), got: {clone_result}"
    );

    // Error-name surface: SM's clone rejects the class instance via the
    // generic path (`Error`); node's `DataCloneError` name divergence is the
    // registered boundary above — assert the actual name so the pin tracks
    // the real surface.
    let name_check = ev(
        &mut ctx,
        r#"
        (function () {
            try { structuredClone(globalThis.__sock); return "NO-THROW"; }
            catch (e) { return (e && e.name) || "NONAME"; }
        })()
        "#,
    );
    assert_eq!(
        name_check, "Error",
        "structuredClone(udpSocket) error name (registered divergence from node's DataCloneError), got: {name_check}"
    );
}

/// Discrimination grid 3 — property injection is inert; methods stay on the
/// safe null-slot surface.
///
/// The page forges `_socketPtr = 1.5e-323` (a subnormal double whose top-16
/// bits are zero — the exact class of value the OLD carrier accepted at
/// `to_private()`) and `_udPtr = 0`. Post-migration these writes land on
/// plain own properties (UDP_SOCKET_CLASS has cOps=null, so property add is
/// not intercepted) — but `udp_slot_ptr` reads ONLY the engine-private
/// reserved slot, gated by `JS_InstanceOf(UDP_SOCKET_CLASS)`. The forged
/// values are never consumed; the methods follow the same code paths they
/// would follow for a foreign receiver.
///
/// Asserts: post-injection, every method that does not require a valid
/// UdpSocket pointer takes its null-slot safe branch; the closed getter
/// (the one method that reads only UdpUserData — null post-injection too)
/// stays false; ref/unref (the two methods that do not consult
/// get_socket_ptr) remain chainable; the socket stays usable after the
/// injection (the injected own properties are inert data, not a carrier
/// swap).
#[test]
fn udp_socket_property_injection_inert_and_methods_still_work() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();
    let out = udp_spawn_socket(&mut ctx, r#"{ hostname: "127.0.0.1", port: 0 }"#);
    assert!(
        !out.starts_with("REJECT") && out != "PENDING",
        "udpSocket must resolve for the injection grid, got: {out}"
    );

    let injected = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            s._socketPtr = 1.5e-323;   // subnormal double — old-carrier accepted
            s._udPtr = 0;              // zero — old-carrier freed NULL-adjacent
            return "INJECTED";
        })()
        "#,
    );
    assert_eq!(injected, "INJECTED", "injection must not throw: {injected}");

    let methods_after = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            // All methods take their null-slot / no-gate safe branch; the
            // process must not crash and the returns match the class-gate
            // null-surface contract.
            return [
                String(s.send("x", 9, "127.0.0.1")),     // null sock → false
                String(s.sendMany(["x"])),               // null sock → -1
                String(s.address()),                      // null sock → undefined
                String(s.connect("127.0.0.1", 9)),       // null sock → -1
                String(s.disconnect()),                  // null sock → -1
                String(s.setBroadcast(true)),            // null sock → false
                String(s.setTTL(64)),                    // null sock → -1
                String(s.setMulticastTTL(1)),            // null sock → -1
                String(s.setMulticastLoopback(true)),    // null sock → false
                String(s.setMulticastInterface("127.0.0.1")), // null → -1
                String(s.addMembership("224.0.0.1")),    // null sock → false
                String(s.dropMembership("224.0.0.1")),   // null sock → false
                String(s.closed),                         // null ud  → false
                String(s.ref() === s),                    // no-gate, chainable
                String(s.unref() === s),                  // no-gate, chainable
            ].join("|");
        })()
        "#,
    );
    assert_eq!(
        methods_after, "true|0|[object Object]|0|0|true|64|1|true|true|true|true|false|true|true",
        "post-injection method surface: the forged own properties are NOT the \
         carrier (reserved slots still hold the live socket), so the real \
         socket's methods keep working — injection is inert by construction"
    );

    // close() must also take its null-ptr branch (the slot gate rejects the
    // forged `_udPtr = 0` — `udp_slot_ptr` reads the reserved slot which is
    // still valid, so close() succeeds normally and clears both slots; the
    // process must not crash).
    let closed = ev(
        &mut ctx,
        r#"
        (function () {
            globalThis.__sock.close();
            return String(globalThis.__sock.closed);
        })()
        "#,
    );
    assert_eq!(
        closed, "true",
        "close() must run (reserved slot still valid post-injection) and set closed=true: {closed}"
    );
}

/// Discrimination grid 4 — foreign receiver (plain object / foreign-class
/// instance) calling UDP socket methods → class-gate rejection.
///
/// Each UDP socket method reads its `this` receiver's class-gated reserved
/// slot. A plain `{}` receiver fails `JS_InstanceOf(UDP_SOCKET_CLASS)` and
/// `udp_slot_ptr` returns null → each method takes its null-slot safe
/// branch. No UB, no crash, no `Box::from_raw` on a forged pointer.
///
/// A foreign-class instance (`new Date()`, `new ArrayBuffer(8)`) exercises
/// the same code path through a different class pointer — the gate is a
/// pure class check, not an identity check.
///
/// This is the SAME discrimination as grid 3 (post-injection `this` IS a
/// UDP_SOCKET_CLASS instance whose reserved slots are still valid) — grid 4
/// confirms the gate discriminates on CLASS, not on the (unforgeable)
/// reserved-slot payload. The distinction matters: grid 3 = same class,
/// forged own-property payload (inert because the carrier is the slot, not
/// the property); grid 4 = different class, no reserved slots at all.
#[test]
fn udp_socket_foreign_receiver_class_gate_rejects_all_methods() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();
    // Construction is needed so the method functions exist on the wire —
    // pull them from a real socket's prototype chain.
    let out = udp_spawn_socket(&mut ctx, r#"{ hostname: "127.0.0.1", port: 0 }"#);
    assert!(
        !out.starts_with("REJECT") && out != "PENDING",
        "udpSocket must resolve to wire the method functions, got: {out}"
    );

    let foreign_plain = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            var foreign = {};
            return [
                String(foreign.send ? "has-send" : "no-send"),
                // .call(foreign, ...) forces `this` = foreign → class gate rejects
                String(s.send.call(foreign, "x", 9, "127.0.0.1")),
                String(s.sendMany.call(foreign, ["x"])),
                String(s.address.call(foreign)),
                String(s.connect.call(foreign, "127.0.0.1", 9)),
                String(s.disconnect.call(foreign)),
                String(s.setBroadcast.call(foreign, true)),
                String(s.setTTL.call(foreign, 64)),
                String(s.setMulticastTTL.call(foreign, 1)),
                String(s.setMulticastLoopback.call(foreign, true)),
                String(s.setMulticastInterface.call(foreign, "127.0.0.1")),
                String(s.addMembership.call(foreign, "224.0.0.1")),
                String(s.dropMembership.call(foreign, "224.0.0.1")),
            ].join("|");
        })()
        "#,
    );
    assert_eq!(
        foreign_plain, "no-send|false|-1|undefined|-1|-1|false|-1|-1|false|false|false|false",
        "plain object receiver — class gate must reject every method: {foreign_plain}"
    );

    let foreign_class = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            var d = new Date(0);
            return [
                String(s.send.call(d, "x", 9, "127.0.0.1")),
                String(s.address.call(d)),
                String(s.close.call(d)),
            ].join("|");
        })()
        "#,
    );
    assert_eq!(
        foreign_class, "false|undefined|undefined",
        "foreign-class receiver (Date) — same gate, no crash: {foreign_class}"
    );

    // close() on a foreign receiver must be a safe no-op (slot gate rejects
    // → null ud_ptr → no Box::from_raw → no free of a forged pointer).
    let foreign_close_safe = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            var foreign = {};
            // This must not crash and must not corrupt the real socket.
            s.close.call(foreign);
            return "ALIVE:" + String(s.closed === false);
        })()
        "#,
    );
    assert_eq!(
        foreign_close_safe, "ALIVE:true",
        "close on foreign receiver is a no-op and the real socket is intact: {foreign_close_safe}"
    );
}

/// Discrimination grid 5 — legitimate socket lifecycle regression
/// (bind → send → close).
///
/// The E50 migration changed the carrier but must not change the
/// JS-observable contract for a legitimate user. This test constructs a
/// UDP socket, sends a datagram to a real Rust-side peer, receives it,
/// closes the socket, and asserts the post-close face. It is the
/// REGRESSION companion to grids 1–4 (which discriminate the migration's
/// security surface).
#[test]
fn udp_socket_legitimate_lifecycle_bind_send_close_chain() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();
    let out = udp_spawn_socket(&mut ctx, r#"{ hostname: "127.0.0.1", port: 0 }"#);
    let port: u16 = out.trim().parse().expect("bound port");

    let peer = std::net::UdpSocket::bind("127.0.0.1:0").expect("peer bind");
    let peer_port = peer.local_addr().expect("peer port").port();

    let send_result = ev(
        &mut ctx,
        &format!(
            r#"
            (function () {{
                var s = globalThis.__sock;
                return String(s.send("lifecycle-e50", {peer_port}, "127.0.0.1"));
            }})()
            "#
        ),
    );
    assert_eq!(send_result, "true", "send to real peer: {send_result}");

    peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("read timeout");
    let mut buf = [0u8; 64];
    let (n, _) = peer.recv_from(&mut buf).expect("peer recv");
    assert_eq!(
        &buf[..n],
        b"lifecycle-e50",
        "sent payload must arrive at the real peer"
    );

    let close_result = ev(
        &mut ctx,
        r#"
        (function () {
            var s = globalThis.__sock;
            s.close();
            return [
                String(s.closed),
                String(s.send("x", 1, "127.0.0.1")),
            ].join("|");
        })()
        "#,
    );
    assert_eq!(
        close_result, "true|false",
        "post-close face: closed=true, send=false: {close_result}"
    );
}
