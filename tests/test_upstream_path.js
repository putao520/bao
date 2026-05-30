// Bun upstream path.join test adapted for Bao
// Source: ~/code/rust/bun/test/js/node/path/join.test.js
import { describe, test } from "bun:test";
import assert from "node:assert";
import path from "node:path";

var passed = 0;
var failed = 0;
function check(actual, expected, label) {
  if (actual === expected) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual));
    failed++;
  }
}

// path.join tests from Bun upstream
var joinTests = [
  [[".", "x/b", "..", "/b/c.js"], "x/b/c.js"],
  [[], "."],
  [["/.", "x/b", "..", "/b/c.js"], "/x/b/c.js"],
  [["/foo", "../../../bar"], "/bar"],
  [["foo", "../../../bar"], "../../bar"],
  [["foo/", "../../../bar"], "../../bar"],
  [["foo/x", "../../../bar"], "../bar"],
  [["foo/x", "./bar"], "foo/x/bar"],
  [["foo/x/", "./bar"], "foo/x/bar"],
  [["foo/x/", ".", "bar"], "foo/x/bar"],
  [["./"], "./"],
  [[".", "./"], "./"],
  [[".", ".", "."], "."],
  [[".", "./", "."], "."],
  [[".", "/./", "."], "."],
  [[".", "/////./", "."], "."],
  [["."], "."],
  [["", "."], "."],
  [["", "foo"], "foo"],
  [["foo", "/bar"], "foo/bar"],
  [["", "/foo"], "/foo"],
  [["", "", "/foo"], "/foo"],
  [["", "", "foo"], "foo"],
  [["foo", ""], "foo"],
  [["foo/", ""], "foo/"],
  [["foo", "", "/bar"], "foo/bar"],
  [["./", "..", "/foo"], "../foo"],
  [["./", "..", "..", "/foo"], "../../foo"],
  [[".", "..", "..", "/foo"], "../../foo"],
  [["", "..", "..", "/foo"], "../../foo"],
  [["/"], "/"],
  [["/", "."], "/"],
  [["/", ".."], "/"],
  [["/", "..", ".."], "/"],
  [[""], "."],
  [["", ""], "."],
  [[" /foo"], " /foo"],
  [[" ", "foo"], " /foo"],
  [[" ", "."], " "],
  [[" ", "/"], " /"],
  [[" ", ""], " "],
  [["/", "foo"], "/foo"],
  [["/", "/foo"], "/foo"],
  [["/", "//foo"], "/foo"],
  [["/", "", "/foo"], "/foo"],
  [["", "/", "foo"], "/foo"],
];

for (var i = 0; i < joinTests.length; i++) {
  var args = joinTests[i][0];
  var expected = joinTests[i][1];
  var actual = path.posix.join.apply(null, args);
  check(actual, expected, "posix.join(" + JSON.stringify(args) + ")");
}

// path.resolve tests
check(path.resolve("a/b", "c"), path.join(path.resolve("."), "a/b/c"), "resolve a/b,c");
check(path.resolve("/a", "/b"), "/b", "resolve /a,/b");

// path.dirname tests
check(path.dirname("/a/b/c"), "/a/b", "dirname /a/b/c");
check(path.dirname("/a/b"), "/a", "dirname /a/b");
check(path.dirname("/a"), "/", "dirname /a");
check(path.dirname("a"), ".", "dirname a");
check(path.dirname("."), ".", "dirname .");

// path.basename tests
check(path.basename("/a/b/c.txt"), "c.txt", "basename /a/b/c.txt");
check(path.basename("/a/b/c.txt", ".txt"), "c", "basename extstrip");
check(path.basename("/a/b/"), "b", "basename trailing slash");

// path.extname tests
check(path.extname("file.txt"), ".txt", "extname .txt");
check(path.extname("file.tar.gz"), ".gz", "extname .tar.gz");
check(path.extname("file"), "", "extname no ext");
check(path.extname(".hidden"), "", "extname hidden");
check(path.extname(".hidden.txt"), ".txt", "extname hidden+ext");

// path.normalize tests
check(path.normalize("/foo/bar//baz/asdf/quux/.."), "/foo/bar/baz/asdf", "normalize double-slash");
check(path.normalize("./a/b/../c"), "a/c", "normalize relative");
check(path.normalize(""), ".", "normalize empty");

// path.isAbsolute tests
check(path.isAbsolute("/foo"), true, "isAbsolute /foo");
check(path.isAbsolute("foo"), false, "isAbsolute foo");
check(path.isAbsolute("./foo"), false, "isAbsolute ./foo");
check(path.isAbsolute("../foo"), false, "isAbsolute ../foo");

// path.parse/format roundtrip
var parsed = path.parse("/home/user/dir/file.txt");
check(parsed.root, "/", "parse root");
check(parsed.dir, "/home/user/dir", "parse dir");
check(parsed.base, "file.txt", "parse base");
check(parsed.ext, ".txt", "parse ext");
check(parsed.name, "file", "parse name");
var formatted = path.format(parsed);
check(formatted, "/home/user/dir/file.txt", "format roundtrip");

// path.sep
check(typeof path.sep, "string", "sep is string");
check(path.sep.length > 0, true, "sep non-empty");

// path.posix vs path.win32
check(typeof path.posix.join, "function", "posix.join exists");
check(typeof path.win32.join, "function", "win32.join exists");
check(typeof path.posix.resolve, "function", "posix.resolve exists");
check(typeof path.win32.resolve, "function", "win32.resolve exists");

console.log("========== Bun Upstream: path module ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) { console.log("RESULT: FAIL"); } else { console.log("RESULT: ALL PASS"); }
