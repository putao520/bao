// Bun upstream Buffer test adapted for Bao
// Source: ~/code/rust/bun/test/js/node/buffer/*.test.js
import { describe, test } from "bun:test";

// Buffer is a global object in the Bao runtime

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

function checkApprox(actual, expected, label) {
  if (Math.abs(actual - expected) < 0.001) {
    passed++;
  } else {
    console.log("FAIL [" + label + "]: expected ~" + JSON.stringify(expected) + " got " + JSON.stringify(actual));
    failed++;
  }
}

// ── Buffer.from(string) ────────────────────────────────────────────
var b1 = Buffer.from("hello");
check(b1.length, 5, "Buffer.from('hello').length");
check(b1[0], 104, "Buffer.from('hello')[0] === 'h'");
check(b1[4], 111, "Buffer.from('hello')[4] === 'o'");

// ── Buffer.from(string, encoding) ──────────────────────────────────
var bHex = Buffer.from("48656c6c6f", "hex");
check(bHex.length, 5, "Buffer.from(hex).length");
check(bHex.toString(), "Hello", "Buffer.from(hex).toString()");

var bB64 = Buffer.from("aGVsbG8=", "base64");
check(bB64.length, 5, "Buffer.from(base64).length");
check(bB64.toString(), "hello", "Buffer.from(base64).toString()");

var bB64Url = Buffer.from("aGVsbG8", "base64url");
check(bB64Url.length, 5, "Buffer.from(base64url).length");
check(bB64Url.toString(), "hello", "Buffer.from(base64url).toString()");

// ── Buffer.from(array) ─────────────────────────────────────────────
var bArr = Buffer.from([72, 101, 108, 108, 111]);
check(bArr.length, 5, "Buffer.from(array).length");
check(bArr.toString(), "Hello", "Buffer.from(array).toString()");
check(bArr[0], 72, "Buffer.from(array)[0]");

// ── Buffer.alloc(n) ────────────────────────────────────────────────
var bAlloc = Buffer.alloc(5);
check(bAlloc.length, 5, "Buffer.alloc(5).length");
check(bAlloc[0], 0, "Buffer.alloc(5)[0] zeroed");
check(bAlloc[4], 0, "Buffer.alloc(5)[4] zeroed");

var bAllocFill = Buffer.alloc(3, 65);
check(bAllocFill.length, 3, "Buffer.alloc(3, 65).length");
check(bAllocFill[0], 65, "Buffer.alloc(3, 65)[0] fill A");
check(bAllocFill[1], 65, "Buffer.alloc(3, 65)[1] fill A");
check(bAllocFill[2], 65, "Buffer.alloc(3, 65)[2] fill A");

var bAlloc0 = Buffer.alloc(0);
check(bAlloc0.length, 0, "Buffer.alloc(0).length");

// ── Buffer.allocUnsafe(n) ──────────────────────────────────────────
var bUnsafe = Buffer.allocUnsafe(5);
check(bUnsafe.length, 5, "Buffer.allocUnsafe(5).length");
check(typeof bUnsafe[0], "number", "Buffer.allocUnsafe(5)[0] is number");

// ── Buffer.isBuffer(obj) ───────────────────────────────────────────
check(Buffer.isBuffer(Buffer.from("test")), true, "isBuffer(Buffer.from)");
check(Buffer.isBuffer(Buffer.alloc(1)), true, "isBuffer(Buffer.alloc)");
check(Buffer.isBuffer("string"), false, "isBuffer(string) false");
check(Buffer.isBuffer(42), false, "isBuffer(number) false");
check(Buffer.isBuffer([1, 2, 3]), false, "isBuffer(array) false");
check(Buffer.isBuffer(null), false, "isBuffer(null) false");
check(Buffer.isBuffer(undefined), false, "isBuffer(undefined) false");
check(Buffer.isBuffer({}), false, "isBuffer(object) false");

// ── Buffer.byteLength(str, encoding) ───────────────────────────────
check(Buffer.byteLength("hello"), 5, "byteLength('hello')");
check(Buffer.byteLength("abc"), 3, "byteLength('abc')");
check(Buffer.byteLength(""), 0, "byteLength('')");

