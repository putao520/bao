# Node.js Conformance Gap Report — bao_runtime

This report documents every deviation and missing API found by the
`tests/node_conformance/` suite, ported from `~/code/rust/bun/test/js/node/`
(MIT, Bun project) against `bao_runtime`'s `node_*` modules.

**Suite status**: 10 modules × (1 main suite + N `#[ignore]` gap tests).
All implemented-API checks PASS. All gap checks are `#[ignore]` with a
`bao_runtime:` annotation describing the deviation. Run `cargo test --include-ignored`
to surface the gaps as expected failures.

## Summary

| Module | Implemented checks | Gap tests (`#[ignore]`) | Conformance % |
|--------|-------------------:|------------------------:|--------------:|
| buffer | 38 | 5 | ~88% |
| path   | 32 | 2 | ~94% |
| fs     | 22 | 4 | ~85% |
| crypto | 28 | 6 | ~82% |
| url    | 17 | 2 | ~89% |
| events | 26 | 3 | ~90% |
| assert | 25 | 3 | ~89% |
| util   | 25 | 5 | ~83% |
| stream | 12 | 2 | ~86% |
| http   | 14 | 6 | ~70% |
| **Total** | **239** | **38** | **~86%** |

---

## Module-by-module gaps

### 1. node:buffer

**Implemented (PASS)**: `alloc/allocUnsafe/allocUnsafeSlow`, `from(string/hex/base64/array/buffer/Uint8Array)`, `concat` (without totalLength), `toString(utf8/hex/ascii/base64/latin1)`, `indexOf(string/byte)`, `includes(string)`, `slice/subarray`, `equals/compare`, `write/copy/fill`, `isBuffer`, `byteLength(string/utf8)`.

**Gaps (`#[ignore]`)**:
- **`Buffer.poolSize`** — not exposed via `require('buffer')`. Node.js exposes it as a number (default 8192).
- **`Buffer.from(str, "base64url")`** — `base64url` encoding not implemented.
- **`Buffer.concat(list, totalLength)`** — DEVIATION: the `totalLength` argument is ignored. Node.js truncates/pads to `totalLength` bytes; bao returns the full concatenated length.
- **`Buffer.prototype.includes(Buffer)` / `indexOf(Buffer)`** — DEVIATION: only string/number args work. Passing a Buffer (Node.js supports this) returns `-1`/`false`.
- **`Buffer.byteLength(str, "hex")`** — DEVIATION: returns raw string length, not decoded byte count. Node.js decodes hex pairs.

### 2. node:path

**Implemented (PASS)**: `join`, `resolve`, `dirname`, `basename` (with/without ext), `extname` (all edge cases), `normalize`, `relative`, `isAbsolute`, `parse`, `format`, `sep`, `delimiter`, `posix.*` (join/isAbsolute/sep).

**Gaps (`#[ignore]`)**:
- **`path.matchesGlob(path, pattern)`** — not implemented.
- **`path.win32`** — DEVIATION: aliases `path` itself on Linux, so `win32.sep` returns `"/"` instead of `"\\"`. Node.js ships a real Windows implementation on all platforms.

### 3. node:fs

**Implemented (PASS)**: `writeFileSync`, `readFileSync` (with encoding), `appendFileSync`, `statSync` (size/isFile/isDirectory), `existsSync`, `mkdirSync`, `readdirSync`, `rmdirSync`, `rmSync` (recursive), `renameSync`, `copyFileSync`, `unlinkSync`, `realpathSync`, `promises.writeFile/readFile`.

**Gaps (`#[ignore]`)**:
- **`readFileSync(path)` without encoding** — DEVIATION: returns a `String` (utf8-decoded), not a `Buffer`. Node.js always returns `Buffer` when no encoding is given.
- **`fs.createReadStream` / `ReadStream`** — not implemented. Node.js exposes a streaming read API.
- **`fs.watch` / `fs.watchFile`** — not implemented.
- **`fs.cp` / `cpSync`** (recursive directory copy) — not implemented.

### 4. node:crypto

**Implemented (PASS)**: `createHash(md5/sha1/sha256/sha512)`, `createHmac(sha1/sha256)`, `randomBytes` (size property), `randomUUID` (format + version nibble), `pbkdf2Sync`, `timingSafeEqual`, `createCipheriv`/`createDecipheriv` (AES-256-CBC roundtrip), `getHashes`, `getCiphers`, `subtle`.

**Gaps (`#[ignore]`)**:
- **`createHmac("md5", ...)`** — DEVIATION: throws "Unsupported HMAC algorithm: md5". Node.js supports HMAC-MD5.
- **`randomBytes(N)` return type** — DEVIATION: returns a generic object (Uint8Array-like), not a `Buffer` instance. `Buffer.isBuffer(crypto.randomBytes(8)) === false`.
- **`createECDH(curve)`** — not implemented.
- **`X509` / `createX509`** — not implemented.
- **`hkdf` / `hkdfSync`** — not implemented.
- **`createDiffieHellman` / `DiffieHellmanGroup`** — not implemented.

### 5. node:url

**Implemented (PASS)**: `URL` constructor (protocol/host/hostname/port/pathname/username/href/hash/search), `URLSearchParams` (get/append/has/delete via URL instance), `url.resolve`, `url.parse` (legacy, protocol/hostname/port), `url.format`, `URL.canParse` (skip-if-missing), `URL.toJSON` / `JSON.stringify(URL)`.

**Gaps (`#[ignore]`)**:
- **`url.pathToFileURL(path)` / `fileURLToPath(url)`** — not implemented.
- **`url.domainToASCII(domain)` / `domainToUnicode(domain)`** — not implemented.

