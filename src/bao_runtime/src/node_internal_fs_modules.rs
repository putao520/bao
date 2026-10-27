// @trace REQ-ENG-004 [api:internal/fs/utils|internal/fs/watchers|internal/fs/recursive_watch|internal/fs/glob]
//
// GOLDEN A fs 域 MODULE_MISSING_internal_or_other + FS_WATCH_recursive_unsupported
// 两族的根治面:node v24 lib/internal/fs/{utils,watchers,recursive_watch}.js
// 的 JS 真源翻译(测试语料直接 require 这些内部模块,并按其内部契约断言;
// Linux 上 node 的 fs.watch({recursive:true}) 本身就路由到 recursive_watch
// 的 JS 层 —— bao 复用同一架构,bao 的 fs.watch(非递归 notify 原语)作为
// 递归层的每目录基元)。
//
// Consumers / wiring:
// - require.rs `known_builtins` 新增 4 个 key(utils/watchers/recursive_watch/
//   promises),每 key 的 require 挂钩触发本文件 ensure_installed(幂等)。
// - node_fs.rs fs_watch 的 recursive+linux 分支经 gc_store "fsinternal" /
//   "watchRecursive" 调 recursive_watch.watchRecursive(node lib/fs.js 的
//   recursive 分派形状)。
// - 翻译约定(与 node_internal_event_target 相同):primordials → realm
//   globals;SafeMap/SafeSet → Map/Set;#private → Symbol 槽;internalBinding
//   依赖(SQLite 面的 uv/fs binding)不可达处以等义 JS 实现替代;逐处偏离在
//   行内标注。上游同步重放锚:三个 source 字符串。

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use ::std::ptr::NonNull;

use bun_core::ZBox;

use crate::require::{cache_builtin, get_builtin};

/// Lazy-evaluated internal fs module sources, evaluated IN ORDER (utils →
/// watchers → recursive_watch → promises): each later module require()s the
/// earlier ones, and every evaluation result is cached under its builtin key
/// so the require hook returns the same object on every require.
pub fn ensure_installed(cx: *mut JSContext) -> bool {
    // Re-entrancy guard: the module sources require() each other during
    // evaluation, and every internal/fs/* require re-enters this hook. The
    // in-flight evaluation has already cached the earlier modules, so a
    // nested call must no-op — otherwise recursive_watch evaluates twice and
    // the stashed watchRecursive/withAsyncIterator identities diverge from
    // the module instance require() hands out.
    INSTALLING.with(|flag| {
        if *flag.borrow() {
            return true;
        }
        *flag.borrow_mut() = true;
        // The flag resets on EVERY path (failed evaluation included) so a
        // later require retries the install instead of silently no-op'ing.
        let result = ensure_installed_inner(cx);
        *flag.borrow_mut() = false;
        result
    })
}

thread_local! {
    static INSTALLING: ::std::cell::RefCell<bool> = const { ::std::cell::RefCell::new(false) };
}

fn ensure_installed_inner(cx: *mut JSContext) -> bool {
    // Install the native glob binding BEFORE FS_GLOB_JS evaluates: the JS
    // Glob class captures it from the temporary global (the
    // install_fs_stream_polyfill global-capture precedent); the global is
    // deleted after the family evaluates so no extra API surface remains.
    // SAFETY: cx is the calling JSNative frame's context (require hook).
    if !unsafe { crate::node_fs::glob_native_binding_install(cx) } {
        return false;
    }
    for (key, source, filename) in [
        (
            "internal/fs/utils",
            FS_UTILS_JS,
            "<internal/fs/utils>",
        ),
        (
            "internal/fs/watchers",
            FS_WATCHERS_JS,
            "<internal/fs/watchers>",
        ),
        (
            "internal/fs/recursive_watch",
            FS_RECURSIVE_WATCH_JS,
            "<internal/fs/recursive_watch>",
        ),
        (
            "internal/fs/promises",
            FS_PROMISES_JS,
            "<internal/fs/promises>",
        ),
        (
            "internal/fs/glob",
            FS_GLOB_JS,
            "<internal/fs/glob>",
        ),
    ] {
        // cache_builtin/get_builtin add the "builtin:" prefix themselves.
        if get_builtin(cx, key).is_some() {
            continue;
        }
        if !evaluate_and_cache(cx, key, source, filename) {
            // The engine exception stays pending; the require caller
            // propagates it verbatim (Node parity).
            return false;
        }
    }
    // Delete the bridge global — the FS_GLOB_JS closure captured the binding
    // object, so the native face stays reachable only through the module.
    {
        let global = unsafe { JS::CurrentGlobalOrNull(cx) };
        if !global.is_null() {
            unsafe {
                let wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                rooted!(&in(wrapped) let global_root = global);
                JS_DeleteProperty1(
                    cx,
                    global_root.handle().into(),
                    c"__bao_internal_glob".as_ptr(),
                );
            }
        }
    }
    // Stash the promises-watch iterator helper for node_fs.rs
    // fs_promises_watch (same gc_store channel as watchRecursive).
    if let Some(watchers_mod) = get_builtin(cx, "internal/fs/watchers") {
        let mut wrapped = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
        let mut fval = UndefinedValue();
        unsafe {
            rooted!(&in(wrapped) let mod_r = watchers_mod);
            JS_GetProperty(
                cx,
                mod_r.handle().into(),
                c"withAsyncIterator".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut fval,
                },
            );
            if fval.is_object() {
                crate::gc_store::gc_store_insert_ns(cx, "fsinternal", "withAsyncIterator", fval.to_object());
            }
        }
    }
    get_builtin(cx, "internal/fs/glob").is_some()
}

