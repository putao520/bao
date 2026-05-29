// Phase 1 Integration Test — validates all implemented REQ
// Run with: bao run tests/phase1_integration.js

var passed = 0;
var failed = 0;
var errors = [];

function assert(cond, msg) {
  if (cond) {
    passed++;
  } else {
    failed++;
    errors.push("FAIL: " + msg);
  }
}

function assertEq(actual, expected, msg) {
  if (actual === expected) {
    passed++;
  } else {
    failed++;
    errors.push("FAIL: " + msg + " — expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual));
  }
}

// ============================================================
// REQ-ENG-001: SpiderMonkey Engine Integration
// ============================================================
assertEq(typeof globalThis, "object", "globalThis exists");
assertEq(typeof undefined, "undefined", "undefined type");
assertEq(typeof null, "object", "null type");
assertEq(typeof true, "boolean", "boolean type");
assertEq(typeof 42, "number", "number type");
assertEq(typeof "hello", "string", "string type");
assertEq(typeof function(){}, "function", "function type");
assertEq(typeof [], "object", "array type");
assertEq(typeof {}, "object", "object type");

// Arithmetic and control flow
assertEq(1 + 1, 2, "basic arithmetic");
assertEq(10 / 3 > 3.3 && 10 / 3 < 3.4, true, "floating point division");

// Exception handling
var caught = false;
try { throw new Error("test"); } catch(e) { caught = true; }
assert(caught, "try/catch works");

// ============================================================
// REQ-ENG-003: host_fn — console
// ============================================================
assertEq(typeof console, "object", "console object exists");
assertEq(typeof console.log, "function", "console.log is function");
assertEq(typeof console.error, "function", "console.error is function");
assertEq(typeof console.warn, "function", "console.warn is function");
assertEq(typeof console.info, "function", "console.info is function");
assertEq(typeof console.time, "function", "console.time is function");
assertEq(typeof console.timeEnd, "function", "console.timeEnd is function");

console.log("[PASS] console.* functions exist");

// ============================================================
// REQ-ENG-004: Event Loop — Timers
// ============================================================
assertEq(typeof setTimeout, "function", "setTimeout exists");
assertEq(typeof setInterval, "function", "setInterval exists");
assertEq(typeof clearTimeout, "function", "clearTimeout exists");
assertEq(typeof clearInterval, "function", "clearInterval exists");

var timerFired = false;
setTimeout(function() { timerFired = true; }, 10);
// Timer will fire after script ends

// ============================================================
// REQ-ENG-005: Module Loader — require()
// ============================================================
assertEq(typeof require, "function", "require() exists");

// Test node:fs
var fs = require("fs");
assert(fs !== undefined, "require('fs') returns value");
assertEq(typeof fs, "object", "fs is an object");

// Test node:path
var path = require("path");
assert(path !== undefined, "require('path') returns value");
assertEq(typeof path.join, "function", "path.join is function");
assertEq(path.join("a", "b", "c"), "a/b/c", "path.join works");

// Test node:os
var os = require("os");
assert(os !== undefined, "require('os') returns value");
assertEq(typeof os.platform, "function", "os.platform is function");
assert(typeof os.platform() === "string", "os.platform() returns string");

// Test node:crypto
var crypto = require("crypto");
assert(crypto !== undefined, "require('crypto') returns value");

// Test node:url
var url = require("url");
assert(url !== undefined, "require('url') returns value");

// Test node:events
var events = require("events");
assert(events !== undefined, "require('events') returns value");

// Test node:stream
var stream = require("stream");
assert(stream !== undefined, "require('stream') returns value");

// ============================================================
// fs.createReadStream / fs.createWriteStream
// ============================================================
var _crsTmp = "/tmp/bao_crs_test_" + Date.now() + ".txt";
fs.writeFileSync(_crsTmp, "read stream test");
var readStream = fs.createReadStream(_crsTmp, { encoding: "utf-8" });
assertEq(typeof readStream, "object", "fs.createReadStream returns object");
assertEq(readStream.readable, true, "ReadStream.readable is true");
assertEq(readStream.writable, false, "ReadStream.writable is false");
assertEq(readStream.path, _crsTmp, "ReadStream.path matches");

var writeStream = fs.createWriteStream(_crsTmp + ".out");
assertEq(typeof writeStream, "object", "fs.createWriteStream returns object");
assertEq(writeStream.readable, false, "WriteStream.readable is false");
assertEq(writeStream.writable, true, "WriteStream.writable is true");
assertEq(writeStream.path, _crsTmp + ".out", "WriteStream.path matches");
writeStream.write("test");
writeStream.end();
try { fs.unlinkSync(_crsTmp); } catch(e) {}
try { fs.unlinkSync(_crsTmp + ".out"); } catch(e) {}

// ============================================================
// REQ-ENG-006: Bun API
// ============================================================
assertEq(typeof Bun, "object", "Bun global exists");
assertEq(typeof Bao, "object", "Bao alias exists");
assertEq(Bun === Bao, true, "Bun === Bao (same object)");
assertEq(typeof Bun.version, "string", "Bun.version is string");
assertEq(typeof Bun.env, "object", "Bun.env is object");
assertEq(typeof Bun.file, "function", "Bun.file is function");
assertEq(typeof Bun.write, "function", "Bun.write is function");
assertEq(typeof Bun.readFile, "function", "Bun.readFile is function");
assertEq(typeof Bun.serve, "function", "Bun.serve is function");
assertEq(typeof Bun.spawn, "function", "Bun.spawn is function");
assertEq(typeof Bun.gc, "function", "Bun.gc is function");
assertEq(typeof Bun.sleep, "function", "Bun.sleep is function");

// Bun.write + Bun.readFile roundtrip
var testPath = "/tmp/bao_test_" + Date.now() + ".txt";
var testContent = "Hello from Bao!";
Bun.write(testPath, testContent);
var readBack = Bun.readFile(testPath);
assertEq(readBack, testContent, "Bun.write + Bun.readFile roundtrip");

// Clean up
try { require("fs").unlinkSync(testPath); } catch(e) {}

// ============================================================
// TEST-ENG-007: Node.js Compatibility — Buffer + process
// ============================================================
assertEq(typeof Buffer, "object", "Buffer global exists (object with from/toString)");
var buf = Buffer.from("hello");
assertEq(buf.toString(), "hello", "Buffer.from + toString");
assertEq(buf.length, 5, "Buffer.length");

assertEq(typeof process, "object", "process global exists");
assertEq(typeof process.pid, "number", "process.pid is number");
assert(process.pid > 0, "process.pid > 0");
assertEq(typeof process.argv, "object", "process.argv exists");
assertEq(typeof process.env, "object", "process.env exists");

// ============================================================
// TEST-ENG-007: node:fs operations
// ============================================================
var fs = require("fs");
var tmpFile = "/tmp/bao_fs_test_" + Date.now() + ".txt";
fs.writeFileSync(tmpFile, "test content");
var content = fs.readFileSync(tmpFile, "utf-8");
assertEq(content, "test content", "fs.writeFileSync + readFileSync");
assert(fs.existsSync(tmpFile), "fs.existsSync");
fs.unlinkSync(tmpFile);
assert(!fs.existsSync(tmpFile), "fs.unlinkSync");

// ============================================================
// TEST-CLI-001: Brand replacement
// ============================================================
assertEq(typeof Bun, "object", "Bun brand exists");
assertEq(typeof Bao, "object", "Bao alias exists");

// ============================================================
// fetch() — global fetch API
// ============================================================
assertEq(typeof fetch, "function", "fetch() global exists");

// ============================================================
// Promise support
// ============================================================
assertEq(typeof Promise, "function", "Promise exists");
var p = new Promise(function(resolve) { resolve(42); });
assert(p !== undefined, "Promise constructor works");

// ============================================================
// Bun.resolve()
// ============================================================
assertEq(typeof Bun.resolve, "function", "Bun.resolve exists");
var resolved = Bun.resolve("./tests/phase1_integration.js");
assert(resolved.indexOf("phase1_integration") >= 0, "Bun.resolve returns valid path");

// ============================================================
// node:net
// ============================================================
var net = require("net");
assert(net !== undefined, "require('net') returns value");
assertEq(typeof net.Server, "function", "net.Server is function");
assertEq(typeof net.Socket, "function", "net.Socket is function");
assertEq(typeof net.createServer, "function", "net.createServer is function");
assertEq(typeof net.connect, "function", "net.connect is function");
assertEq(net.isIP("127.0.0.1"), 4, "net.isIP('127.0.0.1') === 4");
assertEq(net.isIPv4("192.168.1.1"), true, "net.isIPv4 works");
assertEq(net.isIPv6("::1"), false, "net.isIPv6 returns false");

// ============================================================
// Buffer enhancements
// ============================================================
var b1 = Buffer.from("hello");
var b2 = Buffer.from(" world");
var b3 = Buffer.concat([b1, b2]);
assertEq(b3.toString(), "hello world", "Buffer.concat works");
assertEq(b3.length, 11, "Buffer.concat length");

var b4 = Buffer.from("hello world");
var b5 = b4.slice(0, 5);
assertEq(b5.toString(), "hello", "Buffer.slice works");

var b6 = Buffer.alloc(5);
b4.copy(b6);
assertEq(b6.toString(), "hello", "Buffer.copy works");

assertEq(Buffer.isBuffer(b1), true, "Buffer.isBuffer returns true");
assertEq(typeof Buffer.allocUnsafe, "function", "Buffer.allocUnsafe exists");

// ============================================================
// TextEncoder / TextDecoder
// ============================================================
var encoder = new TextEncoder();
var decoder = new TextDecoder();
var encoded = encoder.encode("test");
assertEq(encoded.length, 4, "TextEncoder.encode length");
assertEq(encoded[0], 116, "TextEncoder.encode first byte");
var decoded = decoder.decode(encoded);
assertEq(decoded, "test", "TextDecoder.decode roundtrip");

var fatalDec = new TextDecoder("utf-8", { fatal: true });
assert(fatalDec !== undefined, "TextDecoder with fatal option");

// ============================================================
// process enhancements
// ============================================================
assertEq(typeof process.cwd(), "string", "process.cwd returns string");
assertEq(typeof process.chdir, "function", "process.chdir is function");
assertEq(typeof process.argv0, "string", "process.argv0 is string");
assertEq(typeof process.execPath, "string", "process.execPath is string");
assertEq(typeof process.hrtime, "function", "process.hrtime is function");
assertEq(typeof process.uptime, "function", "process.uptime is function");
assert(typeof process.hrtime() === "object", "process.hrtime returns array");
assert(typeof process.uptime() === "number", "process.uptime returns number");

// ============================================================
// performance.now()
// ============================================================
assertEq(typeof performance, "object", "performance global exists");
assertEq(typeof performance.now, "function", "performance.now is function");
var t1 = performance.now();
assert(typeof t1 === "number", "performance.now returns number");
assert(t1 > 0, "performance.now > 0");

// ============================================================
// URL / URLSearchParams as globals
// ============================================================
assertEq(typeof URL, "function", "URL global constructor exists");
assertEq(typeof URLSearchParams, "function", "URLSearchParams global constructor exists");
var url = new URL("https://example.com/path?foo=bar");
assertEq(url.hostname, "example.com", "URL.hostname");
assertEq(url.pathname, "/path", "URL.pathname");
assertEq(url.search, "?foo=bar", "URL.search");
var params = new URLSearchParams("x=1&y=2");
assertEq(params.get("x"), "1", "URLSearchParams.get");
assertEq(params.has("y"), true, "URLSearchParams.has");

// ============================================================
// node:child_process
// ============================================================
var cp = require("child_process");
assert(cp !== undefined, "require('child_process') returns value");
assertEq(typeof cp.execSync, "function", "child_process.execSync is function");
assertEq(typeof cp.exec, "function", "child_process.exec is function");
assertEq(typeof cp.spawn, "function", "child_process.spawn is function");
assertEq(typeof cp.fork, "function", "child_process.fork is function");
var echoResult = cp.execSync("echo child_process_works");
assertEq(echoResult.trim(), "child_process_works", "execSync runs shell command");

// ============================================================
// node:util
// ============================================================
var util = require("util");
assert(util !== undefined, "require('util') returns value");
assertEq(typeof util.inspect, "function", "util.inspect is function");
assertEq(typeof util.format, "function", "util.format is function");
assertEq(typeof util.promisify, "function", "util.promisify is function");
assertEq(typeof util.callbackify, "function", "util.callbackify is function");
assertEq(typeof util.types, "function", "util.types is function");
assertEq(typeof util.inherits, "function", "util.inherits is function");
assertEq(typeof util.isDeepStrictEqual, "function", "util.isDeepStrictEqual is function");
assertEq(util.format("hello %s", "world"), "hello world", "util.format with %s");
assertEq(util.format("%d + %d = %d", 1, 2, 3), "1 + 2 = 3", "util.format with %d");
assertEq(util.isBoolean(true), true, "util.isBoolean(true)");
assertEq(util.isNumber(42), true, "util.isNumber(42)");
assertEq(util.isString("hi"), true, "util.isString('hi')");
assertEq(util.isArray([1,2]), true, "util.isArray([1,2])");
assertEq(util.isDeepStrictEqual(42, 42), true, "util.isDeepStrictEqual(42,42)");
assertEq(util.isDeepStrictEqual("a", "b"), false, "util.isDeepStrictEqual(a,b)");

// ============================================================
// node:crypto enhanced
// ============================================================
var crypto = require("crypto");
assertEq(typeof crypto.createHash, "function", "crypto.createHash exists");
assertEq(typeof crypto.createHmac, "function", "crypto.createHmac exists");
assertEq(typeof crypto.randomBytes, "function", "crypto.randomBytes exists");
assertEq(typeof crypto.pbkdf2Sync, "function", "crypto.pbkdf2Sync exists");
assertEq(typeof crypto.randomUUID, "function", "crypto.randomUUID exists");
assertEq(typeof crypto.createCipheriv, "function", "crypto.createCipheriv exists");
var hashResult = crypto.createHash("sha256").update("hello").digest("hex");
assertEq(hashResult.length, 64, "sha256 hex digest length");
var hmacResult = crypto.createHmac("sha256", "key").update("data").digest("hex");
assertEq(hmacResult.length, 64, "HMAC-SHA256 hex digest length");
var uuid = crypto.randomUUID();
assertEq(uuid.length, 36, "randomUUID length is 36");
assert(uuid.charAt(8) === "-", "randomUUID has dashes");
var rbuf = crypto.randomBytes(16);
assertEq(rbuf.length, 16, "randomBytes returns correct length");

// ============================================================
// node:zlib
// ============================================================
var zlib = require("zlib");
assert(zlib !== undefined, "require('zlib') returns value");
assertEq(typeof zlib.deflateSync, "function", "zlib.deflateSync exists");
assertEq(typeof zlib.inflateSync, "function", "zlib.inflateSync exists");
assertEq(typeof zlib.gzipSync, "function", "zlib.gzipSync exists");
assertEq(typeof zlib.gunzipSync, "function", "zlib.gunzipSync exists");

// ============================================================
// node:dns
// ============================================================
var dns = require("dns");
assert(dns !== undefined, "require('dns') returns value");

// ============================================================
// node:https
// ============================================================
var https = require("https");
assert(https !== undefined, "require('https') returns value");

// ============================================================
// assert module
// ============================================================
var assertMod = require("assert");
assert(assertMod !== undefined, "require('assert') returns value");
assertEq(typeof assertMod.ok, "function", "assert.ok is function");
assertEq(typeof assertMod.strictEqual, "function", "assert.strictEqual is function");
assertEq(typeof assertMod.deepStrictEqual, "function", "assert.deepStrictEqual is function");
assertEq(typeof assertMod.AssertionError, "function", "assert.AssertionError is function");

// ============================================================
// atob / btoa globals
// ============================================================
assertEq(typeof atob, "function", "atob is function");
assertEq(typeof btoa, "function", "btoa is function");
assertEq(btoa("hello"), "aGVsbG8=", "btoa('hello') encodes");
assertEq(atob("aGVsbG8="), "hello", "atob('aGVsbG8=') decodes");

// ============================================================
// WebAssembly (REQ-ENG-001)
// ============================================================
assertEq(typeof WebAssembly, "object", "WebAssembly global exists");
assertEq(typeof WebAssembly.Module, "function", "WebAssembly.Module is function");
assertEq(typeof WebAssembly.Instance, "function", "WebAssembly.Instance is function");
assertEq(typeof WebAssembly.compile, "function", "WebAssembly.compile is function");
assertEq(typeof WebAssembly.instantiate, "function", "WebAssembly.instantiate is function");

// Simple WASM module: returns 42
// (module (func (export "answer") (result i32) i32.const 42))
var wasmBytes = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
  0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f,
  0x03, 0x02, 0x01, 0x00,
  0x07, 0x0a, 0x01, 0x06, 0x61, 0x6e, 0x73, 0x77, 0x65, 0x72, 0x00, 0x00,
  0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b
]);
var wasmModule = new WebAssembly.Module(wasmBytes);
var wasmInstance = new WebAssembly.Instance(wasmModule);
assertEq(wasmInstance.exports.answer(), 42, "WebAssembly module returns 42");