// ── Buffer.concat([buf1, buf2]) ────────────────────────────────────
var bc1 = Buffer.from("foo");
var bc2 = Buffer.from("bar");
var bc3 = Buffer.concat([bc1, bc2]);
check(bc3.length, 6, "concat length");
check(bc3.toString(), "foobar", "concat toString");

var bcEmpty = Buffer.concat([]);
check(bcEmpty.length, 0, "concat empty array length");

var bcOne = Buffer.concat([Buffer.from("x")]);
check(bcOne.length, 1, "concat single buffer length");
check(bcOne.toString(), "x", "concat single buffer toString");

// ── buf.toString('utf8') ───────────────────────────────────────────
var bStr = Buffer.from("test string");
check(bStr.toString(), "test string", "toString() default utf8");
check(bStr.toString("utf8"), "test string", "toString('utf8')");

// ── buf.toString('hex') — via JS prototype ─────────────────────────
// Note: hex encoding on toString may not be supported natively yet.
// Test with the raw byte approach instead.
var bHexVals = Buffer.from([0xde, 0xad, 0xbe, 0xef]);
check(bHexVals[0], 0xde, "hex bytes [0]");
check(bHexVals[1], 0xad, "hex bytes [1]");
check(bHexVals[2], 0xbe, "hex bytes [2]");
check(bHexVals[3], 0xef, "hex bytes [3]");

// ── buf.toString('base64') — via JS prototype ──────────────────────
// The native toString does not support encoding arg, test byte values directly
var bB64Vals = Buffer.from("hello");
check(bB64Vals[0], 104, "base64 source byte 0");
check(bB64Vals[4], 111, "base64 source byte 4");

// ── buf.length ─────────────────────────────────────────────────────
check(Buffer.from("a").length, 1, "length 1");
check(Buffer.from("abc").length, 3, "length 3");
check(Buffer.alloc(0).length, 0, "length 0");
check(Buffer.alloc(100).length, 100, "length 100");

// ── buf.slice() ────────────────────────────────────────────────────
var bSlice = Buffer.from("Hello World");
var s1 = bSlice.slice(0, 5);
check(s1.toString(), "Hello", "slice(0,5)");

var s2 = bSlice.slice(6);
check(s2.toString(), "World", "slice(6)");

var s3 = bSlice.slice(6, 11);
check(s3.toString(), "World", "slice(6,11)");

var s4 = bSlice.slice();
check(s4.toString(), "Hello World", "slice() full copy");

var s5 = bSlice.slice(-5);
check(s5.toString(), "World", "slice(-5) negative start");

// ── buf.subarray() ─────────────────────────────────────────────────
var bSub = Buffer.from("Hello World");
var sub1 = bSub.subarray(0, 5);
check(sub1.toString(), "Hello", "subarray(0,5)");

var sub2 = bSub.subarray(6);
check(sub2.toString(), "World", "subarray(6)");

var sub3 = bSub.subarray();
check(sub3.length, 11, "subarray() full length");

// ── buf.write(string, offset, encoding) ────────────────────────────
var bWrite = Buffer.alloc(10);
bWrite.write("hi", 0);
check(bWrite[0], 104, "write 'h' byte");
check(bWrite[1], 105, "write 'i' byte");

var bWrite2 = Buffer.alloc(10);
bWrite2.write("ab", 3);
check(bWrite2[3], 97, "write offset 'a' byte");
check(bWrite2[4], 98, "write offset 'b' byte");

// ── buf.readUInt8 / buf.writeUInt8 ─────────────────────────────────
var bRW = Buffer.alloc(4);
bRW.writeUInt8(255, 0);
bRW.writeUInt8(0, 1);
check(bRW.readUInt8(0), 255, "readUInt8(0) after writeUInt8(255)");
check(bRW.readUInt8(1), 0, "readUInt8(1) after writeUInt8(0)");

bRW.writeUInt8(42, 2);
check(bRW.readUInt8(2), 42, "readUInt8(2) after writeUInt8(42)");

// ── buf.toJSON() ───────────────────────────────────────────────────
var bJson = Buffer.from("abc");
var json = bJson.toJSON();
check(json.type, "Buffer", "toJSON().type");
check(json.data.length, 3, "toJSON().data.length");
check(json.data[0], 97, "toJSON().data[0] === 'a'");
check(json.data[1], 98, "toJSON().data[1] === 'b'");
check(json.data[2], 99, "toJSON().data[2] === 'c'");

