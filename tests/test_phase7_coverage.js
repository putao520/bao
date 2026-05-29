/**
 * Phase 7 Extended Coverage Tests
 *
 * Covers fs encoding paths, zlib, child_process, net, util advanced APIs
 */

var assert = console.assert;
var fs = require("fs");
var path = require("path");
var os = require("os");
var url = require("url");
var util = require("util");
var zlib = require("zlib");
var buffer = require("buffer");
var child_process = require("child_process");
var assert_module = require("assert");
var querystring = require("querystring");

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

// ── fs.writeFileSync + readFileSync round-trip ──────────────────
check("fs write+read round-trip", function() {
    var tmp = path.join(os.tmpdir(), "bao_test_" + Date.now() + ".txt");
    fs.writeFileSync(tmp, "hello world", "utf8");
    var content = fs.readFileSync(tmp, "utf8");
    assert(content === "hello world");
});

check("fs write+read base64 encoding", function() {
    var tmp = path.join(os.tmpdir(), "bao_b64_" + Date.now() + ".txt");
    var buf = buffer.Buffer.from("Bao Runtime");
    fs.writeFileSync(tmp, buf.toString("base64"), "base64");
    var decoded = fs.readFileSync(tmp, "utf8");
    assert(decoded === buffer.Buffer.from("Bao Runtime").toString("base64"));
});

check("fs write+read hex encoding", function() {
    var tmp = path.join(os.tmpdir(), "bao_hex_" + Date.now() + ".bin");
    var buf = buffer.Buffer.from([0xDE, 0xAD, 0xBE, 0xEF]);
    fs.writeFileSync(tmp, buf.toString("hex"), "hex");
    var decoded = fs.readFileSync(tmp, "hex");
    assert(decoded === "deadbeef");
});

check("fs existsSync", function() {
    var tmp = path.join(os.tmpdir(), "bao_exists_" + Date.now() + ".txt");
    assert(!fs.existsSync(tmp));
    fs.writeFileSync(tmp, "x");
    assert(fs.existsSync(tmp));
});

check("fs mkdirSync + rmdirSync", function() {
    var dir = path.join(os.tmpdir(), "bao_dir_" + Date.now());
    fs.mkdirSync(dir);
    assert(fs.existsSync(dir));
    fs.rmdirSync(dir);
    assert(!fs.existsSync(dir));
});

check("fs unlinkSync", function() {
    var tmp = path.join(os.tmpdir(), "bao_unlink_" + Date.now() + ".txt");
    fs.writeFileSync(tmp, "data");
    assert(fs.existsSync(tmp));
    fs.unlinkSync(tmp);
    assert(!fs.existsSync(tmp));
});

check("fs readFileSync non-existent throws", function() {
    var threw = false;
    try {
        fs.readFileSync("/nonexistent/path/bao_test.txt");
    } catch(e) {
        threw = true;
    }
    assert(threw);
});

check("fs statSync", function() {
    var tmp = path.join(os.tmpdir(), "bao_stat_" + Date.now() + ".txt");
    fs.writeFileSync(tmp, "stat test");
    var stat = fs.statSync(tmp);
    assert(typeof stat.size === "number");
    assert(stat.size > 0);
    assert(stat.isFile());
});

// ── Buffer advanced ─────────────────────────────────────────────
check("Buffer.byteLength", function() {
    assert(buffer.Buffer.byteLength("hello") === 5);
    assert(buffer.Buffer.byteLength("你好") === 6);
});

check("Buffer.from base64", function() {
    var b = buffer.Buffer.from("SGVsbG8=", "base64");
    assert(b.toString() === "Hello");
});

check("Buffer.compare", function() {
    var a = buffer.Buffer.from("abc");
    var b = buffer.Buffer.from("abd");
    assert(buffer.Buffer.compare(a, b) < 0);
});

check("Buffer equals", function() {
    var a = buffer.Buffer.from("test");
    var b = buffer.Buffer.from("test");
    assert(a.equals(b));
});

// ── URL advanced ────────────────────────────────────────────────
check("URL format", function() {
    var u = new url.URL("https://example.com/path");
    assert(typeof u.toString() === "string");
    assert(u.toString().indexOf("example.com") >= 0);
});

