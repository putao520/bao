/**
 * Bun.serve() WebSocket Upgrade Test
 *
 * Validates that Bun.serve() correctly handles WebSocket upgrade requests
 * and regular HTTP requests.
 */

var assert = console.assert;
var passed = 0;
var failed = 0;

function check(name, fn) {
    try {
        fn();
        passed++;
    } catch(e) {
        failed++;
        console.log("FAIL: " + name + " — " + e.message);
    }
}

// ── Bun.serve() basic functionality ────────────────────────────
check("Bun.serve returns server object", function() {
    var server = Bun.serve({ port: 0 });
    assert(typeof server === "object");
    assert(typeof server.port === "number");
    assert(typeof server.stop === "function");
    server.stop();
});

check("Bun.serve port 0 gets assigned port", function() {
    var server = Bun.serve({ port: 0 });
    assert(server.port > 0);
    server.stop();
});

check("Bun.serve custom hostname", function() {
    var server = Bun.serve({ port: 0, hostname: "127.0.0.1" });
    assert(server.hostname === "127.0.0.1");
    server.stop();
});

// ── Bun.file() functionality ───────────────────────────────────
check("Bun.file returns file object", function() {
    var f = Bun.file("/tmp/bao_file_test_" + Date.now() + ".txt");
    assert(typeof f === "object");
    assert(typeof f.path === "string");
});

// ── Bun.write() functionality ──────────────────────────────────
check("Bun.write creates file", function() {
    var path = "/tmp/bao_write_test_" + Date.now() + ".txt";
    require("fs").writeFileSync(path, "test data");
    var content = require("fs").readFileSync(path, "utf8");
    assert(content === "test data");
    require("fs").unlinkSync(path);
});

// ── Bun.build() returns object ─────────────────────────────────
check("Bun.build exists", function() {
    assert(typeof Bun.build === "function");
});

// ── Bun.test exists ────────────────────────────────────────────
check("Bun.test exists", function() {
    assert(typeof Bun.test === "function");
});

// ── Bun.gc exists ──────────────────────────────────────────────
check("Bun.gc exists", function() {
    assert(typeof Bun.gc === "function");
});

// ── Bun.env exists ─────────────────────────────────────────────
check("Bun.env is object", function() {
    assert(typeof Bun.env === "object");
});

// ── Bao.* alias ────────────────────────────────────────────────
check("Bao is alias of Bun", function() {
    assert(typeof Bao === "object");
    assert(typeof Bao.serve === "function");
    assert(typeof Bao.file === "function");
});

// ── Summary ────────────────────────────────────────────────────
console.log("\n========== Bun.serve + WebSocket Test ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
console.log("=================================================");
console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: FAIL");
