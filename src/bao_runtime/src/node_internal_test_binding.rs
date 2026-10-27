// @trace REQ-ENG-004 [api:internal/test/binding]
//
// `internal/test/binding` — Node's internal test-only module that exposes
// the raw C++ `internalBinding()` helper (used by Node's own test fixtures
// and by the GOLDEN A oracle: nodejs/node v24.x official test/ files run
// under bao call `internalBinding(name)` directly).
//
// Oracle (nodejs/node v24.19.0, probed via `node --expose-internals`):
// `internalBinding(name)` returns the native binding object per name and
// throws `Error: No such binding: <name>` (code `ERR_INVALID_MODULE`) for
// unknown names — e.g. `internalBinding('debug')` throws on a release node
// because that binding simply does not exist there.
//
// Implementation contract (user ruling: no exemptions, implement the real
// body per the oracle consumption set — never fabricate): each known
// binding is registered with only the faces that have an honest real body;
// faces that need the V8/C++ layer (StreamBase chain, `v8::External`,
// private-symbol invisibility, sigint interposition, iterator-kind
// introspection) stay ABSENT — `undefined` is the honest face, and a test
// failing on `undefined` is a real product diff, not a hidden gap.
// Unknown binding names mirror the oracle throw verbatim.
//
// Consumption sets (GOLDEN A failing class, 18 files — the faces below are
// exactly what those tests destructure or call; nothing else is registered):
// - 'uv'        test-fs-copyfile / -error-messages / -watch-enoent:
//               7 errno constants (asserted against real `err.errno` values)
// - 'util'      test-buffer-backing-arraybuffer (`arrayBufferViewHasBuffer`),
//               test-util-internal (`privateSymbols`), test-util-inspect
//               (`previewEntries`)
// - 'fs'        test-fs-existssync-memleak-longpath (`FSReqCallback`),
//               test-fs-filehandle (`openFileHandle`); the monkeypatch
//               targets (`readdir` / `fstat` / `writeFileUtf8`) are write-only
//               consumers (assignment works on any object; the patch taking
//               effect requires a JS-visible binding seam in bao's fs — the
//               next-layer product diff)
// - 'buffer'    test-buffer-fill (`fill` only — the 5-arg native fast-path
//               form fill(buf, val, start, end, encoding); the two consuming
//               calls are start/end range-throw probes). This is the ONLY
//               internalBinding('buffer') consumer in the golden set
//               (sweep: grep internalBinding('buffer') across the fixture
//               dir — every other buffer test consumes only the global
//               Buffer face), so per the nothing-else rule the other 34
//               binding keys (copy/compare/slice/write families,
//               kMaxLength/kStringMaxLength, swap*/isUtf8/isAscii, …) stay
//               unregistered.
// - 'js_stream' test-util-types / -inspect (`JSStream`) — object exists so the
//               module-level `No such module` class is cleared; the class face
//               stays absent (V8 StreamBase chain is the stop-clause layer)
// - 'contextify' test-util-sigint-watchdog (watchdog trio) — object exists,
//               faces absent (C++ SIGINT interposition is the stop-clause layer)

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const INTERNAL_TEST_BINDING_JS: &str = r#"
(function() {
  'use strict';

  // ---- 'uv' -----------------------------------------------------------------
  // POSIX errno negation: libuv maps system errno verbatim on POSIX
  // (vendor libuv uv/errno.h); values oracle-probed on nodejs/node v24.19.0
  // (internalBinding('uv').UV_*). Asserted by the consuming tests against
  // real fs error `err.errno` values, so these must stay the true numbers.
  const uvBinding = {
    UV_ENOENT: -2,
    UV_EEXIST: -17,
    UV_EINVAL: -22,
    UV_ENOTDIR: -20,
    UV_ENOTEMPTY: -39,
    UV_EPERM: -1,
    UV_ENODEV: -19,
  };

  // ---- 'util' ---------------------------------------------------------------
  const utilBinding = {
    // SM ArrayBufferViews always own their buffer (no V8 on-heap small-view
    // optimization exists here), so every real view reports true — the
    // honest engine semantics; consuming assertions that expect on-heap
    // small views fail on that real engine diff (next layer).
    arrayBufferViewHasBuffer(view) {
      return require('util').types.isArrayBufferView(view);
    },

    // Flat iterator preview. Registered boundary (engine gap): SM exposes no
    // Map/Set iterator-kind introspection and no non-consuming table peek,
    // so node's flag form (`[entries, isKeyValue]` tuple) and the
    // set-of-entry-arrays shape cannot be told apart honestly. Entry arrays
    // are flattened once, which matches the map-entries flat preview the
    // consuming tests exercise; the flag argument is accepted and ignored
    // (its kind-dependent form is the next layer). Consumes the iterator —
    // node peeks the backing table without consuming (same engine gap).
    previewEntries(iterator, _isKeyValue) {
      if (iterator === null || typeof iterator !== 'object' ||
          typeof iterator.next !== 'function') {
        return undefined;
      }
      const out = [];
      for (const value of iterator) {
        if (Array.isArray(value)) {
          out.push(value[0], value[1]);
        } else {
          out.push(value);
        }
      }
      return out;
    },

    privateSymbols: {
      // Real unique symbol identities. Node: V8 private symbols (invisible
      // to Reflect.ownKeys and never set on engine errors); SM has no
      // private-symbol surface, so the invisibility and error-decoration
      // assertions are the next-layer product diff.
      arrow_message_private_symbol: Symbol('arrow_message_private_symbol'),
    },
  };

  // ---- 'fs' -----------------------------------------------------------------
  const fsBinding = {
    // Real state-carrying class mirroring the C++ construction contract
    // (cb, useCheck, usePromises); the threadpool integration face is the
    // next layer and stays absent. Consumers (common/gc.js
    // checkIfCollectableByCounting) only need the constructible, named
    // class identity for v8.queryObjects counting.
    FSReqCallback: class FSReqCallback {
      constructor(cb, useCheck, usePromises) {
        this.cb = cb;
        this.useCheck = useCheck;
        this.usePromises = usePromises;
      }
    },

    // Real wrapper over bao's fs.openSync. Node contract: returns `{ fd }`
    // and leaves `ctx` untouched on success; reports via `ctx.errno` and
    // returns undefined on failure (lib/fs.js ctx out-param convention).
    openFileHandle(path, flags, mode, _onDone, ctx) {
      try {
        return { fd: require('fs').openSync(path, flags, mode) };
      } catch (err) {
        if (ctx !== null && typeof ctx === 'object') {
          ctx.errno = err.errno;
        }
        return undefined;
      }
    },
  };

  // ---- 'buffer' ---------------------------------------------------------------
  // native `Fill` fast path (node src/node_buffer.cc), reached by tests as
  // internalBinding('buffer').fill with the 5-arg POSITIONAL form
  // fill(buf, val, start, end, encoding). Oracle-probed on node v24.19.0
  // (`--expose-internals`); this native face is intentionally DIFFERENT from
  // Buffer.prototype.fill — the JS face validates encoding type/value FIRST
  // (fill('a', 0, 0, NaN) → ERR_INVALID_ARG_TYPE; fill('a',0,0,'foo') →
  // ERR_UNKNOWN_ENCODING; hex fill validates strictly) while the native face:
  //   - range-checks start/end FIRST (before any encoding resolution) with
  //     the terse `RangeError [ERR_OUT_OF_RANGE]: Index out of range`
  //     (exactly the two shapes test-buffer-fill asserts);
  //   - treats end beyond the buffer length as a SILENT NO-OP (not a clamp,
  //     not a throw — fill(65, 3, 7) on a 6-byte buffer fills nothing);
  //   - every encoding the native ParseEncoding cannot resolve — numbers,
  //     NaN, false, '', 'foo', and even 'utf16be'/'utf-16be' (not a native
  //     encoding; the JS face does accept them) — silently falls back to
  //     UTF8, never throws;
  //   - hex STRING values use the lenient native hex write: decode stops at
  //     the first non-hex PAIR and the valid prefix is tiled
  //     ('yKJh' → 0 bytes → no-op; '6162zz' → tiles '6162'; '616' → tiles
  //     '61'; the JS face's hex path would ERR_INVALID_ARG_VALUE-throw);
  //   - returns undefined (not the buffer).
  // The byte-fill algorithm itself (per-encoding byte production + tiling)
  // is byte-equivalent to the GLOBAL Buffer.prototype.fill (the E22
  // oracle-aligned face), so after the native-face sanitization above the
  // call delegates to it — the algorithm is not rewritten (reuse ruling).
  // The global `Buffer` is resolved at CALL time (never at install time) and
  // is the same object as require('buffer').Buffer (node_buffer.rs locates
  // the global constructor), i.e. the very face the consuming test uses.
  const bufferBinding = {
    fill(buf, val, start, end, encoding) {
      // byteLength (not the user-visible `length`) — same defineProperty
      // immunity rationale as _bp.fill in the Buffer blob.
      var len = (buf && typeof buf.byteLength === 'number') ? buf.byteLength : buf.length;
      var s = (start === undefined || start === null) ? 0 : (+start);
      var e = (end === undefined || end === null) ? len : (+end);
      if (!isFinite(s)) s = 0;
      if (!isFinite(e)) e = len;
      // Native order: range BEFORE encoding. Both consuming calls land here.
      if (s < 0 || e < 0) {
        var err = new RangeError('Index out of range');
        err.code = 'ERR_OUT_OF_RANGE';
        throw err;
      }
      // Native-exact: end beyond the length is a silent NO-OP (probed:
      // fill(65, 3, 7) on a 6-byte buffer fills nothing — not a clamp).
      if (e > len) return undefined;
      // Only reachable with e <= len; a start beyond that sits past the
      // range end, so clamp it into the (now certainly non-throwing) JS
      // face — the resulting e <= s range is the native no-op.
      if (s > len) s = len;
      // Native honored set (case-insensitive, probed): utf8/utf-8,
      // utf16le/utf-16le, ucs2/ucs-2, latin1, binary, ascii, base64,
      // base64url, hex. Everything else → UTF8 default. Note this set is
      // NARROWER than the JS face's (no utf16be/utf-16be) — kept exact to
      // the probed native behavior. Normalized to lowercase here (the JS
      // face lowercases internally anyway; the hex branch below needs the
      // canonical form).
      var ENCODINGS = {
        utf8: 1, 'utf-8': 1, utf16le: 1, 'utf-16le': 1, ucs2: 1, 'ucs-2': 1,
        latin1: 1, binary: 1, ascii: 1, base64: 1, base64url: 1, hex: 1,
      };
      if (typeof encoding === 'string' && ENCODINGS[encoding.toLowerCase()]) {
        encoding = encoding.toLowerCase();
      } else {
        encoding = undefined;
      }
      // Native hex face for STRING values: lenient valid-prefix pairing.
      // Pre-decode here (the JS face's hex path throws instead) and hand the
      // JS face the decoded prefix as an object fill, whose tiling produces
      // the native bytes. A zero-byte prefix (empty or first-pair-invalid)
      // hits the JS face's empty-source no-op = native parity.
      if (encoding === 'hex' && typeof val === 'string') {
        var bytes = [];
        for (var i = 0; i + 1 < val.length; i += 2) {
          var hi = val.charCodeAt(i);
          var lo = val.charCodeAt(i + 1);
          hi = (hi >= 48 && hi <= 57) ? hi - 48 :
               (hi >= 97 && hi <= 102) ? hi - 87 :
               (hi >= 65 && hi <= 70) ? hi - 55 : -1;
          lo = (lo >= 48 && lo <= 57) ? lo - 48 :
               (lo >= 97 && lo <= 102) ? lo - 87 :
               (lo >= 65 && lo <= 70) ? lo - 55 : -1;
          if (hi < 0 || lo < 0) break;
          bytes.push((hi << 4) | lo);
        }
        Buffer.prototype.fill.call(buf, bytes, s, e);
        return undefined;
      }
      // Delegate the algorithm (positional args are now sanitized into the
      // JS face's happy path: numbers in range + valid/undefined encoding —
      // the JS face's encoding-first validation can no longer throw here).
      Buffer.prototype.fill.call(buf, val, s, e, encoding);
      return undefined;
    },
  };

  // ---- binding dispatch -----------------------------------------------------
  // Same object identity per name across calls (node caches
  // bindingObj[module]) so monkeypatched faces stay visible to later reads.
  const knownBindings = {
    uv: uvBinding,
    util: utilBinding,
    fs: fsBinding,
    buffer: bufferBinding,
    // Consumed faces require the V8/C++ layer (stop-clause registrations):
    // js_stream → JSStream/_externalStream (V8 StreamBase chain, v8::External);
    // contextify → start/stopSigintWatchdog + watchdogHasPendingSigint
    // (C++ SIGINT interposition). The binding OBJECT exists (clears the
    // module-level `No such module` class); every face stays absent
    // (undefined = honest face per the no-fabrication ruling).
    js_stream: {},
    contextify: {},
  };

  function internalBinding(name) {
    if (Object.prototype.hasOwnProperty.call(knownBindings, name)) {
      return knownBindings[name];
    }
    // Oracle-verbatim (node v24.19.0 getInternalBinding): unknown binding
    // names throw ERR_INVALID_MODULE — e.g. 'debug' does not exist even on
    // release node (the two buffer-alloc tests consuming it are
    // debug-build-gated and are skipped there via common.isDebug).
    const error = new Error("No such binding: " + name);
    error.code = "ERR_INVALID_MODULE";
    throw error;
  }

  return { internalBinding: internalBinding };
})()
"#;

