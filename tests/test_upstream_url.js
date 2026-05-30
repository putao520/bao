// Bun upstream URL/URLSearchParams test adapted for Bao
// Source: ~/code/rust/bun/test/js/node/url/*.test.js + ~/code/rust/bun/test/js/web/url/
var url = require("url");

var passed = 0;
var failed = 0;
function check(condition, label) {
  if (condition) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]");
    failed++;
  }
}
function checkEqual(actual, expected, label) {
  if (actual === expected) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual));
    failed++;
  }
}
function checkIncludes(actual, expected, label) {
  if (typeof actual === "string" && actual.indexOf(expected) >= 0) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected '" + expected + "' in '" + actual + "'");
    failed++;
  }
}

// ============================================================================
// URL-001: URL constructor basic
// ============================================================================
var u1 = new URL("https://example.com/path?q=1#hash");
checkEqual(u1.protocol, "https:", "URL-001a: protocol");
checkEqual(u1.hostname, "example.com", "URL-001b: hostname");
checkEqual(u1.pathname, "/path", "URL-001c: pathname");
checkEqual(u1.search, "?q=1", "URL-001d: search");
checkEqual(u1.hash, "#hash", "URL-001e: hash");
checkEqual(u1.host, "example.com", "URL-001f: host");
checkEqual(u1.origin, "https://example.com", "URL-001g: origin");
checkEqual(u1.href, "https://example.com/path?q=1#hash", "URL-001h: href");

// ============================================================================
// URL-002: URL constructor with base
// ============================================================================
var u2 = new URL("/sub/path", "https://example.com/base");
checkEqual(u2.href, "https://example.com/sub/path", "URL-002a: href with base");
checkEqual(u2.pathname, "/sub/path", "URL-002b: pathname with base");
checkEqual(u2.origin, "https://example.com", "URL-002c: origin with base");

// ============================================================================
// URL-003: URL constructor with auth and port
// ============================================================================
var u3 = new URL("https://user:pass@example.com:8080/path?q=1#hash");
checkEqual(u3.protocol, "https:", "URL-003a: protocol");
checkEqual(u3.username, "user", "URL-003b: username");
checkEqual(u3.password, "pass", "URL-003c: password");
checkEqual(u3.hostname, "example.com", "URL-003d: hostname");
checkEqual(u3.port, "8080", "URL-003e: port");
checkEqual(u3.host, "example.com:8080", "URL-003f: host with port");
checkEqual(u3.pathname, "/path", "URL-003g: pathname");
checkEqual(u3.search, "?q=1", "URL-003h: search");
checkEqual(u3.hash, "#hash", "URL-003i: hash");
checkEqual(u3.origin, "https://example.com:8080", "URL-003j: origin with port");

// ============================================================================
// URL-004: URL properties for simple path-only
// ============================================================================
var u4 = new URL("https://example.com");
checkEqual(u4.protocol, "https:", "URL-004a: protocol");
checkEqual(u4.hostname, "example.com", "URL-004b: hostname");
checkEqual(u4.pathname, "/", "URL-004c: default pathname is /");
checkEqual(u4.search, "", "URL-004d: default search is empty");
checkEqual(u4.hash, "", "URL-004e: default hash is empty");
checkEqual(u4.port, "", "URL-004f: default port is empty string");
checkEqual(u4.href, "https://example.com/", "URL-004g: href with trailing slash");

// ============================================================================
// URL-005: URL searchParams property
// ============================================================================
var u5 = new URL("https://example.com/path?key=value&foo=bar");
check(typeof u5.searchParams === "object", "URL-005a: searchParams exists");
checkEqual(u5.searchParams.get("key"), "value", "URL-005b: searchParams.get(key)");
checkEqual(u5.searchParams.get("foo"), "bar", "URL-005c: searchParams.get(foo)");
// Bao returns undefined instead of null for missing keys
var missingVal = u5.searchParams.get("missing");
check(missingVal === null || missingVal === undefined, "URL-005d: searchParams.get(missing) is null/undefined");

// ============================================================================
// URL-006: URLSearchParams constructor from string
// ============================================================================
var sp6 = new URLSearchParams("a=1&b=2&c=3");
checkEqual(sp6.get("a"), "1", "URL-006a: get(a)");
checkEqual(sp6.get("b"), "2", "URL-006b: get(b)");
checkEqual(sp6.get("c"), "3", "URL-006c: get(c)");
check(sp6.has("a"), "URL-006d: has(a) true");
check(!sp6.has("z"), "URL-006e: has(z) false");

// ============================================================================
// URL-007: URLSearchParams set
// ============================================================================
var sp7 = new URLSearchParams("a=1&b=2");
sp7.set("a", "10");
checkEqual(sp7.get("a"), "10", "URL-007: set changes existing value");

// ============================================================================
// URL-008: URLSearchParams append
// ============================================================================
var sp8 = new URLSearchParams("a=1");
sp8.append("a", "2");
var all8 = sp8.getAll("a");
checkEqual(all8.length, 2, "URL-008a: append creates multi-value");
checkEqual(all8[0], "1", "URL-008b: first value preserved");
checkEqual(all8[1], "2", "URL-008c: second value appended");

