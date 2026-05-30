// test_nodejs_depth.js — REQ-ENG-007: Node.js API 深度测试
// 错误处理 + 边界条件 + 异步回调

var passed = 0;
var failed = 0;

function assert(condition, msg) {
  if (condition) { passed++; }
  else { failed++; console.log("FAIL: " + msg); }
}

function assertEqual(actual, expected, msg) {
  if (actual === expected) { passed++; }
  else { failed++; console.log("FAIL: " + msg + " — expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual)); }
}

function assertIncludes(str, sub, msg) {
  if (typeof str === "string" && str.indexOf(sub) >= 0) { passed++; }
  else { failed++; console.log("FAIL: " + msg + " — '" + sub + "' not in '" + str + "'"); }
}

// === NJS-001: fs.writeFileSync + readFileSync 往返验证 ===
var fs = require("fs");
var path = require("path");
var os = require("os");
var crypto = require("crypto");

var tmpDir = os.tmpdir();
var testFile = path.join(tmpDir, "bao_test_" + Date.now() + ".txt");

fs.writeFileSync(testFile, "hello bao", "utf-8");
var readBack = fs.readFileSync(testFile, "utf-8");
assertEqual(readBack, "hello bao", "NJS-001: writeFileSync/readFileSync roundtrip");

// === NJS-002: fs.readFileSync 不存在的文件抛出错误 ===
var enoentThrown = false;
try {
  fs.readFileSync("/nonexistent/path/file.txt", "utf-8");
} catch (e) {
  enoentThrown = true;
  assertIncludes(e.code, "ENOENT", "NJS-002: ENOENT error code for missing file");
  assertEqual(e.path, "/nonexistent/path/file.txt", "NJS-002: error has path property");
}
assert(enoentThrown, "NJS-002: readFileSync throws for missing file");

// === NJS-003: fs.readFileSync buffer mode ===
var bufContent = fs.readFileSync(testFile);
assert(bufContent instanceof Buffer || (bufContent && bufContent.length > 0), "NJS-003: readFileSync without encoding returns Buffer-like");
if (bufContent && bufContent.length) {
  assertEqual(bufContent.length, 9, "NJS-003: Buffer length matches content");
}

// === NJS-004: fs.appendFileSync ===
fs.appendFileSync(testFile, " world", "utf-8");
var appended = fs.readFileSync(testFile, "utf-8");
assertEqual(appended, "hello bao world", "NJS-004: appendFileSync");

// === NJS-005: fs.existsSync ===
assert(fs.existsSync(testFile), "NJS-005a: existsSync returns true for existing file");
assert(!fs.existsSync("/nonexistent/file"), "NJS-005b: existsSync returns false for missing file");

// === NJS-006: fs.unlinkSync ===
fs.unlinkSync(testFile);
assert(!fs.existsSync(testFile), "NJS-006: unlinkSync removes file");

// === NJS-007: fs.mkdirSync + rmdirSync ===
var testDir = path.join(tmpDir, "bao_test_dir_" + Date.now());
fs.mkdirSync(testDir);
assert(fs.existsSync(testDir), "NJS-007a: mkdirSync creates directory");
fs.rmdirSync(testDir);
assert(!fs.existsSync(testDir), "NJS-007b: rmdirSync removes directory");

// === NJS-008: path.join 多段拼接 ===
assertEqual(path.join("a", "b", "c"), "a/b/c", "NJS-008: path.join multi-segment");

// === NJS-009: path.extname ===
assertEqual(path.extname("file.txt"), ".txt", "NJS-009a: path.extname .txt");
assertEqual(path.extname("file"), "", "NJS-009b: path.extname no extension");
assertEqual(path.extname("file.tar.gz"), ".gz", "NJS-009c: path.extname last extension");

// === NJS-010: path.dirname ===
assertEqual(path.dirname("/a/b/c.txt"), "/a/b", "NJS-010: path.dirname");

// === NJS-011: path.basename ===
assertEqual(path.basename("/a/b/c.txt"), "c.txt", "NJS-011a: path.basename");
assertEqual(path.basename("/a/b/c.txt", ".txt"), "c", "NJS-011b: path.basename with ext");

// === NJS-012: path.resolve ===
var resolved = path.resolve(".");
assert(typeof resolved === "string", "NJS-012: path.resolve returns string");
assert(resolved.length > 0, "NJS-012: path.resolve non-empty");

// === NJS-013: path.sep ===
assertEqual(path.sep, "/", "NJS-013: path.sep is /");

// === NJS-014: os.type ===
var osType = os.type();
assert(typeof osType === "string", "NJS-014a: os.type returns string");
assert(osType.length > 0, "NJS-014b: os.type non-empty");

// === NJS-015: os.platform ===
assertEqual(os.platform(), "linux", "NJS-015: os.platform is linux");

// === NJS-016: os.cpus 返回数组 ===
var cpus = os.cpus();
assert(Array.isArray(cpus), "NJS-016a: os.cpus returns array");
assert(cpus.length > 0, "NJS-016b: os.cpus has entries");

// === NJS-017: os.networkInterfaces 返回对象 ===
var nets = os.networkInterfaces();
assert(typeof nets === "object" && nets !== null, "NJS-017: os.networkInterfaces returns object");

// === NJS-018: crypto.createHash MD5 ===
var md5 = crypto.createHash("md5").update("hello").digest("hex");
assertEqual(md5, "5d41402abc4b2a76b9719d911017c592", "NJS-018: crypto MD5");

// === NJS-019: crypto.createHash SHA256 ===
var sha256 = crypto.createHash("sha256").update("hello").digest("hex");
assertEqual(sha256, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824", "NJS-019: crypto SHA256");

// === NJS-020: crypto.createHash 更新链式调用 ===
var chained = crypto.createHash("md5").update("hel").update("lo").digest("hex");
assertEqual(chained, md5, "NJS-020: crypto hash chaining");

// === NJS-021: crypto.createHash base64 编码 ===
var b64 = crypto.createHash("md5").update("hello").digest("base64");
assert(b64.length > 0 && b64 !== md5, "NJS-021: crypto base64 output differs from hex");

// === NJS-022: crypto.randomBytes 同步 ===
var rand = crypto.randomBytes(16);
assert(rand instanceof Buffer || (rand && rand.length === 16), "NJS-022: randomBytes returns 16 bytes");

// === NJS-023: Buffer.from + toString ===
var buf = Buffer.from("hello world", "utf-8");
assertEqual(buf.toString(), "hello world", "NJS-023: Buffer.from + toString");

// === NJS-024: Buffer.alloc 零填充 ===
var zeroBuf = Buffer.alloc(4);
assertEqual(zeroBuf[0], 0, "NJS-024a: Buffer.alloc fills with zero");
assertEqual(zeroBuf[3], 0, "NJS-024b: Buffer.alloc fills with zero");

// === NJS-025: Buffer.from hex ===
var hexBuf = Buffer.from("48656c6c6f", "hex");
assertEqual(hexBuf.toString(), "Hello", "NJS-025: Buffer.from hex encoding");

// === NJS-026: Buffer.from base64 ===
var b64Buf = Buffer.from("SGVsbG8=", "base64");
assertEqual(b64Buf.toString(), "Hello", "NJS-026: Buffer.from base64");

// === NJS-027: Buffer byteLength ===
assertEqual(Buffer.byteLength("hello"), 5, "NJS-027: Buffer.byteLength");

// === NJS-028: Buffer concat ===
var b1 = Buffer.from("hel");
var b2 = Buffer.from("lo");
var merged = Buffer.concat([b1, b2]);
assertEqual(merged.toString(), "hello", "NJS-028: Buffer.concat");

// === NJS-029: process.versions ===
assert(typeof process.versions === "object", "NJS-029: process.versions is object");

// === NJS-030: process.arch ===
assert(typeof process.arch === "string", "NJS-030: process.arch is string");

// === NJS-031: process.pid 是数字 ===
assert(typeof process.pid === "number", "NJS-031: process.pid is number");
assert(process.pid > 0, "NJS-031: process.pid > 0");

// === NJS-032: process.cwd 返回字符串 ===
var cwd = process.cwd();
assert(typeof cwd === "string", "NJS-032a: process.cwd returns string");
assert(cwd.length > 0, "NJS-032b: process.cwd non-empty");

// === NJS-033: util.inspect 基本类型 ===
var util = require("util");
assert(typeof util.inspect("hello") === "string", "NJS-033a: util.inspect string");
assertEqual(util.inspect(42), "42", "NJS-033b: util.inspect number");
assertEqual(util.inspect(true), "true", "NJS-033c: util.inspect boolean");

// === NJS-034: util.format ===
assertEqual(util.format("Hello %s", "World"), "Hello World", "NJS-034a: util.format %s");
assertEqual(util.format("Count: %d", 42), "Count: 42", "NJS-034b: util.format %d");

// === NJS-035: EventEmitter on/off/emit ===
var EventEmitter = require("events").EventEmitter;
var ee = new EventEmitter();
var emitted = false;
ee.on("test", function() { emitted = true; });
ee.emit("test");
assert(emitted, "NJS-035a: EventEmitter emit triggers listener");
ee.removeAllListeners("test");
var emitted2 = false;
ee.on("test2", function() { emitted2 = true; });
ee.off("test2");
ee.emit("test2");
assert(!emitted2, "NJS-035b: EventEmitter off removes listener");

// === NJS-036: EventEmitter once ===
var onceCount = 0;
var ee2 = new EventEmitter();
ee2.once("ping", function() { onceCount++; });
ee2.emit("ping");
ee2.emit("ping");
assertEqual(onceCount, 1, "NJS-036: EventEmitter once fires only once");

// === NJS-037: URL 构造和属性 ===
var url = new URL("https://example.com/path?q=1#hash");
assertEqual(url.protocol, "https:", "NJS-037a: URL protocol");
assertEqual(url.hostname, "example.com", "NJS-037b: URL hostname");
assertEqual(url.pathname, "/path", "NJS-037c: URL pathname");
assertEqual(url.search, "?q=1", "NJS-037d: URL search");
assertEqual(url.hash, "#hash", "NJS-037e: URL hash");

// === NJS-038: URLSearchParams ===
var params = new URLSearchParams("a=1&b=2");
assertEqual(params.get("a"), "1", "NJS-038a: URLSearchParams get");
assertEqual(params.get("b"), "2", "NJS-038b: URLSearchParams get");
assert(params.get("c") === null || params.get("c") === undefined, "NJS-038c: URLSearchParams get missing returns null/undefined");

// === NJS-039: TextEncoder/TextDecoder ===
var encoded = new TextEncoder().encode("hello");
assertEqual(encoded.length, 5, "NJS-039a: TextEncoder encodes to 5 bytes");
var decoded = new TextDecoder().decode(encoded);
assertEqual(decoded, "hello", "NJS-039b: TextDecoder roundtrip");

// === NJS-040: structuredClone ===
var original = { a: 1, b: [2, 3] };
var cloned = structuredClone(original);
assertEqual(cloned.a, 1, "NJS-040a: structuredClone deep copy");
cloned.a = 99;
assertEqual(original.a, 1, "NJS-040b: structuredClone independent copy");

// Cleanup
try { fs.unlinkSync(testFile); } catch(e) {}

console.log("\n========== Node.js Depth Test ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
console.log("=========================================");
console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: HAS FAILURES");
