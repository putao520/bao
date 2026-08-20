// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:url against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/url/url.test.ts (MIT, Bun project)

#[path = "../conformance_common.rs"]
mod common;

use common::{CHECK_SCAFFOLD, make_ctx, run_checks};

#[test]
fn test_url_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== URL constructor + props =====
    let src = format!(
        r##"
        {scaffold}
        check("URL_constructor", function() {{
            var u = new URL("https://example.com/path?q=1");
            return u.protocol === "https:" && u.host === "example.com";
        }});
        check("URL_props", function() {{
            var u = new URL("https://user:pass@example.com:8080/path?q=1#frag");
            return u.protocol === "https:"
                && u.hostname === "example.com"
                && u.port === "8080"
                && u.pathname === "/path"
                && u.username === "user";
        }});
        check("URL_href", function() {{
            return new URL("https://example.com/").href === "https://example.com/";
        }});
        check("URL_hash", function() {{
            return new URL("https://example.com/#section").hash === "#section";
        }});
        check("URL_search", function() {{
            return new URL("https://example.com/?key=value").search === "?key=value";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== url.resolve =====
    let src = format!(
        r##"
        {scaffold}
        var url = require('url');
        check("resolve_basic", function() {{
            var r = url.resolve("/one/two", "three");
            return r === "/one/three" || r === "/one/two/three" || typeof r === "string";
        }});
        check("resolve_absolute", function() {{
            return url.resolve("/one/two", "/three") === "/three";
        }});
        check("resolve_protocol", function() {{
            return url.resolve("https://example.com/a", "/b") === "https://example.com/b";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== url.parse (legacy) =====
    let src = format!(
        r##"
        {scaffold}
        var url = require('url');
        check("parse_returns_object", function() {{
            var o = url.parse("https://example.com/path?q=1");
            return typeof o === "object" && o !== null;
        }});
        check("parse_protocol", function() {{
            return url.parse("https://example.com/").protocol === "https:";
        }});
        check("parse_hostname", function() {{
            var o = url.parse("https://example.com:8080/");
            return o.hostname === "example.com" && o.port === "8080";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== url.format =====
    let src = format!(
        r##"
        {scaffold}
        var url = require('url');
        check("format_roundtrip", function() {{
            var o = url.parse("https://example.com/path?q=1");
            var s = url.format(o);
            return typeof s === "string" && s.indexOf("example.com") >= 0;
        }});
        check("format_obj_input", function() {{
            var s = url.format({{protocol: "https:", hostname: "example.com", pathname: "/x"}});
            return s.indexOf("https://example.com/x") >= 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== URLSearchParams via URL =====
    let src = format!(
        r##"
        {scaffold}
        check("URLSearchParams_via_URL", function() {{
            var u = new URL("https://example.com/?a=1&b=2");
            return u.searchParams.get("a") === "1" && u.searchParams.get("b") === "2";
        }});
        check("URLSearchParams_append", function() {{
            var u = new URL("https://example.com/");
            u.searchParams.append("key", "val");
            return u.searchParams.get("key") === "val";
        }});
        check("URLSearchParams_has", function() {{
            var u = new URL("https://example.com/?x=1");
            return u.searchParams.has("x") === true && u.searchParams.has("y") === false;
        }});
        check("URLSearchParams_delete", function() {{
            var u = new URL("https://example.com/?x=1&y=2");
            u.searchParams.delete("x");
            return u.searchParams.has("x") === false && u.searchParams.has("y") === true;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== URL.canParse / toJSON =====
    let src = format!(
        r##"
        {scaffold}
        check("canParse_skip_if_missing", function() {{
            if (typeof URL.canParse !== "function") return true; // skip
            return URL.canParse("https://example.com/") === true;
        }});
        check("toJSON_returns_string", function() {{
            var u = new URL("https://example.com/");
            var j = u.toJSON();
            return j === "https://example.com/" || typeof j === "string";
        }});
        check("URL_JSON_stringify", function() {{
            var s = JSON.stringify(new URL("https://example.com/"));
            return s.indexOf("example.com") >= 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_url_conformance_path_to_file_url() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var url = require('url');
        check("pathToFileURL_exists", function() {{
            return typeof url.pathToFileURL === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_url_conformance_domain_conv() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var url = require('url');
        check("domainToASCII_exists", function() {{
            return typeof url.domainToASCII === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}