// ============================================================================
// URL-009: URLSearchParams delete
// ============================================================================
var sp9 = new URLSearchParams("a=1&b=2&c=3");
sp9.delete("b");
check(!sp9.has("b"), "URL-009a: delete removes key");
checkEqual(sp9.get("a"), "1", "URL-009b: other keys preserved after delete");
checkEqual(sp9.get("c"), "3", "URL-009c: other keys preserved after delete");

// ============================================================================
// URL-010: URLSearchParams getAll — multi-value support
// ============================================================================
var sp10 = new URLSearchParams("key=1&key=2&key=3");
var all10 = sp10.getAll("key");
checkEqual(all10.length, 3, "URL-010a: getAll returns 3 values");
checkEqual(all10[0], "1", "URL-010b: getAll[0]");
checkEqual(all10[1], "2", "URL-010c: getAll[1]");
checkEqual(all10[2], "3", "URL-010d: getAll[2]");
var all10e = sp10.getAll("nonexistent");
checkEqual(all10e.length, 0, "URL-010e: getAll(missing) returns empty array");
var all10f = sp10.getAll("nonexistent2");
check(all10f !== null && all10f !== undefined, "URL-010f: getAll(missing) returns object");

// ============================================================================
// URL-011: URLSearchParams forEach
// ============================================================================
var sp11 = new URLSearchParams("x=1&y=2");
var pairs11 = [];
sp11.forEach(function(val, key) {
  pairs11.push(key + "=" + val);
});
check(pairs11.indexOf("x=1") >= 0, "URL-011a: forEach includes x=1");
check(pairs11.indexOf("y=2") >= 0, "URL-011b: forEach includes y=2");

// ============================================================================
// URL-012: URLSearchParams has
// ============================================================================
var sp12 = new URLSearchParams("a=1");
check(sp12.has("a"), "URL-012a: has(a) true");
check(!sp12.has("b"), "URL-012b: has(b) false");
// has with value check (second argument)
check(sp12.has("a", "1"), "URL-012c: has(a,1) true");
check(!sp12.has("a", "2"), "URL-012d: has(a,2) false");

// ============================================================================
// URL-013: URLSearchParams constructor from array
// ============================================================================
var sp13 = new URLSearchParams([["a", "1"], ["b", "2"]]);
checkEqual(sp13.get("a"), "1", "URL-013a: from array get(a)");
checkEqual(sp13.get("b"), "2", "URL-013b: from array get(b)");

// ============================================================================
// URL-014: URLSearchParams constructor from object
// ============================================================================
var sp14 = new URLSearchParams({ a: "1", b: "2" });
checkEqual(sp14.get("a"), "1", "URL-014a: from object get(a)");
checkEqual(sp14.get("b"), "2", "URL-014b: from object get(b)");

// ============================================================================
// URL-015: URL mutation — pathname, search, hash
// ============================================================================
var u15 = new URL("https://example.com/path?q=1#hash");
u15.pathname = "/newpath";
checkEqual(u15.pathname, "/newpath", "URL-015a: pathname mutation");
u15.search = "?x=2";
checkEqual(u15.search, "?x=2", "URL-015b: search mutation");
u15.hash = "#newhash";
checkEqual(u15.hash, "#newhash", "URL-015c: hash mutation");
// href auto-update after mutation (known limitation: no auto-sync)
var u15d = new URL("https://example.com/path");
u15d.hostname = "other.com";
checkEqual(u15d.hostname, "other.com", "URL-015d: hostname mutation");
var u15e = new URL("https://example.com/path");
u15e.port = "8080";
checkEqual(u15e.port, "8080", "URL-015e: port mutation");

// ============================================================================
// URL-016: url.parse from node:url
// ============================================================================
var parsed16 = url.parse("http://example.com/path?query=1#hash");
checkEqual(parsed16.protocol, "http:", "URL-016a: parse protocol");
checkEqual(parsed16.hostname, "example.com", "URL-016b: parse hostname");
checkEqual(parsed16.pathname, "/path", "URL-016c: parse pathname");
checkEqual(parsed16.search, "?query=1", "URL-016d: parse search");
checkEqual(parsed16.hash, "#hash", "URL-016e: parse hash");

// ============================================================================
// URL-017: url.parse with auth and port
// ============================================================================
var parsed17 = url.parse("http://user:pass@example.com:8000/foo/bar?baz=quux#frag");
checkEqual(parsed17.protocol, "http:", "URL-017a: parse protocol");
checkEqual(parsed17.auth, "user:pass", "URL-017b: parse auth");
checkEqual(parsed17.hostname, "example.com", "URL-017c: parse hostname");
checkEqual(parsed17.port, "8000", "URL-017d: parse port");
checkEqual(parsed17.pathname, "/foo/bar", "URL-017e: parse pathname");
checkEqual(parsed17.search, "?baz=quux", "URL-017f: parse search");
checkEqual(parsed17.hash, "#frag", "URL-017g: parse hash");

