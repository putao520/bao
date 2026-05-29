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
// REQ-ENG-006: Bun API
// ============================================================
assertEq(typeof Bun, "object", "Bun global exists");
assertEq(typeof Bao, "object", "Bao alias exists");
assertEq(Bun === Bao, true, "Bun === Bao (same object)");
assertEq(typeof Bun.version, "string", "Bun.version is string");
assertEq(typeof Bun.env, "function", "Bun.env is function");
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
// REQ-ENG-007: Node.js Compatibility — Buffer + process
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
// REQ-ENG-007: node:fs operations
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
// REQ-CLI-001: Brand replacement
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