// ============================================================
// Bun.build / Bun.test JS API
// ============================================================
assertEq(typeof Bun.build, "function", "Bun.build is function");
var buildResult = Bun.build({entrypoints: ["tests/phase1_integration.js"]});
assertEq(typeof buildResult, "object", "Bun.build returns object");
assertEq(buildResult.success, true, "Bun.build result has success=true");
assertEq(typeof buildResult.outputs, "object", "Bun.build result has outputs");

assertEq(typeof Bun.test, "function", "Bun.test is function");
assertEq(typeof Bun.testRun, "function", "Bun.testRun is function");
Bun.test("inline test", function() {});
var testRunResult = Bun.testRun();
assertEq(testRunResult.total >= 1, true, "Bun.testRun reports collected tests");

// ============================================================
// Bun.build / Bun.test (CLI-level)
// ============================================================
var cp = require("child_process");
var buildResult = cp.execSync("./target/debug/bao build --help").toString();
assert(buildResult.indexOf("build") >= 0, "bao build --help works");
var testResult = cp.execSync("./target/debug/bao test --help").toString();
assert(testResult.indexOf("test") >= 0, "bao test --help works");
var installResult = cp.execSync("./target/debug/bao install --help").toString();
assert(installResult.indexOf("install") >= 0, "bao install --help works");

// ============================================================
// Results
// ============================================================
console.log("\n========== Phase 1 Integration Test ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (errors.length > 0) {
  for (var i = 0; i < errors.length; i++) {
    console.log(errors[i]);
  }
}
console.log("==============================================");
if (failed > 0) {
  console.log("RESULT: FAIL");
} else {
  console.log("RESULT: ALL PASS");
}