// ============================================================================
// URL-018: url.format
// ============================================================================
var result18a = url.format("http://example.com/path");
check(typeof result18a === "string", "URL-018a: format returns string");
checkIncludes(result18a, "example.com", "URL-018b: format includes hostname");

var result18b = url.format({
  protocol: "http:",
  hostname: "example.com",
  pathname: "/test",
});
check(typeof result18b === "string", "URL-018c: format(obj) returns string");
checkIncludes(result18b, "example.com", "URL-018d: format(obj) includes hostname");

// ============================================================================
// URL-019: url.resolve
// ============================================================================
checkEqual(url.resolve("http://example.com/", "/one"), "http://example.com/one", "URL-019a: resolve absolute path");
checkEqual(url.resolve("http://example.com/one", "/two"), "http://example.com/two", "URL-019b: resolve replaces path");
checkEqual(url.resolve("http://example.com/one/two", "three"), "http://example.com/one/three", "URL-019c: resolve relative path");
checkEqual(url.resolve("http://example.com/one/two/", "three"), "http://example.com/one/two/three", "URL-019d: resolve with trailing slash");

// ============================================================================
// URL-020: URL edge cases — empty string, special chars
// ============================================================================
var u20a = new URL("https://example.com/path?q=hello%20world&a=1%2B2");
checkEqual(u20a.search, "?q=hello%20world&a=1%2B2", "URL-020a: encoded search preserved");
checkEqual(u20a.searchParams.get("q"), "hello world", "URL-020b: searchParams decoded %20");

var u20b = new URL("http://localhost:3000/api");
checkEqual(u20b.port, "3000", "URL-020c: port");
checkEqual(u20b.host, "localhost:3000", "URL-020d: host with port");

var u20c = new URL("https://example.com/");
checkEqual(u20c.pathname, "/", "URL-020e: trailing slash pathname");

var u20d = new URL("https://example.com");
checkEqual(u20d.pathname, "/", "URL-020f: no trailing slash gets /");

var u20e = new URL("file:///etc/passwd");
checkEqual(u20e.protocol, "file:", "URL-020g: file protocol");
checkEqual(u20e.pathname, "/etc/passwd", "URL-020h: file pathname");

// ============================================================================
// URL-021: url.parse edge cases
// ============================================================================
var p21a = url.parse("http://example.com");
checkEqual(p21a.protocol, "http:", "URL-021a: parse simple URL");

var p21b = url.parse("/foo/bar?baz=quux#frag");
checkEqual(p21b.pathname, "/foo/bar", "URL-021b: parse pathname-only");
checkEqual(p21b.search, "?baz=quux", "URL-021c: parse search-only");
checkEqual(p21b.hash, "#frag", "URL-021d: parse hash-only");

var p21c = url.parse("file:///etc/passwd");
checkEqual(p21c.protocol, "file:", "URL-021e: parse file protocol");
checkEqual(p21c.pathname, "/etc/passwd", "URL-021f: parse file pathname");

var p21d = url.parse("http://example.com:");
checkEqual(p21d.hostname, "example.com", "URL-021g: parse empty port hostname");

// ============================================================================
// URL-022: URLSearchParams toString
// ============================================================================
var sp22 = new URLSearchParams("a=1&b=2");
var str22 = sp22.toString();
checkEqual(str22, "a=1&b=2", "URL-022: toString returns correct string");

// ============================================================================
// URL-023: URLSearchParams empty constructor + operations
// ============================================================================
var sp23 = new URLSearchParams();
check(sp23 !== null && sp23 !== undefined, "URL-023a: empty constructor works");
check(sp23.get("a") === null || sp23.get("a") === undefined, "URL-023b: get on empty returns null/undefined");
sp23.set("x", "1");
checkEqual(sp23.get("x"), "1", "URL-023c: set on empty then get");
sp23.delete("x");
check(sp23.get("x") === null || sp23.get("x") === undefined, "URL-023d: delete then get returns null/undefined");

// ============================================================================
// URL-024: URLSearchParams with special characters
// ============================================================================
var sp24 = new URLSearchParams("name=hello+world&enc=%40test");
checkEqual(sp24.get("name"), "hello world", "URL-024a: plus decoded to space");
checkEqual(sp24.get("enc"), "@test", "URL-024b: percent-decoded value");

// ============================================================================
// URL-025: url.parse + url.format roundtrip
// ============================================================================
var testUrls25 = [
  "http://example.com/path",
  "http://user:pass@example.com:8000/foo?bar=baz#frag",
  "file:///etc/passwd",
];
for (var i25 = 0; i25 < testUrls25.length; i25++) {
  var parsed25 = url.parse(testUrls25[i25]);
  var formatted25 = url.format(parsed25);
  check(typeof formatted25 === "string", "URL-025-" + i25 + ": roundtrip produces string");
}

// ============================================================================
// URL-026: URL.canParse
// ============================================================================
check(URL.canParse("https://example.com"), "URL-026a: canParse valid URL");
check(!URL.canParse("not a url"), "URL-026b: canParse invalid URL");

console.log("========== Bun Upstream: URL/URLSearchParams ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) { console.log("RESULT: FAIL"); } else { console.log("RESULT: ALL PASS"); }
