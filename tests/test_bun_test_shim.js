// Test: bun:test shim compatibility
var testApi = require("bun:test");

var describe = testApi.describe;
var it = testApi.it;
var expect = testApi.expect;
var test = testApi.test;
var beforeEach = testApi.beforeEach;
var afterEach = testApi.afterEach;

var passed = 0;
var failed = 0;

// Verify all expected exports exist
var requiredExports = ["describe", "test", "it", "expect", "beforeEach", "afterEach", "beforeAll", "afterAll"];
for (var i = 0; i < requiredExports.length; i++) {
  var name = requiredExports[i];
  if (typeof testApi[name] !== "function") {
    console.log("FAIL: bun:test missing export: " + name);
    failed++;
  } else {
    passed++;
  }
}

// Test expect() matchers
try {
  expect(1).toBe(1);
  passed++;
} catch(e) { console.log("FAIL: toBe(1,1): " + e.message); failed++; }

try {
  expect("hello").toBe("hello");
  passed++;
} catch(e) { console.log("FAIL: toBe(str): " + e.message); failed++; }

try {
  expect(null).toBeNull();
  passed++;
} catch(e) { console.log("FAIL: toBeNull: " + e.message); failed++; }

try {
  expect(true).toBeTruthy();
  passed++;
} catch(e) { console.log("FAIL: toBeTruthy: " + e.message); failed++; }

try {
  expect(0).toBeFalsy();
  passed++;
} catch(e) { console.log("FAIL: toBeFalsy: " + e.message); failed++; }

try {
  expect(5).toBeGreaterThan(3);
  passed++;
} catch(e) { console.log("FAIL: toBeGreaterThan: " + e.message); failed++; }

try {
  expect(3).toBeLessThan(5);
  passed++;
} catch(e) { console.log("FAIL: toBeLessThan: " + e.message); failed++; }

try {
  expect([1,2,3]).toContain(2);
  passed++;
} catch(e) { console.log("FAIL: toContain(array): " + e.message); failed++; }

try {
  expect("hello world").toContain("world");
  passed++;
} catch(e) { console.log("FAIL: toContain(string): " + e.message); failed++; }

try {
  expect([1,2,3]).toHaveLength(3);
  passed++;
} catch(e) { console.log("FAIL: toHaveLength: " + e.message); failed++; }

try {
  expect(function() { throw new Error("boom"); }).toThrow();
  passed++;
} catch(e) { console.log("FAIL: toThrow: " + e.message); failed++; }

try {
  expect({a:1, b:2}).toMatchObject({a:1});
  passed++;
} catch(e) { console.log("FAIL: toMatchObject: " + e.message); failed++; }

try {
  expect({a:1}).toHaveProperty("a");
  passed++;
} catch(e) { console.log("FAIL: toHaveProperty: " + e.message); failed++; }

// Test not matchers
try {
  expect(1).not.toBe(2);
  passed++;
} catch(e) { console.log("FAIL: not.toBe: " + e.message); failed++; }

try {
  expect("hello").not.toContain("xyz");
  passed++;
} catch(e) { console.log("FAIL: not.toContain: " + e.message); failed++; }

// Test describe/test registration
var suiteRun = false;
describe("example suite", function() {
  test("a test", function() {
    suiteRun = true;
    expect(42).toBe(42);
  });
});

// Run tests
var result = globalThis.__run_bun_tests();
if (result.passed > 0 && suiteRun) {
  passed++;
} else {
  console.log("FAIL: describe/test didn't run suites");
  failed++;
}

console.log("========== bun:test Shim Test ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) {
  console.log("RESULT: FAIL");
} else {
  console.log("RESULT: ALL PASS");
}