### 6. node:events

**Implemented (PASS)**: `EventEmitter` constructor + instance, `on/emit` (return true/false), `once`, `removeListener/off`, `removeAllListeners(event/all)`, `listenerCount`, `listeners`, `eventNames`, `setMaxListeners`/`getMaxListeners`, `prependListener`, static `on/once/getEventListeners`.

**Gaps (`#[ignore]`)**:
- **`EventEmitter.defaultMaxListeners`** — DEVIATION: not exposed on the constructor (`undefined`). Node.js exposes a writable number (default 10).
- **`EventEmitter.captureRejections`** option — not implemented.
- **`EventEmitter.errorMonitor`** symbol — not implemented.

### 7. node:assert

**Implemented (PASS)**: `ok` (truthy + throws on falsy/null/undefined/false), `equal` (loose coercion), `strictEqual` (no coercion), `notEqual`, `notStrictEqual`, `deepEqual` (object + throws on diff), `deepStrictEqual` (strict + throws on coercion), `throws` (matches class/regex), `doesNotThrow`, `fail`, `ifError` (null/undefined/error), `assert/strict` submodule.

**Gaps (`#[ignore]`)**:
- **`assert.match(string, regex)` / `assert.doesNotMatch`** — not exposed on the assert object. Node.js ships these.
- **`assert.rejects(asyncFn)` / `assert.doesNotReject`** — not implemented.
- **`assert.CallTracker`** — not implemented.

### 8. node:util

**Implemented (PASS)**: `isString/isNumber/isBoolean/isFunction/isObject/isArray/isNull/isUndefined/isDate/isRegExp/isError/isSymbol`, `inspect` (string/array/null/primitives), `format` (%s/%d/extra args), `promisify` (returns function), `callbackify`, `isDeepStrictEqual` (primitives only), `util.types` (isPromise/isNativeError), `deprecate/inherits/getSystemErrorName/parseArgs`.

**Gaps (`#[ignore]`)**:
- **`util.inspect(obj)`** — DEVIATION: returns `"[Object]"` for plain objects instead of the property listing (`{ a: 1 }`). Node.js walks properties.
- **`util.promisify(fn)`** — DEVIATION: returns a `function`, not a thenable/Promise. Node.js returns a function that returns a Promise.
- **`util.isDeepStrictEqual(objA, objB)`** — DEVIATION: only works for primitives. Objects always compare unequal. Node.js deep-compares.
- **`util.types.isExternal` / `isKeyObject` / `isCryptoKey`** — not implemented.
- **`util.styleText(format, text)`** — not implemented.

### 9. node:stream

**Implemented (PASS)**: module exports `Readable`/`Writable`/`Duplex`/`Transform`/`PassThrough` constructors, `pipeline`, `finished`; `new Readable({read})` instance + `push`/`read`; `new Writable({write})` instance + `write`/`end`; `new PassThrough()`.

**Gaps (`#[ignore]`)**:
- **`stream.Readable.from(iterable)`** async iterator — not implemented.
- **Stream Web API** (`stream.ReadableStream`/`WritableStream`) — not exposed on the stream module. (Web Streams may exist globally via servo; this is about the node:stream re-export.)

### 10. node:http

**Implemented (PASS)**: `createServer`, `request`, `get`, `METHODS` (as string — see gap), `STATUS_CODES` (200/404/301/500), `Server` constructor, server `listen`/`close`.

**Gaps (`#[ignore]`)**:
- **`http.METHODS`** — DEVIATION: exposed as a comma-separated string (`"GET,POST,..."`), not an array. Node.js exposes an array.
- **`http.Server` instances** — DEVIATION: not EventEmitters (no `.on`/`.emit`). Node.js servers extend EventEmitter for `listening`/`request`/`close` events.
- **`http.ClientRequest` / `IncomingMessage` / `OutgoingMessage`** — not exposed as named classes on the module.
- **`http.Agent` / `http.globalAgent`** — not exposed.
- **`http.maxRedirects`** — not exposed.
- **`http.validateHeaderName` / `validateHeaderValue`** — not exposed.

---

## Cross-cutting themes

1. **Type fidelity**: Several crypto/buffer/fs APIs return generic objects or strings where Node.js returns `Buffer` instances. This breaks code that relies on `Buffer.isBuffer()` or Buffer methods on the return value.
2. **Object deep semantics**: `util.isDeepStrictEqual` and `assert.deepEqual` object comparison is incomplete in some paths; worth a follow-up audit against the Node.js deep-equal algorithm.
3. **Class exposure**: `http.ClientRequest/IncomingMessage/OutgoingMessage` and similar typed constructors are absent — code that does `req instanceof http.IncomingMessage` will fail.
4. **EventEmitter integration**: `http.Server` is not an EventEmitter, so the standard `server.on("request", ...)` pattern from Node.js tutorials does not work.
5. **Encoding gaps**: `base64url`, hex-aware `byteLength`, and hex-aware `Buffer.includes`/`indexOf` are missing.

## How to use this report

- The `#[ignore]` annotations on each gap test encode the same text, so grepping
  the test files for `bao_runtime:` will surface every gap inline.
- Run `cargo test -p bun_runtime --test <module>_conformance -- --include-ignored`
  to see the gaps as failing tests (they assert the Node.js-correct behavior,
  which bao_runtime does not yet produce).
- When implementing a gap, remove the `#[ignore]` attribute and merge the gap
  test's checks into the main suite, or leave it standalone.
