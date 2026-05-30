// test_http_depth.js — REQ-ENG-006: fetch/HTTP 深度测试
// Bun.serve HTTP 方法 + fetch POST/PUT/DELETE + 状态码 + headers
// 注: node http + fetch 在同一进程会死锁（事件循环架构限制），
// 此测试使用 Bun.serve（独立线程）来验证 HTTP 方法

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

// === HTTP-001: Bun.serve 动态端口绑定 ===
var server = Bun.serve({ port: 0 });
var port = server.port;
assert(port > 0, "HTTP-001: Bun.serve binds to dynamic port");

// === HTTP-002: fetch GET ===
fetch("http://127.0.0.1:" + port + "/test-path").then(function(res) {
  assertEqual(res.status, 200, "HTTP-002a: GET returns 200");
  return res.text();
}).then(function(text) {
  assertIncludes(text, "GET", "HTTP-002b: GET method in response");
  assertIncludes(text, "/test-path", "HTTP-002c: path in response");

  // === HTTP-003: fetch POST ===
  return fetch("http://127.0.0.1:" + port + "/api/post", {
    method: "POST",
    body: "post-data"
  });
}).then(function(res) {
  assertEqual(res.status, 200, "HTTP-003a: POST returns 200");
  return res.text();
}).then(function(text) {
  assertIncludes(text, "POST", "HTTP-003b: POST method in response");
  assertIncludes(text, "/api/post", "HTTP-003c: POST url in response");

  // === HTTP-004: fetch PUT ===
  return fetch("http://127.0.0.1:" + port + "/api/put", {
    method: "PUT",
    body: "put-data"
  });
}).then(function(res) {
  assertEqual(res.status, 200, "HTTP-004a: PUT returns 200");
  return res.text();
}).then(function(text) {
  assertIncludes(text, "PUT", "HTTP-004b: PUT method in response");

  // === HTTP-005: fetch DELETE ===
  return fetch("http://127.0.0.1:" + port + "/api/delete", {
    method: "DELETE"
  });
}).then(function(res) {
  assertEqual(res.status, 200, "HTTP-005a: DELETE returns 200");
  return res.text();
}).then(function(text) {
  assertIncludes(text, "DELETE", "HTTP-005b: DELETE method in response");

  // === HTTP-006: fetch PATCH ===
  return fetch("http://127.0.0.1:" + port + "/api/patch", {
    method: "PATCH",
    body: "patch-data"
  });
}).then(function(res) {
  assertEqual(res.status, 200, "HTTP-006a: PATCH returns 200");
  return res.text();
}).then(function(text) {
  assertIncludes(text, "PATCH", "HTTP-006b: PATCH method in response");

  // === HTTP-007: fetch HEAD ===
  return fetch("http://127.0.0.1:" + port + "/api/head", {
    method: "HEAD"
  });
}).then(function(res) {
  assertEqual(res.status, 200, "HTTP-007: HEAD returns 200");

  // === HTTP-008: 多个并发请求 ===
  return Promise.all([
    fetch("http://127.0.0.1:" + port + "/a"),
    fetch("http://127.0.0.1:" + port + "/b"),
    fetch("http://127.0.0.1:" + port + "/c")
  ]);
}).then(function(responses) {
  assertEqual(responses.length, 3, "HTTP-008a: 3 concurrent responses");
  assertEqual(responses[0].status, 200, "HTTP-008b: first response 200");
  assertEqual(responses[1].status, 200, "HTTP-008c: second response 200");
  assertEqual(responses[2].status, 200, "HTTP-008d: third response 200");

  // === HTTP-009: fetch 非 existent host ===
  return fetch("http://127.0.0.1:1/nonexistent");
}).then(function() {
  // 不应该到达这里
  assert(false, "HTTP-009: fetch to closed port should reject");
  server.stop();
  finishTests();
}).catch(function(err) {
  assert(err !== undefined, "HTTP-009: fetch to closed port rejects with error");
  server.stop();
  finishTests();
});

var finishCalled = false;
function finishTests() {
  if (finishCalled) return;
  finishCalled = true;
  try { server.stop(); } catch(e) {}
  console.log("\n========== HTTP Depth Test ==========");
  console.log("PASSED: " + passed);
  console.log("FAILED: " + failed);
  console.log("=====================================");
  console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: HAS FAILURES");
}