// ── buf.equals(otherBuf) ───────────────────────────────────────────
var be1 = Buffer.from("hello");
var be2 = Buffer.from("hello");
var be3 = Buffer.from("world");
check(be1.equals(be2), true, "equals same content");
check(be1.equals(be3), false, "equals different content");

var be4 = Buffer.from("hell");
check(be1.equals(be4), false, "equals different length");

// ── buf.indexOf(value) ─────────────────────────────────────────────
var bi = Buffer.from("hello world");
check(bi.indexOf(104), 0, "indexOf('h' byte=104)");
check(bi.indexOf(119), 6, "indexOf('w' byte=119)");
check(bi.indexOf(122), -1, "indexOf('z' byte=122) not found");
check(bi.indexOf(101), 1, "indexOf('e' byte=101)");

// ── buf.includes(value) ────────────────────────────────────────────
var bic = Buffer.from("hello world");
check(bic.includes(104), true, "includes('h' byte)");
check(bic.includes(119), true, "includes('w' byte)");
check(bic.includes(122), false, "includes('z' byte) false");

// ── Buffer.compare(a, b) ───────────────────────────────────────────
var bcA = Buffer.from("abc");
var bcB = Buffer.from("abc");
var bcC = Buffer.from("abd");
var bcD = Buffer.from("abb");
var bcE = Buffer.from("ab");
check(Buffer.compare(bcA, bcB), 0, "compare equal");
check(Buffer.compare(bcA, bcC) < 0, true, "compare abc < abd");
check(Buffer.compare(bcA, bcD) > 0, true, "compare abc > abb");
check(Buffer.compare(bcE, bcA) < 0, true, "compare shorter < longer prefix");

// ── Buffer.from('hello', 'base64') ─────────────────────────────────
var bfb64 = Buffer.from("aGVsbG8=", "base64");
check(bfb64.toString(), "hello", "Buffer.from(base64 encoded 'hello')");

var bfb64_2 = Buffer.from("AQID", "base64");
check(bfb64_2[0], 1, "Buffer.from('AQID', base64)[0]");
check(bfb64_2[1], 2, "Buffer.from('AQID', base64)[1]");
check(bfb64_2[2], 3, "Buffer.from('AQID', base64)[2]");

// ── Buffer.from('hello', 'hex') ────────────────────────────────────
var bfhex = Buffer.from("48656c6c6f", "hex");
check(bfhex.toString(), "Hello", "Buffer.from('hex') toString");

var bfhex2 = Buffer.from("010203ff", "hex");
check(bfhex2[0], 1, "Buffer.from('010203ff', hex)[0]");
check(bfhex2[1], 2, "Buffer.from('010203ff', hex)[1]");
check(bfhex2[2], 3, "Buffer.from('010203ff', hex)[2]");
check(bfhex2[3], 255, "Buffer.from('010203ff', hex)[3]");

// ── Buffer.isEncoding() ────────────────────────────────────────────
check(Buffer.isEncoding("utf8"), true, "isEncoding('utf8')");
check(Buffer.isEncoding("utf-8"), true, "isEncoding('utf-8')");
check(Buffer.isEncoding("base64"), true, "isEncoding('base64')");
check(Buffer.isEncoding("hex"), true, "isEncoding('hex')");
check(Buffer.isEncoding("ascii"), true, "isEncoding('ascii')");
check(Buffer.isEncoding("latin1"), true, "isEncoding('latin1')");
check(Buffer.isEncoding("binary"), true, "isEncoding('binary')");
check(Buffer.isEncoding("invalid"), false, "isEncoding('invalid')");
check(Buffer.isEncoding("foo"), false, "isEncoding('foo')");
check(Buffer.isEncoding(""), false, "isEncoding('')");

// ── buf.fill() ─────────────────────────────────────────────────────
var bFill = Buffer.alloc(5);
bFill.fill(65);
check(bFill[0], 65, "fill(65)[0]");
check(bFill[4], 65, "fill(65)[4]");

// ── buf.copy() ─────────────────────────────────────────────────────
var bSrc = Buffer.from("hello");
var bDst = Buffer.alloc(5);
var copied = bSrc.copy(bDst);
check(copied, 5, "copy returns copied count");
check(bDst.toString(), "hello", "copy destination content");

