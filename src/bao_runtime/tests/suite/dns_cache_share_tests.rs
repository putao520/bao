// @trace TEST-ENG-DNS-CACHE [req:REQ-ENG-007] [level:integration]
//
// Cross-path shared DNS cache (fusion: servo/hyper + usockets + node:dns →
// one per-host cache). Proves the `node:dns` lookup path consults the
// process-wide cache: an address that only exists because this test primed
// the cache (the host is synthetic and unresolvable by any system resolver)
// comes back through `dns.lookup`.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

#[test]
fn test_dns_lookup_reads_shared_cache() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Prime the shared cache the way any of the three resolution paths would
    // after resolving this (synthetic, otherwise unresolvable) host.
    bun_dns::cache::insert(
        b"cache-wired.test",
        vec![bun_dns::cache::IpAddr::V4([9, 9, 9, 9])],
        Some(60),
    );

    let addr = match ctx.eval(
        "require('dns').lookup('cache-wired.test').address",
        "<test>",
    ) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        _ => String::new(),
    };
    assert_eq!(
        addr, "9.9.9.9",
        "dns.lookup must serve the shared cache entry"
    );

    // The lookup itself now feeds the cache too: a miss (fresh host) resolves
    // via getaddrinfo and lands in the cache, so the usockets/servo paths see
    // it. `localhost` always resolves; assert via the cache directly.
    let _ = match ctx.eval("require('dns').lookup('localhost').address", "<test>") {
        Ok(JsValue::String(s)) => s,
        _ => String::new(),
    };
    assert!(
        bun_dns::cache::lookup(b"localhost").is_some(),
        "dns.lookup miss path must insert into the shared cache"
    );
}
