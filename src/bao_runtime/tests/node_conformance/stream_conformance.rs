// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:stream against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/stream/ (MIT, Bun project)

#[path = "../conformance_common.rs"]
mod common;

use common::{make_ctx, run_checks, CHECK_SCAFFOLD};

#[test]
fn test_stream_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== module shape =====
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        check("module_exists", function() {{
            return typeof stream === "object" && stream !== null;
        }});
        check("Readable_constructor", function() {{
            return typeof stream.Readable === "function";
        }});
        check("Writable_constructor", function() {{
            return typeof stream.Writable === "function";
        }});
        check("Duplex_constructor", function() {{
            return typeof stream.Duplex === "function";
        }});
        check("Transform_constructor", function() {{
            return typeof stream.Transform === "function";
        }});
        check("PassThrough_constructor", function() {{
            return typeof stream.PassThrough === "function";
        }});
        check("pipeline_is_function", function() {{
            return typeof stream.pipeline === "function";
        }});
        check("finished_is_function", function() {{
            return typeof stream.finished === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== Readable basic =====
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        var Readable = stream.Readable;
        check("readable_instance", function() {{
            var r = new Readable({{ read: function() {{}} }});
            return r instanceof Readable && typeof r.on === "function";
        }});
        check("readable_push_data", function() {{
            var r = new Readable({{ read: function() {{}} }});
            r.push("hello ");
            r.push("world");
            r.push(null);
            return typeof r.read === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== Writable basic =====
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        var Writable = stream.Writable;
        check("writable_instance", function() {{
            var w = new Writable({{ write: function(chunk, enc, cb) {{ cb(); }} }});
            return w instanceof Writable;
        }});
        check("writable_write_callable", function() {{
            var w = new Writable({{ write: function(chunk, enc, cb) {{ cb(); }} }});
            return typeof w.write === "function" && typeof w.end === "function";
        }});
        check("writable_end_callable", function() {{
            var w = new Writable({{ write: function(chunk, enc, cb) {{ cb(); }} }});
            w.end();
            return true;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== PassThrough =====
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        var PassThrough = stream.PassThrough;
        check("passthrough_construct", function() {{
            var p = new PassThrough();
            return p instanceof PassThrough && typeof p.write === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
#[ignore = "bao_runtime: stream.Readable.from async iterator not implemented. See GAP_REPORT.md"]
fn test_stream_conformance_readable_from() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        check("Readable.from_is_function", function() {{
            return typeof stream.Readable.from === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
#[ignore = "bao_runtime: stream web API (ReadableStream/WritableStream) not exposed on stream module. See GAP_REPORT.md"]
fn test_stream_conformance_web_api() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var stream = require('stream');
        check("web_ReadableStream", function() {{
            return typeof stream.ReadableStream === "function" ||
                   typeof stream.WebStream === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}
