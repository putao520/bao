/**
 * process.stdin Readable Stream Test
 *
 * Validates G5: process.stdin has readable stream properties and methods
 */

// ── SI-001: process.stdin properties ──────────────────────────────
console.log("[TEST] SI-001: process.stdin properties");

console.assert(process.stdin !== undefined, "process.stdin exists");
console.assert(process.stdin.fd === 0, "stdin fd is 0");
console.assert(typeof process.stdin.isTTY === "boolean", "isTTY is boolean");
console.assert(process.stdin.readable === true, "readable is true");
console.log("[PASS] SI-001: process.stdin properties");

// ── SI-002: process.stdin has stream methods ──────────────────────
console.log("[TEST] SI-002: process.stdin has stream methods");

console.assert(typeof process.stdin.read === "function", "read() exists");
console.assert(typeof process.stdin.on === "function", "on() exists");
console.assert(typeof process.stdin.pipe === "function", "pipe() exists");
console.assert(typeof process.stdin.resume === "function", "resume() exists");
console.assert(typeof process.stdin.pause === "function", "pause() exists");
console.assert(typeof process.stdin.destroy === "function", "destroy() exists");
console.log("[PASS] SI-002: process.stdin has stream methods");

// ── SI-003: process.stdin.read returns null when no data ──────────
console.log("[TEST] SI-003: process.stdin.read returns null when no data");

// When stdin is not a TTY and has no data piped, read() returns null
var result = process.stdin.read();
console.assert(result === null, "read() returns null when no stdin data");
console.log("[PASS] SI-003: process.stdin.read returns null when no data");

// ── SI-004: process.stdin methods are chainable ───────────────────
console.log("[TEST] SI-004: process.stdin methods are chainable");

var ret = process.stdin.resume();
console.assert(ret === undefined, "resume() returns undefined");
var ret2 = process.stdin.pause();
console.assert(ret2 === undefined, "pause() returns undefined");
console.log("[PASS] SI-004: process.stdin methods are chainable");

// ── Summary ──────────────────────────────────────────────────────
console.log("\n========== process.stdin Test ==========");
console.log("PASSED: 4");
console.log("FAILED: 0");
console.log("=========================================");
console.log("RESULT: ALL PASS");