/// Evaluate one module source and cache its exports under `builtin:<key>`.
/// GC contract: called from a JSNative frame (require hook / fs_watch);
/// pending exceptions propagate to the caller.
fn evaluate_and_cache(cx: *mut JSContext, key: &str, source: &str, filename: &str) -> bool {
    unsafe {
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let fname = ZBox::from_bytes(filename.as_bytes());
        let opts = NewCompileOptions(cx, fname.as_ptr(), 1);
        if opts.is_null() {
            JS_ClearPendingException(cx);
            return false;
        }
        let ok = JS::Evaluate2(cx, opts, &mut source_text, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok || !rval.is_object() {
            if ok {
                JS_ClearPendingException(cx);
            }
            return false;
        }
        let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let obj = rval.to_object();
        rooted!(&in(wrapped) let obj_rooted = obj);
        cache_builtin(&mut wrapped, key, obj_rooted.get());
        true
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// internal/fs/utils — node v24 lib/internal/fs/utils.js subset. The exact
// export set is what the node fs test corpus destructures:
//   stringToFlags (test-fs-open-flags: full switch + ERR_INVALID_ARG_VALUE
//                  TypeError tail — oracle-verified),
//   validateOffsetLengthRead/Write (test-fs-util-validateoffsetlength:
//                  ERR_OUT_OF_RANGE RangeError, message
//                  `The value of "offset" is out of range. It must be >= 0.
//                  Received -1` — oracle-verified wording),
//   validateRmdirOptions / validateRmOptionsSync (test-fs-rmdir-recursive /
//                  test-fs-rm),
//   realpathCacheKey (test-fs-realpath-stale-stat-values: Symbol),
//   BigIntStats (test-fs-watchfile-bigint: 14-bigint-arg ctor + StatsBase
//                  prototype isDirectory/isFile/isSymbolicLink/... — source
//                  port of the utils.js StatsBase/BigIntStats pair),
//   getDirents/getDirent + Dirent (test-fs-utils-get-dirents),
//   getValidatedPath (recursive_watch dependency),
//   defaultRmdirOptions / defaultRmOptions.
// Deviations from upstream (all inline-marked): internalBinding('constants')
// → literal Linux constants; hideStackFrames → identity; once() → inline.
// ═══════════════════════════════════════════════════════════════════════════
const FS_UTILS_JS: &str = r#"
(function() {
  'use strict';

  // internalBinding('constants') faces used here — literal Linux values
  // (deviation: no uv binding; the fs.constants table mirrors node's).
  const S_IFMT = 0xF000, S_IFDIR = 0x4000, S_IFREG = 0x8000, S_IFLNK = 0xA000,
        S_IFCHR = 0x2000, S_IFIFO = 0x1000, S_IFBLK = 0x6000, S_IFSOCK = 0xC000;
  const O_RDONLY = 0, O_WRONLY = 1, O_RDWR = 2, O_CREAT = 0x40, O_EXCL = 0x80,
        O_TRUNC = 0x200, O_APPEND = 0x400, O_SYNC = 0x101000;
  const UV_DIRENT_UNKNOWN = 0, UV_DIRENT_FILE = 1, UV_DIRENT_DIR = 2,
        UV_DIRENT_LINK = 3, UV_DIRENT_FIFO = 4, UV_DIRENT_SOCKET = 5,
        UV_DIRENT_CHAR = 6, UV_DIRENT_BLOCK = 7;

  function lazyFs() { return require('fs'); }

  // ── error factories (node internal/errors message shapes) ───────────────
  function invalidArgType(name, expected, value) {
    const err = new TypeError(
      `The "${name}" argument must be of type ${expected}. ${received(value)}`);
    err.code = 'ERR_INVALID_ARG_TYPE';
    return err;
  }
  function received(value) {
    if (value === null) return 'Received null';
    const t = typeof value;
    if (t === 'undefined') return 'Received undefined';
    if (t === 'number' || t === 'boolean' || t === 'bigint') return `Received type ${t} (${value})`;
    if (t === 'string') return `Received type string (${JSON.stringify(value)})`;
    if (t === 'function') return `Received function ${value.name}`;
    if (Array.isArray(value)) return 'Received an instance of Array';
    return `Received an instance of ${value.constructor ? value.constructor.name : 'Object'}`;
  }
  function invalidArgValue(name, value, reason) {
    const err = new TypeError(
      reason !== undefined
        ? `The "${name}" argument is invalid. ${reason}. ${received(value)}`
        : `The "${name}" argument is invalid. ${received(value)}`);
    err.code = 'ERR_INVALID_ARG_VALUE';
    return err;
  }
  function outOfRange(name, range, value) {
    const err = new RangeError(
      `The value of "${name}" is out of range. It must be ${range}. Received ${value}`);
    err.code = 'ERR_OUT_OF_RANGE';
    return err;
  }
  function invalidFlagValue(flags) {
    const err = new TypeError(`Invalid flag: ${flags}`);
    err.code = 'ERR_INVALID_FLAG';
    return err;
  }

  // ── validators (internal/validators subset) ─────────────────────────────
  function validateBoolean(value, name) {
    if (typeof value !== 'boolean')
      throw invalidArgType(name, 'boolean', value);
  }
  function validateObject(value, name) {
    if (value === null || Array.isArray(value) || typeof value !== 'object')
      throw invalidArgType(name, 'Object', value);
  }
  function validateInt32(value, name, min, max) {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < -2147483648 || value > 2147483647)
      throw invalidArgType(name, 'int32', value);
    if ((min !== undefined && value < min) || (max !== undefined && value > max))
      throw outOfRange(name, min !== undefined && max !== undefined ? `>= ${min} && <= ${max}` : (min !== undefined ? `>= ${min}` : `<= ${max}`), value);
  }
  function validateUint32(value, name, min) {
    if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 4294967295)
      throw invalidArgType(name, 'uint32', value);
    if (min !== undefined && value < min)
      throw outOfRange(name, `>= ${min}`, value);
  }

  // getValidatedPath — string | Buffer | URL(file:) → string.
  function getValidatedPath(filename, name = 'path') {
    if (typeof filename === 'string') return filename;
    if (typeof filename === 'object' && filename !== null) {
      if (typeof filename.href === 'string') {
        // URL (file: scheme only — node's fileURLToPath validation).
        const href = filename.href;
        if (!href.startsWith('file:')) {
          const err = new TypeError('The URL must be of scheme file');
          err.code = 'ERR_INVALID_URL_SCHEME';
          throw err;
        }
        return decodeURIComponent(href.replace(/^file:\/\//, '').replace(/^[^/]*/, ''));
      }
      if (typeof filename.length === 'number') {
        // Buffer/TypedArray path bytes.
        return Buffer.from(filename).toString('utf8');
      }
    }
    throw invalidArgType(name, 'string or an instance of Buffer or URL', filename);
  }

  // ── stringToFlags (oracle-verified switch; number/null passthroughs) ────
  function stringToFlags(flags, name = 'flags') {
    if (typeof flags === 'number') {
      validateInt32(flags, name);
      return flags + 0;
    }
    if (flags == null) {
      return O_RDONLY;
    }
    switch (flags) {
      case 'r' : return O_RDONLY;
      case 'rs' : // Fall through.
      case 'sr' : return O_RDONLY | O_SYNC;
      case 'r+' : return O_RDWR;
      case 'rs+' : // Fall through.
      case 'sr+' : return O_RDWR | O_SYNC;

      case 'w' : return O_TRUNC | O_CREAT | O_WRONLY;
      case 'wx' : // Fall through.
      case 'xw' : return O_TRUNC | O_CREAT | O_WRONLY | O_EXCL;

      case 'w+' : return O_TRUNC | O_CREAT | O_RDWR;
      case 'wx+': // Fall through.
      case 'xw+': return O_TRUNC | O_CREAT | O_RDWR | O_EXCL;

      case 'a' : return O_APPEND | O_CREAT | O_WRONLY;
      case 'ax' : // Fall through.
      case 'xa' : return O_APPEND | O_CREAT | O_WRONLY | O_EXCL;
      case 'as' : // Fall through.
      case 'sa' : return O_APPEND | O_CREAT | O_WRONLY | O_SYNC;

      case 'a+' : return O_APPEND | O_CREAT | O_RDWR;
      case 'ax+': // Fall through.
      case 'xa+': return O_APPEND | O_CREAT | O_RDWR | O_EXCL;
      case 'as+': // Fall through.
      case 'sa+': return O_APPEND | O_CREAT | O_RDWR | O_SYNC;
    }
    throw invalidArgValue('flags', flags);
  }

  // ── read/write offset/length validation (oracle message shapes) ─────────
  function validateOffsetLengthRead(offset, length, bufferLength) {
    if (offset < 0) throw outOfRange('offset', '>= 0', offset);
    if (length < 0) throw outOfRange('length', '>= 0', length);
    if (offset + length > bufferLength)
      throw outOfRange('length', `<= ${bufferLength - offset}`, length);
  }
  function validateOffsetLengthWrite(offset, length, bufferLength) {
    if (offset < 0) throw outOfRange('offset', '>= 0', offset);
    if (length < 0) throw outOfRange('length', '>= 0', length);
    if (offset + length > bufferLength)
      throw outOfRange('length', `<= ${bufferLength - offset}`, length);
  }

  // ── rmdir/rm option validation ──────────────────────────────────────────
  const defaultRmdirOptions = { retryDelay: 0, maxRetries: 0, recursive: false };
  const defaultRmOptions = { retryDelay: 0, maxRetries: 0, recursive: false, force: false };
  function validateRmdirOptions(options, defaults = defaultRmdirOptions) {
    if (options === undefined)
      return defaults;
    validateObject(options, 'options');
    options = { ...defaults, ...options };
    validateBoolean(options.recursive, 'options.recursive');
    validateInt32(options.retryDelay, 'options.retryDelay', 0);
    validateUint32(options.maxRetries, 'options.maxRetries');
    return options;
  }
  function validateRmOptionsSync(path, options, expectDir) {
    options = validateRmdirOptions(options, defaultRmOptions);
    validateBoolean(options.force, 'options.force');
    if (!options.force || expectDir || !options.recursive) {
      let isDirectory;
      try {
        isDirectory = lazyFs().lstatSync(path, { throwIfNoEntry: !options.force })?.isDirectory();
      } catch (err) {
        if (options.force) { isDirectory = undefined; } else { throw err; }
      }
      if (expectDir && !isDirectory) {
        return false;
      }
      if (isDirectory && !options.recursive) {
        const err = new Error('is a directory');
        err.code = 'ERR_FS_EISDIR';
        err.path = path;
        err.syscall = 'rm';
        err.errno = 21;
        throw err;
      }
    }
    return options;
  }

  // ── realpath cache key (Symbol identity contract) ───────────────────────
  const realpathCacheKey = Symbol('realpath.cacheKey');

  // ── StatsBase / BigIntStats (source port of utils.js) ───────────────────
  function StatsBase(dev, mode, nlink, uid, gid, rdev, blksize,
                     ino, size, blocks) {
    this.dev = dev;
    this.mode = mode;
    this.nlink = nlink;
    this.uid = uid;
    this.gid = gid;
    this.rdev = rdev;
    this.blksize = blksize;
    this.ino = ino;
    this.size = size;
    this.blocks = blocks;
  }
  StatsBase.prototype._checkModeProperty = function(property) {
    return (this.mode & S_IFMT) === property;
  };
  StatsBase.prototype.isDirectory = function() { return this._checkModeProperty(S_IFDIR); };
  StatsBase.prototype.isFile = function() { return this._checkModeProperty(S_IFREG); };
  StatsBase.prototype.isSymbolicLink = function() { return this._checkModeProperty(S_IFLNK); };
  StatsBase.prototype.isCharacterDevice = function() { return this._checkModeProperty(S_IFCHR); };
  StatsBase.prototype.isFIFO = function() { return this._checkModeProperty(S_IFIFO); };
  StatsBase.prototype.isBlockDevice = function() { return this._checkModeProperty(S_IFBLK); };
  StatsBase.prototype.isSocket = function() { return this._checkModeProperty(S_IFSOCK); };

  const kNsPerMsBigInt = 1000000n;
  function dateFromMs(ms) {
    return new Date(Math.round(Number(ms)));
  }
  function setOwnProperty(obj, key, value) {
    Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
  }
  function lazyDateGetter(key, msKey) {
    return {
      enumerable: true,
      configurable: true,
      get() { return setOwnProperty(this, key, dateFromMs(this[msKey])); },
      set(value) { setOwnProperty(this, key, value); },
    };
  }

  function BigIntStats(dev, mode, nlink, uid, gid, rdev, blksize,
                       ino, size, blocks,
                       atimeNs, mtimeNs, ctimeNs, birthtimeNs) {
    StatsBase.call(this, dev, mode, nlink, uid, gid, rdev, blksize, ino, size, blocks);
    this.atimeMs = atimeNs / kNsPerMsBigInt;
    this.mtimeMs = mtimeNs / kNsPerMsBigInt;
    this.ctimeMs = ctimeNs / kNsPerMsBigInt;
    this.birthtimeMs = birthtimeNs / kNsPerMsBigInt;
    this.atimeNs = atimeNs;
    this.mtimeNs = mtimeNs;
    this.ctimeNs = ctimeNs;
    this.birthtimeNs = birthtimeNs;
  }
  Object.setPrototypeOf(BigIntStats.prototype, StatsBase.prototype);
  Object.setPrototypeOf(BigIntStats, StatsBase);
  Object.defineProperties(BigIntStats.prototype, {
    atime: lazyDateGetter('atime', 'atimeMs'),
    mtime: lazyDateGetter('mtime', 'mtimeMs'),
    ctime: lazyDateGetter('ctime', 'ctimeMs'),
    birthtime: lazyDateGetter('birthtime', 'birthtimeMs'),
  });
  BigIntStats.prototype._checkModeProperty = function(property) {
    return (this.mode & BigInt(S_IFMT)) === BigInt(property);
  };

  // ── Dirent + getDirents/getDirent (binding out-param shape preserved:
  //    names/types are parallel arrays; UV_DIRENT_UNKNOWN entries lstat) ────
  // parentPath extension (node v20.1+ Dirent shape, v24 canonical): the
  // fs.glob withFileTypes face instances `new Dirent(name, type, parentPath)`
  // (test-fs-glob normalizeDirent reads dirent.parentPath + dirent.name);
  // existing 2-arg callers keep working with parentPath undefined.
  const kDirentType = Symbol('dirent.type');
  function Dirent(name, type, parentPath) {
    this.name = name;
    this[kDirentType] = type;
    this.parentPath = parentPath;
    this.path = parentPath;
  }
  Dirent.prototype.isDirectory = function() { return this[kDirentType] === UV_DIRENT_DIR; };
  Dirent.prototype.isFile = function() { return this[kDirentType] === UV_DIRENT_FILE; };
  Dirent.prototype.isSymbolicLink = function() { return this[kDirentType] === UV_DIRENT_LINK; };
  Dirent.prototype.isFIFO = function() { return this[kDirentType] === UV_DIRENT_FIFO; };
  Dirent.prototype.isSocket = function() { return this[kDirentType] === UV_DIRENT_SOCKET; };
  Dirent.prototype.isCharacterDevice = function() { return this[kDirentType] === UV_DIRENT_CHAR; };
  Dirent.prototype.isBlockDevice = function() { return this[kDirentType] === UV_DIRENT_BLOCK; };

  function pathJoin(a, b) { return require('path').join(a, b); }
  function typeToDirent(name, type) { return new Dirent(name, type); }

  function getDirents(path, { 0: names, 1: types }, callback) {
    let i;
    if (typeof callback === 'function') {
      const len = names.length;
      let toFinish = 0;
      let called = false;
      const once = (fn) => (...args) => {
        if (called) return;
        called = true;
        fn(...args);
      };
      callback = once(callback);
      const results = new Array(len);
      const finishOne = (idx, dirent) => {
        results[idx] = dirent;
        toFinish--;
        if (toFinish === 0) {
          callback(null, results);
        }
      };
      for (i = 0; i < len; i++) {
        const type = types[i];
        if (type === UV_DIRENT_UNKNOWN) {
          const name = names[i];
          const idx = i;
          toFinish++;
          let filepath;
          try {
            filepath = pathJoin(path, name);
          } catch (err) {
            callback(err);
            return;
          }
          lazyFs().lstat(filepath, (err, stats) => {
            if (err) { callback(err); return; }
            finishOne(idx, stats);
          });
        } else {
          results[i] = typeToDirent(names[i], type);
        }
      }
      // Synchronous-only known-type lists resolve immediately.
      if (toFinish === 0) {
        callback(null, results);
      }
      return;
    }
    // Sync form (no callback): resolve all synchronously, lstat'ing unknowns.
    const len = names.length;
    const results = new Array(len);
    for (i = 0; i < len; i++) {
      const type = types[i];
      if (type === UV_DIRENT_UNKNOWN) {
        let stats;
        try {
          stats = lazyFs().lstatSync(pathJoin(path, names[i]), { throwIfNoEntry: false });
        } catch {
          stats = undefined;
        }
        results[i] = stats;
      } else {
        results[i] = typeToDirent(names[i], type);
      }
    }
    return results;
  }

  function getDirent(path, name, type, callback) {
    if (typeof callback === 'function') {
      if (type === UV_DIRENT_UNKNOWN) {
        lazyFs().lstat(pathJoin(path, name), (err, stats) => {
          if (err) { callback(err); return; }
          callback(null, stats);
        });
        return;
      }
      callback(null, typeToDirent(name, type));
      return;
    }
    if (type === UV_DIRENT_UNKNOWN) {
      try {
        return lazyFs().lstatSync(pathJoin(path, name), { throwIfNoEntry: false });
      } catch {
        return undefined;
      }
    }
    return typeToDirent(name, type);
  }

  return {
    constants: {
      UV_DIRENT_UNKNOWN, UV_DIRENT_FILE, UV_DIRENT_DIR, UV_DIRENT_LINK,
      UV_DIRENT_FIFO, UV_DIRENT_SOCKET, UV_DIRENT_CHAR, UV_DIRENT_BLOCK,
    },
    stringToFlags,
    validateOffsetLengthRead,
    validateOffsetLengthWrite,
    validateRmdirOptions,
    validateRmOptionsSync,
    validateBoolean,
    validateObject,
    validateInt32,
    validateUint32,
    getValidatedPath,
    realpathCacheKey,
    StatsBase,
    BigIntStats,
    Dirent,
    getDirents,
    getDirent,
    defaultRmdirOptions,
    defaultRmOptions,
  };
})();
"#;

// ═══════════════════════════════════════════════════════════════════════════
// internal/fs/watchers — node v24 lib/internal/fs/watchers.js subset. The
// node fs corpus imports { kFSWatchStart, createIgnoreMatcher, StatWatcher,
// FSWatcher }. Upstream binds FSEvent/StatWatcher via internalBinding — here
// the wraps route through bao's fs.watch/fs.watchFile primitives (same event
// contract: FSWatcher emits change(eventType, filename) + close; StatWatcher
// emits change(curr, prev) + stop). #private → Symbol slots (deviation, inline).
// ═══════════════════════════════════════════════════════════════════════════
const FS_WATCHERS_JS: &str = r#"
(function() {
  'use strict';

  const events_mod = require('events');
  // node shape: module value === .EventEmitter === the function — accept both.
  const EventEmitter = events_mod.EventEmitter || events_mod;
  const { getValidatedPath } = require('internal/fs/utils');

  const kFSWatchStart = Symbol('kFSWatchStart');
  const kFSStatWatcherStart = Symbol('kFSStatWatcherStart');
  const KFSStatWatcherRefCount = Symbol('KFSStatWatcherRefCount');
  const KFSStatWatcherMaxRefCount = Symbol('KFSStatWatcherMaxRefCount');
  const kFSStatWatcherAddOrCleanRef = Symbol('kFSStatWatcherAddOrCleanRef');
  const kHandle = Symbol('kHandle');
  const kOpts = Symbol('kOptions');

  function lazyFs() { return require('fs'); }

  function isRegExp(value) { return value instanceof RegExp; }

  // createIgnoreMatcher — string patterns route through a minimal glob
  // translator (deviation: upstream uses internal minimatch; the supported
  // subset is * / ** / ? / literal, matchBase semantics for slash-less
  // patterns — the shapes the fs watch-ignore tests drive); RegExp and
  // Function matchers pass through verbatim.
  function globToRegExp(pattern) {
    let re = '';
    for (let i = 0; i < pattern.length; i++) {
      const c = pattern[i];
      if (c === '*') {
        if (pattern[i + 1] === '*') {
          while (pattern[i + 1] === '*') i++;
          if (pattern[i + 1] === '/') { re += '(?:[^/]*/)*'; i++; }
          else re += '.*';
        } else {
          re += '[^/]*';
        }
      } else if (c === '?') {
        re += '[^/]';
      } else if ('\\^$.|+()[]{}'.includes(c)) {
        re += '\\' + c;
      } else {
        re += c;
      }
    }
    return new RegExp('^' + re + '$');
  }

  function createIgnoreMatcher(ignore) {
    if (ignore == null) return null;
    const matchers = Array.isArray(ignore) ? ignore : [ignore];
    const compiled = [];
    for (let i = 0; i < matchers.length; i++) {
      const matcher = matchers[i];
      if (typeof matcher === 'string') {
        const re = matcher.includes('/')
          ? globToRegExp(matcher)
          // matchBase: slash-less patterns match the basename.
          : new RegExp('(^|/)(' + globToRegExp(matcher).source.replace(/^\^/, '') + ')$');
        compiled.push((filename) => re.test(filename));
      } else if (isRegExp(matcher)) {
        compiled.push((filename) => matcher.test(filename));
      } else if (typeof matcher === 'function') {
        compiled.push(matcher);
      } else {
        const err = new TypeError(
          'The "options.ignore" argument must be of type string, RegExp, Function, or Array. Received an instance of Object');
        err.code = 'ERR_INVALID_ARG_TYPE';
        throw err;
      }
    }
    return (filename) => {
      for (let i = 0; i < compiled.length; i++) {
        if (compiled[i](filename)) return true;
      }
      return false;
    };
  }

  function emitStop(self) {
    self.emit('stop');
  }

  // ── StatWatcher (fs.watchFile face) ─────────────────────────────────────
  function StatWatcher(bigint) {
    EventEmitter.call(this);
    this[kHandle] = null;
    this[KFSStatWatcherRefCount] = 1;
    this[KFSStatWatcherMaxRefCount] = 1;
    this._bigint = bigint === true;
  }
  Object.setPrototypeOf(StatWatcher.prototype, EventEmitter.prototype);
  Object.setPrototypeOf(StatWatcher, EventEmitter);

  StatWatcher.prototype[kFSStatWatcherStart] = function(filename, persistent, interval) {
    if (this[kHandle] !== null)
      return;
    filename = getValidatedPath(filename, 'filename');
    const fs = lazyFs();
    const onChange = (curr, prev) => {
      this.emit('change', curr, prev);
    };
    this[kHandle] = fs.watchFile(filename, {
      persistent,
      interval: typeof interval === 'number' ? interval : 5007,
      bigint: this._bigint,
    }, onChange);
    // bao's watchFile returns its StatWatcher object (ref/unref face).
  };

  // Backwards-compat no-op stub (node parity: start() is not public API).
  StatWatcher.prototype.start = () => {};

  StatWatcher.prototype.stop = function() {
    if (this[kHandle] === null)
      return;
    const handle = this[kHandle];
    this[kHandle] = null;
    if (handle && typeof handle.close === 'function') {
      try { handle.close(); } catch { /* already closed */ }
    }
    process.nextTick(emitStop, this);
  };

  StatWatcher.prototype[kFSStatWatcherAddOrCleanRef] = function(operate) {
    if (operate === 'add') {
      this[KFSStatWatcherRefCount]++;
      this[KFSStatWatcherMaxRefCount]++;
    } else if (operate === 'clean') {
      this[KFSStatWatcherMaxRefCount]--;
      this.unref();
    } else if (operate === 'cleanAll') {
      this[KFSStatWatcherMaxRefCount] = 0;
      this[KFSStatWatcherRefCount] = 0;
      if (this[kHandle] && typeof this[kHandle].unref === 'function') this[kHandle].unref();
    }
  };

  StatWatcher.prototype.ref = function() {
    if (this[KFSStatWatcherRefCount] === this[KFSStatWatcherMaxRefCount])
      return this;
    if (this[kHandle] && this[KFSStatWatcherRefCount]++ === 0 && typeof this[kHandle].ref === 'function')
      this[kHandle].ref();
    return this;
  };

  StatWatcher.prototype.unref = function() {
    if (this[KFSStatWatcherRefCount] === 0) return this;
    if (this[kHandle] && --this[KFSStatWatcherRefCount] === 0 && typeof this[kHandle].unref === 'function')
      this[kHandle].unref();
    return this;
  };

  // ── FSWatcher (fs.watch primitive face — bao's notify-backed watcher) ───
  function FSWatcher() {
    EventEmitter.call(this);
    this[kHandle] = null;
    this._ignoreMatcher = null;
    this[kOpts] = { persistent: true, recursive: false, encoding: 'utf8' };
  }
  Object.setPrototypeOf(FSWatcher.prototype, EventEmitter.prototype);
  Object.setPrototypeOf(FSWatcher, EventEmitter);

  FSWatcher.prototype[kFSWatchStart] = function(filename,
                                                persistent,
                                                recursive,
                                                encoding,
                                                ignore,
                                                throwIfNoEntry = true) {
    if (this[kHandle] !== null) {  // closed
      return;
    }
    filename = getValidatedPath(filename, 'filename');
    this._ignoreMatcher = createIgnoreMatcher(ignore);
    this[kOpts] = {
      persistent: persistent !== false,
      recursive: recursive === true,
      encoding: typeof encoding === 'string' ? encoding : 'utf8',
    };
    const fs = lazyFs();
    this[kHandle] = fs.watch(filename, this[kOpts]);
    // Chain bao's watcher events onto this instance (ignore filter applied
    // like upstream's handle.onchange).
    const handle = this[kHandle];
    if (handle && typeof handle.on === 'function') {
      handle.on('change', (eventType, fname) => {
        if (fname != null && this._ignoreMatcher && this._ignoreMatcher(fname)) {
          return;
        }
        this.emit('change', eventType, fname);
      });
      handle.on('close', () => {
        this[kHandle] = null;
        this.emit('close');
      });
      handle.on('error', (err) => this.emit('error', err));
    }
  };

  // Backwards-compat no-op stub (node parity).
  FSWatcher.prototype.start = () => {};

  FSWatcher.prototype.close = function() {
    if (this[kHandle] === null) {  // closed
      return;
    }
    const handle = this[kHandle];
    this[kHandle] = null;
    if (handle && typeof handle.close === 'function') {
      handle.close();
    }
    process.nextTick(() => this.emit('close'));
  };

  FSWatcher.prototype.ref = function() {
    if (this[kHandle] && typeof this[kHandle].ref === 'function') this[kHandle].ref();
    return this;
  };

  FSWatcher.prototype.unref = function() {
    if (this[kHandle] && typeof this[kHandle].unref === 'function') this[kHandle].unref();
    return this;
  };

  // fs.promises.watch iterator face for the native (non-recursive) watcher:
  // node's promises watch yields { eventType, filename } objects
  // (test-fs-promises-watch: `for await (const { eventType, filename } of
  // watcher)`); a watcher that already carries Symbol.asyncIterator (the JS
  // recursive layer) passes through untouched.
  function withAsyncIterator(watcher) {
    if (watcher == null || watcher[Symbol.asyncIterator]) {
      return watcher;
    }
    watcher[Symbol.asyncIterator] = function() {
      const self = this;
      return {
        next: () => new Promise((resolve) => {
          const onNext = (eventType, filename) => {
            cleanup();
            resolve({ value: { eventType, filename }, done: false });
          };
          const onClose = () => {
            cleanup();
            resolve({ done: true });
          };
          function cleanup() {
            self.removeListener('change', onNext);
            self.removeListener('close', onClose);
          }
          self.on('change', onNext);
          self.on('close', onClose);
        }),
        return: () => {
          self.close();
          return Promise.resolve({ done: true });
        },
        [Symbol.asyncIterator]() { return this; },
      };
    };
    return watcher;
  }

  return {
    createIgnoreMatcher,
    FSWatcher,
    StatWatcher,
    kFSWatchStart,
    kFSStatWatcherStart,
    kFSStatWatcherAddOrCleanRef,
    withAsyncIterator,
  };
})();
"#;

// ═══════════════════════════════════════════════════════════════════════════
// internal/fs/recursive_watch — node v24 lib/internal/fs/recursive_watch.js
// faithful port (the Linux recursive fs.watch implementation node itself
// uses: one non-recursive fs.watch per directory + entry bookkeeping + new-
// dir rescans; events carry paths relative to the watched root). #private →
// Symbol slots; primordials → globals; AbortError/ERR_INVALID_ARG_VALUE →
// inline factories. watchRecursive() = node lib/fs.js recursive dispatch arm
// (options normalization + kFSWatchStart) — stashed by ensure_installed for
// node_fs.rs fs_watch.
// ═══════════════════════════════════════════════════════════════════════════
const FS_RECURSIVE_WATCH_JS: &str = r#"
(function() {
  'use strict';

  const { EventEmitter } = require('events');
  const { getValidatedPath, validateBoolean } = require('internal/fs/utils');
  const { createIgnoreMatcher, kFSWatchStart } = require('internal/fs/watchers');
  const path = require('path');

  const kOpts = Symbol('kOptions');
  const kClosed = Symbol('kClosed');
  const kEntries = Symbol('kEntries');
  const kWatchers = Symbol('kWatchers');
  const kSymbolicLinks = Symbol('kSymbolicLinks');
  const kRootPath = Symbol('kRootPath');
  const kIgnoreMatcher = Symbol('kIgnoreMatcher');

  function lazyLoadFsSync() {
    return require('fs');
  }

  const kEmptyObject = Object.freeze({ __proto__: null });

  const kIsAIX = process.platform === 'aix';

  function isPathMissingForWatch(error, file) {
    if (error.code === 'ENOENT') {
      return true;
    }
    if (!kIsAIX || error.code !== 'ENODEV') {
      return false;
    }
    try {
      return lazyLoadFsSync().statSync(file, { throwIfNoEntry: false }) === undefined;
    } catch {
      return false;
    }
  }

  // Inotify reports changes to a directory's entries, with their names, on the
  // directory's own watch, so one watcher per directory is enough on Linux.
  const kDirectoryWatchReportsEntries = process.platform === 'linux';

  function validateAbortSignal(signal, name) {
    if (signal !== null && typeof signal !== 'object') {
      const err = new TypeError(`The "${name}" argument must be of type AbortSignal or null. Received ${String(signal)}`);
      err.code = 'ERR_INVALID_ARG_TYPE';
      throw err;
    }
  }

  // node v24.19.0 oracle (fs.watch / fs.promises.watch, both faces route the
  // recursive layer through here): non string/RegExp/function/Array →
  // ERR_INVALID_ARG_TYPE 'The "options.ignore" property must be one of type
  // string or function or an instance of RegExp. Received …'; empty string →
  // ERR_INVALID_ARG_VALUE "The property 'options.ignore' must be a non-empty
  // string. Received ''" (test-fs-promises-watch-ignore-invalid).
  function validateIgnoreOption(ignore, name) {
    if (ignore == null) return;
    const ok = (typeof ignore === 'string' && ignore !== '') || ignore instanceof RegExp ||
      typeof ignore === 'function' || Array.isArray(ignore);
    if (!ok) {
      let err;
      if (typeof ignore === 'string') {
        err = new TypeError(`The property '${name}' must be a non-empty string. Received '${ignore}'`);
        err.code = 'ERR_INVALID_ARG_VALUE';
      } else {
        err = new TypeError(`The "${name}" property must be one of type string or function or an instance of RegExp. Received ${describeReceived(ignore)}`);
        err.code = 'ERR_INVALID_ARG_TYPE';
      }
      throw err;
    }
  }
  function describeReceived(value) {
    if (value === null) return 'null';
    const t = typeof value;
    if (t === 'number') return `type number (${value})`;
    if (t === 'boolean') return `type boolean (${value})`;
    if (t === 'bigint') return `type bigint (${value}n)`;
    if (t === 'string') return `type string (${JSON.stringify(value)})`;
    if (t === 'function') return `function ${value.name}`;
    if (Array.isArray(value)) return 'an instance of Array';
    return 'an instance of Object';
  }

  class FSWatcher extends EventEmitter {
    constructor(options = kEmptyObject) {
      super();

      const { persistent, recursive, signal, encoding, ignore } = options;
      let { throwIfNoEntry } = options;

      if (recursive != null) {
        validateBoolean(recursive, 'options.recursive');
      }

      if (persistent != null) {
        validateBoolean(persistent, 'options.persistent');
      }

      if (signal != null) {
        validateAbortSignal(signal, 'options.signal');
      }

      if (throwIfNoEntry != null) {
        validateBoolean(throwIfNoEntry, 'options.throwIfNoEntry');
      } else {
        throwIfNoEntry = true;
      }

      if (encoding != null) {
        if (typeof encoding !== 'string') {
          const err = new TypeError('The "options.encoding" argument is invalid.');
          err.code = 'ERR_INVALID_ARG_VALUE';
          throw err;
        }
      }

      validateIgnoreOption(ignore, 'options.ignore');
      this[kIgnoreMatcher] = createIgnoreMatcher(ignore);

      this[kOpts] = { persistent, recursive, signal, encoding, throwIfNoEntry };
      this[kClosed] = false;
      this[kEntries] = new Set();
      this[kWatchers] = new Map();
      this[kSymbolicLinks] = new Set();
      this[kRootPath] = path.resolve();
    }

    close() {
      if (this[kClosed]) {
        return;
      }

      this[kClosed] = true;

      for (const watcher of this[kWatchers].values()) {
        watcher.close();
      }
      this[kWatchers].clear();
      this[kEntries].clear();
      this[kSymbolicLinks].clear();
      this.emit('close');
    }

    [Symbol.asyncIterator]() {
      const signal = this[kOpts].signal;
      const promiseExecutor = signal == null ?
        (resolve) => {
          this.once('change', (eventType, filename) => {
            resolve({ __proto__: null, value: { eventType, filename } });
          });
        } : (resolve, reject) => {
          const onAbort = () => {
            this.close();
            const err = new Error('The operation was aborted');
            err.code = 'ABORT_ERR';
            reject(err);
          };
          if (signal.aborted) return onAbort();
          signal.addEventListener('abort', onAbort, { __proto__: null, once: true });
          this.once('change', (eventType, filename) => {
            signal.removeEventListener('abort', onAbort);
            resolve({ __proto__: null, value: { eventType, filename } });
          });
        };
      return {
        next: () => (this[kClosed] ?
          { __proto__: null, done: true } :
          new Promise(promiseExecutor)),
        return: () => {
          this.close();
          return { __proto__: null, done: true };
        },
        [Symbol.asyncIterator]() { return this; },
      };
    }

    $kEmit(eventType, file) {
      this.emit('change', eventType, path.relative(this[kRootPath], file));
    }

    $kForget(file) {
      const watcher = this[kWatchers].get(file);
      if (watcher !== undefined) {
        watcher.close();
        this[kWatchers].delete(file);
      }
      this[kEntries].delete(file);
      this[kSymbolicLinks].delete(file);

      const childPrefix = file + path.sep;
      for (const entry of [...this[kEntries]]) {
        if (entry.startsWith(childPrefix)) {
          this[kEntries].delete(entry);
          this[kSymbolicLinks].delete(entry);
          const childWatcher = this[kWatchers].get(entry);
          if (childWatcher !== undefined) {
            childWatcher.close();
            this[kWatchers].delete(entry);
          }
        }
      }
    }

    $kWatch(file, onChange) {
      if (this[kClosed]) {
        return false;
      }
      if (this[kWatchers].has(file)) {
        return true;
      }
      const fs = lazyLoadFsSync();
      let watcher;
      try {
        watcher = fs.watch(file, { persistent: this[kOpts].persistent }, onChange);
      } catch (err) {
        if (isPathMissingForWatch(err, file) &&
            (file !== this[kRootPath] || !this[kOpts].throwIfNoEntry)) {
          return false;
        }
        throw err;
      }
      this[kWatchers].set(file, watcher);
      return true;
    }

    $kScanFolder(folder, initial) {
      if (!this.$kWatch(folder, (eventType, filename) => this.$kOnFolderEvent(folder, filename))) {
        this.$kForget(folder);
        return;
      }

      const fs = lazyLoadFsSync();
      let entries;
      try {
        entries = fs.readdirSync(folder, { withFileTypes: true });
      } catch (error) {
        if (error.code === 'ENOENT' || error.code === 'ENOTDIR') {
          if (folder === this[kRootPath] && initial &&
              (error.code !== 'ENOENT' || this[kOpts].throwIfNoEntry)) {
            throw error;
          }
          if (!initial && !this[kClosed]) {
            this.$kEmit('rename', folder);
          }
          this.$kForget(folder);
        } else {
          this.emit('error', error);
        }
        return;
      }

      for (const entry of entries) {
        if (this[kClosed]) {
          break;
        }
        const file = path.join(folder, entry.name);
        if (!this[kEntries].has(file) && !(this[kIgnoreMatcher] && this[kIgnoreMatcher](path.relative(this[kRootPath], file)))) {
          this.$kAddEntry(file, entry, initial);
        }
      }
    }

    $kAddEntry(file, entry, initial) {
      this[kEntries].add(file);
      if (!initial) {
        this.$kEmit('rename', file);
      }
      if (entry.isSymbolicLink()) {
        this[kSymbolicLinks].add(file);
        if (!this.$kWatch(file, () => this.$kEmit('rename', file))) {
          this.$kForget(file);
        }
      } else if (entry.isDirectory()) {
        this.$kScanFolder(file, initial);
      } else if (!kDirectoryWatchReportsEntries) {
        if (!this.$kWatch(file, () => this.$kOnEntryEvent(file))) {
          this.$kForget(file);
        }
      }
    }

    $kOnFolderEvent(folder, filename) {
      if (this[kClosed]) {
        return;
      }
      const fs = lazyLoadFsSync();
      if (!kDirectoryWatchReportsEntries || filename == null) {
        if (fs.statSync(folder, { throwIfNoEntry: false }) === undefined) {
          this.$kEmit('rename', folder);
          this.$kForget(folder);
        } else {
          this.$kScanFolder(folder, false);
        }
        return;
      }
      const file = path.join(folder, filename);

      if (!this[kEntries].has(file)) {
        if (this[kIgnoreMatcher] && this[kIgnoreMatcher](path.relative(this[kRootPath], file))) {
          return;
        }
        const entry = fs.lstatSync(file, { throwIfNoEntry: false });
        if (entry !== undefined) {
          this.$kAddEntry(file, entry, false);
        } else if (folder === this[kRootPath] && fs.statSync(folder, { throwIfNoEntry: false }) === undefined) {
          this.$kEmit('rename', folder);
          this.$kForget(folder);
        }
        return;
      }

      this.$kOnEntryEvent(file);
    }

    $kOnEntryEvent(file) {
      if (this[kClosed]) {
        return;
      }
      const fs = lazyLoadFsSync();
      const stats = fs.statSync(file, { throwIfNoEntry: false });
      if (stats === undefined) {
        this.$kEmit('rename', file);
        this.$kForget(file);
      } else if (this[kSymbolicLinks].has(file)) {
        this.$kEmit('rename', file);
      } else if (stats.isDirectory()) {
        this.$kScanFolder(file, false);
      } else {
        this.$kEmit('change', file);
      }
    }

    $kWatchRootFile(file) {
      const fs = lazyLoadFsSync();
      this[kEntries].add(file);
      if (!this.$kWatch(file, () => {
        if (this[kClosed]) {
          return;
        }
        if (fs.statSync(file, { throwIfNoEntry: false }) === undefined) {
          this.$kEmit('rename', file);
          this.$kForget(file);
        } else {
          this.emit('change', 'change', path.basename(file));
        }
      })) {
        this.$kForget(file);
      }
    }

    [kFSWatchStart](filename) {
      filename = path.resolve(getValidatedPath(filename));

      try {
        const file = lazyLoadFsSync().statSync(filename);

        this[kRootPath] = filename;
        this[kClosed] = false;

        if (file.isDirectory()) {
          this.$kScanFolder(filename, true);
        } else {
          this.$kWatchRootFile(filename);
        }
      } catch (error) {
        if (this[kOpts].throwIfNoEntry || error.code !== 'ENOENT') {
          error.filename = filename;
          this.close();
          throw error;
        }
      }
    }

    ref() {
      for (const watcher of this[kWatchers].values()) {
        if (typeof watcher.ref === 'function') watcher.ref();
      }
    }

    unref() {
      for (const watcher of this[kWatchers].values()) {
        if (typeof watcher.unref === 'function') watcher.unref();
      }
    }
  }

  // node lib/fs.js recursive dispatch arm: normalize options the way the
  // public fs.watch() face does, attach the listener, start the watcher.
  function watchRecursive(filename, options, listener) {
    if (typeof options === 'function') {
      listener = options;
      options = kEmptyObject;
    }
    if (typeof options === 'string') {
      options = { encoding: options };
    }
    if (options == null) {
      options = kEmptyObject;
    }
    const watcher = new FSWatcher(options);
    if (typeof listener === 'function') {
      watcher.on('change', listener);
    }
    watcher[kFSWatchStart](filename);
    return watcher;
  }

  return {
    FSWatcher,
    kFSWatchStart,
    watchRecursive,
  };
})();
"#;

// ═══════════════════════════════════════════════════════════════════════════
// internal/fs/promises — node's fs/promises re-export face (the corpus
// destructures FileHandle from here and patches its prototype; the SAME
// constructor backs fs/promises, which is the node relationship).
// ═══════════════════════════════════════════════════════════════════════════
const FS_PROMISES_JS: &str = r#"
(function() {
  'use strict';
  return require('fs/promises');
})();
"#;

// ═══════════════════════════════════════════════════════════════════════════
// internal/fs/glob — node v24.19.0 lib/internal/fs/glob.js wrapper port. The
// walk engine itself is native (node_fs.rs GlobEngine — node main's
// native-binding architecture); this JS face mirrors the v24.19.0 JS wrapper
// verbatim: Glob class (constructor validation + excludeAdapter + toDirent +
// globSync/glob/globAll), matchGlobPattern, createMatcher (minimal shim: the
// corpus only consumes its .match), and a GlobSync alias for the pre-native
// module shape cited by the golden triage.
//
// The native binding arrives as the temporary global __bao_internal_glob
// (installed + deleted by ensure_installed around the family eval).
// Deviations (all inline-marked): maxDepth is node-main-only (absent at
// v24.19.0) and not exposed; the exclude-fn payload crosses the native
// boundary as a plain {name,type,parent} record that the adapter wraps into
// the canonical utils Dirent (same observable shape as node's readdir Dirent).
// ═══════════════════════════════════════════════════════════════════════════
const FS_GLOB_JS: &str = r##"
(function() {
  'use strict';

  const native = globalThis.__bao_internal_glob;
  const {
    kFlagWindows, kFlagDot, kFlagFollowSymlinks, kFlagWithFileTypes, kFlagMatchBase,
  } = native;

  const { Dirent } = require('internal/fs/utils');
  const path = require('path');

  const kEmptyObject = Object.freeze({ __proto__: null });

  // #private → Symbol 槽 (本文件翻译约定: SM 类私有字段以 Symbol 属性替代)
  const kRoot = Symbol('glob.root');
  const kPatterns = Symbol('glob.patterns');
  const kExcludePatterns = Symbol('glob.excludePatterns');
  const kExcludeFn = Symbol('glob.excludeFn');
  const kWithFileTypes = Symbol('glob.withFileTypes');
  const kFollowSymlinks = Symbol('glob.followSymlinks');

  function toPathIfFileURL(p) {
    if (p !== null && typeof p === 'object' && typeof p.href === 'string') {
      if (!p.href.startsWith('file:')) {
        const err = new Error('The URL must be of scheme file');
        err.code = 'ERR_INVALID_URL_SCHEME';
        throw err;
      }
      return decodeURIComponent(p.href.replace(/^file:\/\//, '').replace(/^[^/]*/, ''));
    }
    return p;
  }

  function invalidArgType(name, expected, value) {
    const detail = value === null ? 'null'
      : typeof value === 'string' ? `type string (${JSON.stringify(value)})`
      : typeof value;
    const err = new TypeError(
      `The "${name}" must be of type ${expected}. Received ${detail}`);
    err.code = 'ERR_INVALID_ARG_TYPE';
    return err;
  }

  function validateObject(value, name) {
    if (value === null || Array.isArray(value) || typeof value !== 'object') {
      throw invalidArgType(name, 'object', value);
    }
  }

  function validateBoolean(value, name) {
    if (typeof value !== 'boolean') {
      const err = new TypeError(
        `The "${name}" option must be of type boolean. Received ${
          value === null ? 'null' : typeof value}`);
      err.code = 'ERR_INVALID_ARG_TYPE';
      throw err;
    }
  }

  function validateStringArrayOrFunction(value, name) {
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; ++i) {
        if (typeof value[i] !== 'string') {
          throw invalidArgType(`${name}[${i}]`, 'string', value[i]);
        }
      }
      return;
    }
    if (typeof value !== 'function') {
      throw invalidArgType(name, ['string[]', 'function'], value);
    }
  }

  function validateString(value, name) {
    if (typeof value !== 'string') throw invalidArgType(name, 'string', value);
  }

  function getDirentSync(p) {
    try {
      const st = require('fs').lstatSync(p, { __proto__: null, throwIfNoEntry: false });
      if (st === undefined) return null;
      const type = st.isDirectory() ? 2 : st.isSymbolicLink() ? 3 : 1;
      return new Dirent(path.basename(p), type, path.dirname(p));
    } catch {
      return null;
    }
  }

  function globFlags(g) {
    return (g[kFollowSymlinks] ? kFlagFollowSymlinks : 0) |
      (g[kWithFileTypes] ? kFlagWithFileTypes : 0);
  }

  // Shapes the exclude-fn payload across the native boundary: path strings
  // pass through; dirent records become canonical utils Dirents.
  function excludeAdapterOf(g) {
    const exclude = g[kExcludeFn];
    if (exclude === undefined) {
      return undefined;
    }
    if (!g[kWithFileTypes]) {
      return (value) => exclude(value);
    }
    return (payload) => {
      if (payload !== null && typeof payload === 'object') {
        return exclude(new Dirent(payload.name, payload.type, payload.parent));
      }
      const dirent = getDirentSync(payload);
      return dirent === null ? false : exclude(dirent);
    };
  }

  function toDirent(g, p, type) {
    const full = path.isAbsolute(p) ? p : path.join(g[kRoot], p);
    return new Dirent(path.basename(full), type, path.dirname(full));
  }

  class Glob {
    constructor(pattern, options = kEmptyObject) {
      validateObject(options, 'options');
      const { exclude, cwd, followSymlinks, withFileTypes } = options;
      this[kRoot] = toPathIfFileURL(cwd) ?? '.';
      validateString(this[kRoot], 'options.cwd');
      if (followSymlinks != null) {
        validateBoolean(followSymlinks, 'options.followSymlinks');
        this[kFollowSymlinks] = followSymlinks;
      }
      this[kWithFileTypes] = !!withFileTypes;
      this[kExcludePatterns] = [];
      if (exclude != null) {
        validateStringArrayOrFunction(exclude, 'options.exclude');
        if (Array.isArray(exclude)) {
          // Exclude patterns are matched against absolute paths (v24
          // constructor resolves them against root before matching).
          this[kExcludePatterns] = exclude.map(
            (p) => path.resolve(this[kRoot], p));
        } else {
          this[kExcludeFn] = exclude;
        }
      }
      if (typeof pattern === 'object') {
        if (!Array.isArray(pattern)) {
          throw invalidArgType('patterns', ['string', 'string[]'], pattern);
        }
        for (const p of pattern) validateString(p, 'patterns');
        this[kPatterns] = pattern;
      } else {
        validateString(pattern, 'patterns');
        this[kPatterns] = [pattern];
      }
    }

    get hasMagic() {
      for (const p of this[kPatterns]) {
        if (native.hasMagic(p)) return true;
      }
      return false;
    }

    globSync() {
      const [paths, types] = native.globSync(
        this[kRoot], this[kPatterns], this[kExcludePatterns], globFlags(this),
        excludeAdapterOf(this));
      if (!this[kWithFileTypes]) {
        return paths;
      }
      const dirents = [];
      for (let i = 0; i < paths.length; i++) {
        dirents.push(toDirent(this, paths[i], types[i]));
      }
      return dirents;
    }

    async *glob() {
      const [paths, types] = native.globSync(
        this[kRoot], this[kPatterns], this[kExcludePatterns], globFlags(this),
        excludeAdapterOf(this));
      for (let i = 0; i < paths.length; i++) {
        yield this[kWithFileTypes] ? toDirent(this, paths[i], types[i]) : paths[i];
      }
    }

    async globAll() {
      const [paths, types] = native.globSync(
        this[kRoot], this[kPatterns], this[kExcludePatterns], globFlags(this),
        excludeAdapterOf(this));
      if (!this[kWithFileTypes]) {
        return paths;
      }
      const dirents = [];
      for (let i = 0; i < paths.length; i++) {
        dirents.push(toDirent(this, paths[i], types[i]));
      }
      return dirents;
    }
  }

  function matchGlobPattern(p, pattern) {
    validateString(p, 'path');
    validateString(pattern, 'pattern');
    return native.matchPath(p, pattern);
  }

  // Minimal shim over the native matcher — the corpus consumes only .match;
  // the full Minimatch face (set/globParts introspection) is not exposed.
  function createMatcher(pattern) {
    validateString(pattern, 'pattern');
    return {
      __proto__: null,
      match: (f) => native.matchPath(f, pattern),
    };
  }

  return {
    __proto__: null,
    Glob,
    // Pre-native module shape alias (triage cited {Glob, GlobSync}); at
    // v24.19.0 the module exports {Glob, createMatcher, matchGlobPattern}.
    GlobSync: Glob,
    createMatcher,
    matchGlobPattern,
  };
})();
"##;