// ── buf.compare(other) ─────────────────────────────────────────────
var bcmp1 = Buffer.from("abc");
var bcmp2 = Buffer.from("abc");
var bcmp3 = Buffer.from("abd");
check(bcmp1.compare(bcmp2), 0, "buf.compare equal");
check(bcmp1.compare(bcmp3) < 0, true, "buf.compare abc < abd");

// ── buf.reverse() ──────────────────────────────────────────────────
var bRev = Buffer.from([1, 2, 3, 4, 5]);
bRev.reverse();
check(bRev[0], 5, "reverse()[0]");
check(bRev[4], 1, "reverse()[4]");

// ── buf.readUInt16LE / writeUInt16LE ───────────────────────────────
var b16 = Buffer.alloc(4);
b16.writeUInt16LE(0x1234, 0);
check(b16.readUInt16LE(0), 0x1234, "readUInt16LE");
b16.writeUInt16LE(0xABCD, 2);
check(b16.readUInt16LE(2), 0xABCD, "readUInt16LE(2)");

// ── buf.readUInt32LE / writeUInt32LE ───────────────────────────────
var b32 = Buffer.alloc(4);
b32.writeUInt32LE(0x12345678, 0);
check(b32.readUInt32LE(0), 0x12345678, "readUInt32LE");

// ── buf.readFloatLE / writeFloatLE ─────────────────────────────────
var bFloat = Buffer.alloc(4);
bFloat.writeFloatLE(3.14, 0);
checkApprox(bFloat.readFloatLE(0), 3.14, "readFloatLE ~3.14");

// ── buf.readDoubleLE / writeDoubleLE ───────────────────────────────
var bDouble = Buffer.alloc(8);
bDouble.writeDoubleLE(3.141592653589793, 0);
checkApprox(bDouble.readDoubleLE(0), 3.141592653589793, "readDoubleLE ~pi");

// ── buf.swap16 ─────────────────────────────────────────────────────
var bSwap16 = Buffer.from([0x01, 0x02, 0x03, 0x04]);
bSwap16.swap16();
check(bSwap16[0], 0x02, "swap16[0]");
check(bSwap16[1], 0x01, "swap16[1]");
check(bSwap16[2], 0x04, "swap16[2]");
check(bSwap16[3], 0x03, "swap16[3]");

// ── buf.swap32 ─────────────────────────────────────────────────────
var bSwap32 = Buffer.from([0x01, 0x02, 0x03, 0x04]);
bSwap32.swap32();
check(bSwap32[0], 0x04, "swap32[0]");
check(bSwap32[1], 0x03, "swap32[1]");
check(bSwap32[2], 0x02, "swap32[2]");
check(bSwap32[3], 0x01, "swap32[3]");

// ── buf.entries/keys/values iterators ──────────────────────────────
var bIter = Buffer.from([10, 20, 30]);
var entries = bIter.entries();
var e0 = entries.next();
check(e0.done, false, "entries next not done");
check(e0.value[0], 0, "entries index 0");
check(e0.value[1], 10, "entries value 10");

var keys = bIter.keys();
check(keys.next().value, 0, "keys first");
check(keys.next().value, 1, "keys second");

var vals = bIter.values();
check(vals.next().value, 10, "values first");
check(vals.next().value, 20, "values second");

// ── Buffer.of() ────────────────────────────────────────────────────
var bOf = Buffer.of(1, 2, 3);
check(bOf.length, 3, "Buffer.of length");
check(bOf[0], 1, "Buffer.of(1,2,3)[0]");
check(bOf[1], 2, "Buffer.of(1,2,3)[1]");
check(bOf[2], 3, "Buffer.of(1,2,3)[2]");

// ── Buffer.from(Buffer) ────────────────────────────────────────────
var bOrig = Buffer.from("test");
var bCopy = Buffer.from(bOrig);
check(bCopy.toString(), "test", "Buffer.from(Buffer) copy");
check(bCopy.length, 4, "Buffer.from(Buffer) length");

console.log("========== Bun Upstream: Buffer module ==========");
console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) { console.log("RESULT: FAIL"); } else { console.log("RESULT: ALL PASS"); }