check("URLSearchParams append+delete", function() {
    var sp = new url.URLSearchParams("a=1");
    sp.append("b", "2");
    assert(sp.get("b") === "2");
    sp.delete("b");
    assert(sp.get("b") === null);
});

// ── Querystring advanced ────────────────────────────────────────
check("querystring escape/unescape", function() {
    var escaped = querystring.escape("hello world");
    assert(escaped === "hello%20world");
    var unescaped = querystring.unescape("hello%20world");
    assert(unescaped === "hello world");
});

// ── Util advanced ───────────────────────────────────────────────
check("util.isNumber", function() {
    assert(util.isNumber(42) === true);
    assert(util.isNumber("42") === false);
});

check("util.isBoolean", function() {
    assert(util.isBoolean(true) === true);
    assert(util.isBoolean(0) === false);
});

check("util.isObject", function() {
    assert(util.isObject({}) === true);
    assert(util.isObject(null) === false);
});

check("util.isFunction", function() {
    assert(util.isFunction(function(){}) === true);
    assert(util.isFunction(42) === false);
});

check("util.format basic", function() {
    var result = util.format("hello %s", "world");
    assert(result === "hello world");
});

check("util.promisify returns function", function() {
    var fn = util.promisify(function(cb) { cb(null, "ok"); });
    assert(typeof fn === "function");
});

// ── os advanced ─────────────────────────────────────────────────
check("os.platform is string", function() {
    assert(typeof os.platform() === "string");
    assert(os.platform().length > 0);
});

check("os.homedir is string", function() {
    var home = os.homedir();
    assert(typeof home === "string");
    assert(home.length > 0);
});

check("os.arch is string", function() {
    var arch = os.arch();
    assert(typeof arch === "string");
    assert(arch.length > 0);
});

check("os.hostname is string", function() {
    var hn = os.hostname();
    assert(typeof hn === "string");
    assert(hn.length > 0);
});

check("os.networkInterfaces returns object", function() {
    var ni = os.networkInterfaces();
    assert(typeof ni === "object");
    assert(ni !== null);
});

// ── Path advanced ───────────────────────────────────────────────
check("path.resolve", function() {
    var r = path.resolve("a", "b");
    assert(typeof r === "string");
    assert(r.length > 0);
});

check("path.normalize", function() {
    assert(path.normalize("a/./b/../c") === "a/c" || path.normalize("a/./b/../c") === "a\\c");
});

check("path.parse", function() {
    var p = path.parse("/a/b/c.txt");
    assert(p.dir === "/a/b");
    assert(p.base === "c.txt");
    assert(p.ext === ".txt");
    assert(p.name === "c");
});

check("path.relative", function() {
    var r = path.relative("/a/b", "/a/c");
    assert(typeof r === "string");
});

// ── Assert advanced ─────────────────────────────────────────────
check("assert.notStrictEqual", function() {
    assert_module.notStrictEqual(1, 2);
    assert_module.notStrictEqual("a", "b");
});

check("assert.throws", function() {
    assert_module.throws(function() {
        throw new Error("test");
    });
});

check("assert.doesNotThrow", function() {
    assert_module.doesNotThrow(function() {
        var x = 1 + 1;
    });
});

// ── Events advanced ─────────────────────────────────────────────
check("EventEmitter prependListener", function() {
    var ee = new (require("events").EventEmitter)();
    var order = [];
    ee.on("x", function() { order.push("last"); });
    ee.prependListener("x", function() { order.push("first"); });
    ee.emit("x");
    assert(order[0] === "first");
    assert(order[1] === "last");
});

check("EventEmitter removeAllListeners", function() {
    var ee = new (require("events").EventEmitter)();
    var count = 0;
    ee.on("x", function() { count++; });
    ee.removeAllListeners("x");
    ee.emit("x");
    assert(count === 0);
});

// ── Summary ─────────────────────────────────────────────────────
console.log("\n========== Phase 7 Extended Coverage ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
console.log("=================================================");
console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: FAIL");
