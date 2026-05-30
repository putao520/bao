// test_module_resolution.js — REQ-ENG-005: 模块解析边界条件测试
// import/export + require + 动态 import + 错误处理

var passed = 0;
var failed = 0;

function assert(condition, msg) {
  if (condition) { passed++; }
  else { failed++; console.log("FAIL: " + msg); }
}

function assertIncludes(str, sub, msg) {
  if (typeof str === "string" && str.indexOf(sub) >= 0) { passed++; }
  else { failed++; console.log("FAIL: " + msg + " — '" + sub + "' not in '" + str + "'"); }
}

// === MOD-001: require 内置模块 ===
var fs = require("fs");
assert(typeof fs === "object" && fs !== null, "MOD-001a: require('fs') returns object");
assert(typeof fs.readFileSync === "function", "MOD-001b: fs has readFileSync");
assert(typeof fs.writeFileSync === "function", "MOD-001c: fs has writeFileSync");

var path = require("path");
assert(typeof path.join === "function", "MOD-001d: path has join");

var http = require("http");
assert(typeof http.createServer === "function", "MOD-001e: http has createServer");

// === MOD-002: require 缓存 ===
var fs2 = require("fs");
assert(fs === fs2, "MOD-002: require returns cached module");

// === MOD-003: require 不存在的模块抛错 ===
var moduleNotFound = false;
try {
  require("nonexistent_module_xyz");
} catch (e) {
  moduleNotFound = true;
  assertIncludes(String(e), "nonexistent_module_xyz", "MOD-003: module not found error includes module name");
}
assert(moduleNotFound, "MOD-003: require missing module throws");

// === MOD-004: require Node.js 前缀模块 ===
var nodeFs = require("node:fs");
assert(typeof nodeFs.readFileSync === "function", "MOD-004a: require('node:fs') works");
var nodePath = require("node:path");
assert(typeof nodePath.join === "function", "MOD-004b: require('node:path') works");

// === MOD-005: require 带路径的文件 ===
var os = require("os");
var tmpDir = os.tmpdir();
var fs_sync = require("fs");
var testModPath = path.join(tmpDir, "bao_mod_test_" + Date.now() + ".js");
fs_sync.writeFileSync(testModPath, "module.exports = { value: 42, greet: function(name) { return 'hello ' + name; } };");

var mod = require(testModPath);
assertEqual(mod.value, 42, "MOD-005a: require file exports value");
assertEqual(mod.greet("bao"), "hello bao", "MOD-005b: require file exports function");

// === MOD-006: require JSON 文件 ===
var jsonPath = path.join(tmpDir, "bao_mod_test_" + Date.now() + ".json");
fs_sync.writeFileSync(jsonPath, '{"name":"bao","version":"0.1.0"}');

var jsonMod = require(jsonPath);
assertEqual(jsonMod.name, "bao", "MOD-006a: require JSON file parses name");
assertEqual(jsonMod.version, "0.1.0", "MOD-006b: require JSON file parses version");

// === MOD-007: require 导出不同类型 ===
var funcModPath = path.join(tmpDir, "bao_func_mod_" + Date.now() + ".js");
fs_sync.writeFileSync(funcModPath, "module.exports = function(x) { return x * 2; };");
var funcMod = require(funcModPath);
assertEqual(funcMod(5), 10, "MOD-007: require exports function directly");

// === MOD-008: require 导出 class ===
var classModPath = path.join(tmpDir, "bao_class_mod_" + Date.now() + ".js");
fs_sync.writeFileSync(classModPath, "class Foo { constructor(v) { this.v = v; } get() { return this.v; } } module.exports = Foo;");
var FooClass = require(classModPath);
var instance = new FooClass(99);
assertEqual(instance.get(), 99, "MOD-008: require exports class");

// === MOD-009: 动态 import() ===
var dynamicModPath = path.join(tmpDir, "bao_dynamic_mod_" + Date.now() + ".js");
fs_sync.writeFileSync(dynamicModPath, "export const name = 'dynamic'; export function add(a, b) { return a + b; }");

var dynamicImport = false;
import(dynamicModPath).then(function(mod) {
  dynamicImport = true;
  assertEqual(mod.name, "dynamic", "MOD-009a: dynamic import gets named export");
  assertEqual(mod.add(3, 4), 7, "MOD-009b: dynamic import function works");
  finishTests();
}).catch(function(e) {
  console.log("Dynamic import error:", e);
  failed++;
  finishTests();
});

// === MOD-010: require 相对路径 ===
var relModPath = path.join(tmpDir, "bao_rel_mod_" + Date.now() + ".js");
fs_sync.writeFileSync(relModPath, "module.exports = { ok: true };");
var relMod = require(relModPath);
assertEqual(relMod.ok, true, "MOD-010: require with absolute path");

// Cleanup
try { fs_sync.unlinkSync(testModPath); } catch(e) {}
try { fs_sync.unlinkSync(jsonPath); } catch(e) {}
try { fs_sync.unlinkSync(funcModPath); } catch(e) {}
try { fs_sync.unlinkSync(classModPath); } catch(e) {}

function assertEqual(actual, expected, msg) {
  if (actual === expected) { passed++; }
  else { failed++; console.log("FAIL: " + msg + " — expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual)); }
}

var finishCalled = false;
function finishTests() {
  if (finishCalled) return;
  finishCalled = true;
  try { fs_sync.unlinkSync(dynamicModPath); } catch(e) {}
  console.log("\n========== Module Resolution Test ==========");
  console.log("PASSED: " + passed);
  console.log("FAILED: " + failed);
  console.log("=============================================");
  console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: HAS FAILURES");
}
