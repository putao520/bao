// test_bun_api.js — Bun.* API 深度测试
var passed = 0;
var failed = 0;

function assert(c, m) { if(c) passed++; else { failed++; console.log("FAIL: " + m); } }
function assertEqual(a, e, m) { if(a===e) passed++; else { failed++; console.log("FAIL: " + m + " got " + JSON.stringify(a)); } }
function assertIncludes(str, sub, m) { if(typeof str==="string" && str.indexOf(sub)>=0) passed++; else { failed++; console.log("FAIL: " + m); } }

// === BUN-001: Bun.read alias ===
assert(typeof Bun.read === "function", "BUN-001: Bun.read is function");

// === BUN-002: Bun.exit exists ===
assert(typeof Bun.exit === "function", "BUN-002: Bun.exit is function");

// === BUN-003: Bun.sleepSync ===
assert(typeof Bun.sleepSync === "function", "BUN-003a: Bun.sleepSync is function");
var t1 = Date.now();
Bun.sleepSync(10);
var elapsed = Date.now() - t1;
assert(elapsed >= 8, "BUN-003b: Bun.sleepSync(10) takes >= 8ms got " + elapsed);

// === BUN-004: Bun.revision ===
assert(typeof Bun.revision === "string", "BUN-004a: Bun.revision is string");
assert(Bun.revision.length > 0, "BUN-004b: Bun.revision non-empty");

// === BUN-005: Bun.main ===
assert(typeof Bun.main === "string", "BUN-005: Bun.main is string");

// === BUN-006: Bun.hash ===
assert(typeof Bun.hash === "function", "BUN-006a: Bun.hash is function");
var h = Bun.hash("hello");
assertEqual(h.length, 64, "BUN-006b: Bun.hash sha256 hex length");
assertIncludes(h, "2cf24dba", "BUN-006c: Bun.hash sha256 correct");

var h512 = Bun.hash("hello", "sha512");
assertEqual(h512.length, 128, "BUN-006d: Bun.hash sha512 hex length");

// === BUN-007: Bun.version ===
assert(typeof Bun.version === "string", "BUN-007a: Bun.version is string");
assert(Bun.version.length > 0, "BUN-007b: Bun.version non-empty");

// === BUN-008: Bun.serve ===
assert(typeof Bun.serve === "function", "BUN-008: Bun.serve is function");

// === BUN-009: Bun.spawn ===
assert(typeof Bun.spawn === "function", "BUN-009: Bun.spawn is function");

// === BUN-010: Bun.inspect ===
assert(typeof Bun.inspect === "function", "BUN-010a: Bun.inspect is function");
var insp = Bun.inspect({a: 1});
assert(typeof insp === "string", "BUN-010b: Bun.inspect returns string");

// === BUN-011: Bun.cwd ===
assert(typeof Bun.cwd === "function", "BUN-011a: Bun.cwd is function");
var cwd = Bun.cwd();
assert(typeof cwd === "string", "BUN-011b: Bun.cwd returns string");

// === BUN-012: Bun.gc ===
assert(typeof Bun.gc === "function", "BUN-012: Bun.gc is function");

// === BUN-013: Bun.which ===
assert(typeof Bun.which === "function", "BUN-013: Bun.which is function");

// === BUN-014: Bun.resolve ===
assert(typeof Bun.resolve === "function", "BUN-014: Bun.resolve is function");

// === BUN-015: Bun.file ===
assert(typeof Bun.file === "function", "BUN-015: Bun.file is function");

// === BUN-016: Bun.write ===
assert(typeof Bun.write === "function", "BUN-016: Bun.write is function");

// === BUN-017: Bun.env is object ===
assert(typeof Bun.env === "object", "BUN-017: Bun.env is object");

// === BUN-018: Bun.argv is object ===
assert(typeof Bun.argv === "object", "BUN-018: Bun.argv is object");

// === BUN-019: Bun.sleep (async) ===
assert(typeof Bun.sleep === "function", "BUN-019: Bun.sleep is function");

// === BUN-020: Bun.build exists ===
assert(typeof Bun.build === "function", "BUN-020: Bun.build is function");

// === BUN-021: API count >= 22 ===
var keys = Object.keys(Bun);
assert(keys.length >= 22, "BUN-021: Bun has >= 22 APIs got " + keys.length);

console.log("\n========== Bun API Test ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
console.log("==================================");
console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: HAS FAILURES");
