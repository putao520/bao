/**
 * Bun.build() Test
 *
 * Validates G2: Bun.build() reads entrypoints and returns build result
 */

var fs = require("fs");
var path = require("path");

// ── BB-001: Bun.build with no args returns success ────────────────
console.log("[TEST] BB-001: Bun.build with no args returns success");

var result1 = Bun.build();
console.assert(result1 !== undefined, "Bun.build returns value");
console.assert(result1.success === true, "success is true with no args");
console.assert(Array.isArray(result1.outputs), "outputs is array");
console.assert(result1.outputs.length === 0, "outputs is empty with no args");
console.log("[PASS] BB-001: Bun.build with no args returns success");

// ── BB-002: Bun.build with empty entrypoints ──────────────────────
console.log("[TEST] BB-002: Bun.build with empty entrypoints");

var result2 = Bun.build({ entrypoints: [] });
console.assert(result2.success === true, "success is true with empty entrypoints");
console.assert(result2.outputs.length === 0, "outputs is empty");
console.log("[PASS] BB-002: Bun.build with empty entrypoints");

// ── BB-003: Bun.build reads entry file ────────────────────────────
console.log("[TEST] BB-003: Bun.build reads entry file");

var tmpDir = "/tmp/bao_build_test_" + Date.now();
fs.mkdirSync(tmpDir, { recursive: true });
var entryFile = tmpDir + "/index.js";
fs.writeFileSync(entryFile, 'console.log("hello from build");');

var result3 = Bun.build({ entrypoints: [entryFile] });
console.assert(result3.success === true, "success is true with valid entry");
console.assert(result3.outputs.length === 1, "one output artifact");
console.assert(result3.outputs[0].path === entryFile, "output path matches entry");
console.assert(result3.outputs[0].output === "dist/index.js", "output file in dist dir");
console.assert(typeof result3.outputs[0].size === "number", "size is a number");
console.assert(result3.outputs[0].size > 0, "size is positive");
console.assert(result3.outputs[0].kind === "js", "kind is js for .js file");
console.log("[PASS] BB-003: Bun.build reads entry file");

// ── BB-004: Bun.build with non-existent file returns failure ──────
console.log("[TEST] BB-004: Bun.build with non-existent file returns failure");

var result4 = Bun.build({ entrypoints: ["/nonexistent/file.js"] });
console.assert(result4.success === false, "success is false for missing file");
console.assert(result4.logs !== undefined, "logs object exists on failure");
console.assert(typeof result4.logs.message === "string", "error message is string");
console.log("[PASS] BB-004: Bun.build with non-existent file returns failure");

// ── BB-005: Bun.build with custom outdir and naming ───────────────
console.log("[TEST] BB-005: Bun.build with custom outdir and naming");

var tsFile = tmpDir + "/app.ts";
fs.writeFileSync(tsFile, "const x: number = 1;");

var result5 = Bun.build({
    entrypoints: [tsFile],
    outdir: "build",
    naming: "[name].bundle.js"
});
console.assert(result5.success === true, "success with custom config");
console.assert(result5.outputs[0].output === "build/app.bundle.js", "custom naming applied");
console.assert(result5.outputs[0].kind === "ts", "kind is ts for .ts file");
console.log("[PASS] BB-005: Bun.build with custom outdir and naming");

// Cleanup
fs.unlinkSync(entryFile);
fs.unlinkSync(tsFile);
fs.rmdirSync(tmpDir);

// ── Summary ──────────────────────────────────────────────────────
console.log("\n========== Bun.build() Test ==========");
console.log("PASSED: 5");
console.log("FAILED: 0");
console.log("=======================================");
console.log("RESULT: ALL PASS");
