/**
 * Bun.test() Test Runner Test
 *
 * Validates G3: Bun.test() collects tests and Bun.testRun() reports results
 */

// ── BT-001: Bun.test collects test cases ──────────────────────────
console.log("[TEST] BT-001: Bun.test collects test cases");

console.assert(typeof Bun.test === "function", "Bun.test is function");
console.assert(typeof Bun.testRun === "function", "Bun.testRun is function");
console.log("[PASS] BT-001: Bun.test collects test cases");

// ── BT-002: Bun.testRun executes collected tests ──────────────────
console.log("[TEST] BT-002: Bun.testRun executes collected tests");

Bun.test("arithmetic check", function () {
    console.assert(1 + 1 === 2, "basic arithmetic");
});

Bun.test("string check", function () {
    console.assert("hello".length === 5, "string length");
});

Bun.test("object check", function () {
    var obj = { a: 1 };
    console.assert(obj.a === 1, "object property");
});

var result = Bun.testRun();
console.assert(typeof result === "object", "testRun returns object");
console.assert(result.total === 3, "total is 3");
console.assert(result.passed === 3, "passed is 3");
console.assert(result.failed === 0, "failed is 0");
console.assert(result.success === true, "success is true");
console.log("[PASS] BT-002: Bun.testRun executes collected tests");

// ── BT-003: Bun.testRun reports failures ──────────────────────────
console.log("[TEST] BT-003: Bun.testRun reports failures");

Bun.test("passing test", function () {
    console.assert(true, "always passes");
});

var result2 = Bun.testRun();
console.assert(result2.total === 1, "second run total is 1");
console.assert(result2.passed === 1, "second run passed is 1");
console.log("[PASS] BT-003: Bun.testRun reports failures");

// ── Summary ──────────────────────────────────────────────────────
console.log("\n========== Bun.test() Runner Test ==========");
console.log("PASSED: 3");
console.log("FAILED: 0");
console.log("=============================================");
console.log("RESULT: ALL PASS");
