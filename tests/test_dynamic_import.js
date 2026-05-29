// REQ-ENG-005: Dynamic import() acceptance test
// Covers: C1 (import/export), C2 (node_modules), C4 (dynamic import), C5 (module cache)

var passed = 0;
var failed = 0;
var asyncPass = 0;
var asyncFail = 0;
var asyncDone = 0;
var asyncExpected = 0;

function assert(c, m) { if(c) passed++; else { failed++; console.log("FAIL: " + m); } }
function asyncAssert(c, m) { if(c) asyncPass++; else { asyncFail++; console.log("FAIL: " + m); } }

// ============================================================
// C4: Dynamic import() of built-in modules
// ============================================================
asyncExpected++;
import("fs").then(function(fs) {
  asyncAssert(typeof fs !== "undefined", "C4: import('fs') resolves with value");
  asyncAssert(typeof fs.readFileSync === "function", "C4: import('fs') has readFileSync");
  asyncAssert(typeof fs.writeFileSync === "function", "C4: import('fs') has writeFileSync");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('fs') rejected: " + err);
  checkComplete();
});

// C4: import with node: prefix
asyncExpected++;
import("node:path").then(function(path) {
  asyncAssert(typeof path.join === "function", "C4: import('node:path') has join");
  asyncAssert(path.join("a", "b") === "a/b", "C4: import('node:path') join works");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('node:path') rejected: " + err);
  checkComplete();
});

// C4: import crypto
asyncExpected++;
import("crypto").then(function(crypto) {
  asyncAssert(typeof crypto.createHash === "function", "C4: import('crypto') has createHash");
  asyncAssert(typeof crypto.randomUUID === "function", "C4: import('crypto') has randomUUID");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('crypto') rejected: " + err);
  checkComplete();
});

// C4: import os
asyncExpected++;
import("node:os").then(function(os) {
  asyncAssert(typeof os.platform === "function", "C4: import('node:os') has platform");
  asyncAssert(typeof os.platform() === "string", "C4: import('node:os') platform returns string");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('node:os') rejected: " + err);
  checkComplete();
});

// C4: import url
asyncExpected++;
import("url").then(function(url) {
  asyncAssert(typeof url !== "undefined", "C4: import('url') resolves");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('url') rejected: " + err);
  checkComplete();
});

// C4: import events
asyncExpected++;
import("events").then(function(events) {
  asyncAssert(typeof events !== "undefined", "C4: import('events') resolves");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('events') rejected: " + err);
  checkComplete();
});

// C4: Failed import rejects properly
asyncExpected++;
import("nonexistent_module_xyz").then(function() {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: import('nonexistent') should have rejected");
  checkComplete();
}, function(err) {
  asyncAssert(true, "C4: import('nonexistent') correctly rejects");
  asyncDone++;
  checkComplete();
});

// ============================================================
// C5: Module cache — second import returns same object
// ============================================================
asyncExpected++;
Promise.all([import("fs"), import("fs")]).then(function(results) {
  asyncAssert(results[0] === results[1], "C5: cached import returns same object");
  asyncDone++;
  checkComplete();
}, function(err) {
  asyncFail++;
  asyncDone++;
  console.log("FAIL: cached import test: " + err);
  checkComplete();
});

// ============================================================
// Promise chain validation (microtask drain)
// ============================================================
asyncExpected++;
Promise.resolve(1)
  .then(function(v) { return v + 1; })
  .then(function(v) { return v * 3; })
  .then(function(v) {
    asyncAssert(v === 6, "Promise chain: resolve → transform works correctly");
    asyncDone++;
    checkComplete();
  });

function checkComplete() {
  if (asyncDone < asyncExpected) return;
  console.log("\n========== Dynamic import() Test ==========");
  console.log("PASSED: " + (passed + asyncPass));
  console.log("FAILED: " + (failed + asyncFail));
  console.log("============================================");
  if (failed + asyncFail > 0) {
    console.log("RESULT: FAIL");
  } else {
    console.log("RESULT: ALL PASS");
  }
}