/// Empty at realm-init time (mirrors node_internal_event_target::install):
/// install-time evaluation poisons the realm — the GC-pressure class where
/// the init-period function-object allocations push the nursery allocation
/// order over a latent engine cliff, and any later module-body variable
/// access then segfaults (timeline-attributed 2026-10-26: HEAD/B5/B6 series).
/// The real evaluation happens lazily on the first
/// `require('internal/test/binding')` via [`ensure_installed`], in user
/// context, when realm init has fully completed.
pub fn install(_cx: &mut mozjs::context::JSContext) {}

thread_local! {
    /// Per-realm once flag: repeat failed evaluations must not re-run the
    /// whole source on every require attempt (event_target precedent).
    static NITB_EVAL_DONE: ::std::cell::Cell<bool> = ::std::cell::Cell::new(false);
}

/// Evaluate the `internal/test/binding` port and cache the module object for
/// the require() surface. Idempotent (gc_store cache is the idempotence
/// boundary). Called from `require_fn` (a JSNative frame): `cx` is live and
/// the caller propagates a pending exception when evaluation fails.
pub fn ensure_installed(cx: *mut JSContext) -> bool {
    NITB_EVAL_DONE.with(|flag| {
        if flag.get() {
            return;
        }
        flag.set(true);
        install_eval(cx);
    });
    crate::require::get_builtin(cx, "internal/test/binding").is_some()
}

fn install_eval(raw_cx: *mut JSContext) {
    let source = INTERNAL_TEST_BINDING_JS;
    unsafe {
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let opts = NewCompileOptions(raw_cx, c"<internal/test/binding>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            mozjs::glue::DeleteCompileOptions(opts);
            if !ok {
                JS_ReportErrorUTF8(
                    raw_cx,
                    c"failed to evaluate internal/test/binding source".as_ptr(),
                );
                return;
            }
            if !rval.is_object() {
                return;
            }
            let obj = rval.to_object();
            let mut wrapped =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
            rooted!(&in(wrapped) let obj_rooted = obj);
            // Mirror Node's behavior: require('internal/test/binding')
            // throws if the module cannot be loaded or is missing — here
            // we only cache it when evaluation produced a real object.
            cache_builtin(&mut wrapped, "internal/test/binding", obj_rooted.get());
        }
    }
}
