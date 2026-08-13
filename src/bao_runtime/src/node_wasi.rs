// @trace REQ-ENG-008 [api:wasi] — WASI (WebAssembly System Interface) module
//! node:wasi module — WASI preview1 implementation via JS IIFE.
//!
//! Provides the `WASI` class and `wasiSnapshotPreview1` object that enable
//! running WebAssembly modules compiled to the WASI ABI. The implementation
//! uses pure JS to bridge WASI syscalls to the host environment.
//!
//! ## Architecture
//!
//! Follows the same JS IIFE pattern as node_stream.rs / node_vm.rs:
//! - `WASI_SOURCE` const holds the JS source
//! - `install()` evaluates the IIFE, extracts the returned object, and
//!   registers it via `cache_builtin(cx, "wasi", ...)`
//!
//! ## References
//!
//! - Bun upstream: `src/js/node/wasi.ts`
//! - WASI preview1 spec: https://github.com/WebAssembly/WASI/blob/main/phases/snapshot/docs.md

use bun_core::ZBox;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const WASI_SOURCE: &str = r#"
(function() {
  // ── errno constants ──
  var ESUCCESS = 0;
  var E2BIG = 1;
  var EACCES = 2;
  var EADDRINUSE = 3;
  var EADDRNOTAVAIL = 4;
  var EAFNOSUPPORT = 5;
  var EAGAIN = 6;
  var EALREADY = 7;
  var EBADF = 8;
  var EBADMSG = 9;
  var EBUSY = 10;
  var ECANCELED = 11;
  var ECHILD = 12;
  var ECONNABORTED = 13;
  var ECONNREFUSED = 14;
  var ECONNRESET = 15;
  var EDEADLK = 16;
  var EDESTADDRREQ = 17;
  var EDOM = 18;
  var EDQUOT = 19;
  var EEXIST = 20;
  var EFAULT = 21;
  var EFBIG = 22;
  var EHOSTUNREACH = 23;
  var EIDRM = 24;
  var EILSEQ = 25;
  var EINPROGRESS = 26;
  var EINTR = 27;
  var EINVAL = 28;
  var EIO = 29;
  var EISCONN = 30;
  var EISDIR = 31;
  var ELOOP = 32;
  var EMFILE = 33;
  var EMLINK = 34;
  var EMSGSIZE = 35;
  var EMULTIHOP = 36;
  var ENAMETOOLONG = 37;
  var ENETDOWN = 38;
  var ENETRESET = 39;
  var ENETUNREACH = 40;
  var ENFILE = 41;
  var ENOBUFS = 42;
  var ENODEV = 43;
  var ENOENT = 44;
  var ENOEXEC = 45;
  var ENOLCK = 46;
  var ENOLINK = 47;
  var ENOMEM = 48;
  var ENOMSG = 49;
  var ENOPROTOOPT = 50;
  var ENOSPC = 51;
  var ENOSYS = 52;
  var ENOTCONN = 53;
  var ENOTDIR = 54;
  var ENOTEMPTY = 55;
  var ENOTRECOVERABLE = 56;
  var ENOTSOCK = 57;
  var ENOTSUP = 58;
  var ENOTTY = 59;
  var ENXIO = 60;
  var EOVERFLOW = 61;
  var EOWNERDEAD = 62;
  var EPERM = 63;
  var EPIPE = 64;
  var EPROTO = 65;
  var EPROTONOSUPPORT = 66;
  var EPROTOTYPE = 67;
  var ERANGE = 68;
  var EROFS = 69;
  var ESPIPE = 70;
  var ESRCH = 71;
  var ESTALE = 72;
  var ETIMEDOUT = 73;
  var ETXTBSY = 74;
  var EXDEV = 75;
  var ENOTCAPABLE = 76;

  // ── file type constants ──
  var FILETYPE_UNKNOWN = 0;
  var FILETYPE_BLOCK_DEVICE = 1;
  var FILETYPE_CHARACTER_DEVICE = 2;
  var FILETYPE_DIRECTORY = 3;
  var FILETYPE_REGULAR_FILE = 4;
  var FILETYPE_SYMBOLIC_LINK = 7;
  var FILETYPE_SOCKET_STREAM = 6;
  var FILETYPE_SOCKET_DGRAM = 5;

  // ── clock constants ──
  var CLOCK_REALTIME = 0;
  var CLOCK_MONOTONIC = 1;
  var CLOCK_PROCESS_CPUTIME_ID = 2;
  var CLOCK_THREAD_CPUTIME_ID = 3;

  // ── stdio constants ──
  var STDIN_FILENO = 0;
  var STDOUT_FILENO = 1;
  var STDERR_FILENO = 2;

  // ── signal constants ──
  var SIGHUP = 1;
  var SIGINT = 2;
  var SIGQUIT = 3;
  var SIGILL = 4;
  var SIGTRAP = 5;
  var SIGABRT = 6;
  var SIGBUS = 7;
  var SIGFPE = 8;
  var SIGKILL = 9;
  var SIGUSR1 = 10;
  var SIGSEGV = 11;
  var SIGUSR2 = 12;
  var SIGPIPE = 13;
  var SIGALRM = 14;
  var SIGTERM = 15;
  var SIGCHLD = 16;
  var SIGCONT = 17;
  var SIGSTOP = 19;
  var SIGTSTP = 20;
  var SIGTTIN = 21;
  var SIGTTOU = 22;
  var SIGURG = 23;
  var SIGXCPU = 24;
  var SIGXFSZ = 25;
  var SIGVTALRM = 26;

  // ── fdflags constants ──
  var FDFLAG_APPEND = 1;
  var FDFLAG_DSYNC = 2;
  var FDFLAG_NONBLOCK = 4;
  var FDFLAG_RSYNC = 8;
  var FDFLAG_SYNC = 16;

  // ── oflags constants ──
  var OFLAG_CREAT = 1;
  var OFLAG_DIRECTORY = 2;
  var OFLAG_EXCL = 4;
  var OFLAG_TRUNC = 8;

  // ── rights constants ──
  var RIGHT_FD_DATASYNC = 1;
  var RIGHT_FD_READ = 2;
  var RIGHT_FD_SEEK = 4;
  var RIGHT_FD_FDSTAT_SET_FLAGS = 8;
  var RIGHT_FD_SYNC = 16;
  var RIGHT_FD_TELL = 32;
  var RIGHT_FD_WRITE = 64;
  var RIGHT_FD_ADVISE = 128;
  var RIGHT_FD_ALLOCATE = 256;
  var RIGHT_PATH_CREATE_DIRECTORY = 512;
  var RIGHT_PATH_CREATE_FILE = 1024;
  var RIGHT_PATH_LINK_SOURCE = 2048;
  var RIGHT_PATH_LINK_TARGET = 4096;
  var RIGHT_PATH_OPEN = 8192;
  var RIGHT_FD_READDIR = 16384;
  var RIGHT_PATH_READLINK = 32768;
  var RIGHT_PATH_RENAME_SOURCE = 65536;
  var RIGHT_PATH_RENAME_TARGET = 131072;
  var RIGHT_PATH_FILESTAT_GET = 262144;
  var RIGHT_PATH_FILESTAT_SET_SIZE = 524288;
  var RIGHT_PATH_FILESTAT_SET_TIMES = 1048576;
  var RIGHT_FD_FILESTAT_GET = 2097152;
  var RIGHT_FD_FILESTAT_SET_SIZE = 4194304;
  var RIGHT_FD_FILESTAT_SET_TIMES = 8388608;
  var RIGHT_PATH_SYMLINK = 16777216;
  var RIGHT_PATH_REMOVE_DIRECTORY = 33554432;
  var RIGHT_PATH_UNLINK_FILE = 67108864;
  var RIGHT_POLL_FD_READWRITE = 134217728;
  var RIGHT_SOCK_SHUTDOWN = 268435456;

  var RIGHTS_ALL = RIGHT_FD_DATASYNC | RIGHT_FD_READ | RIGHT_FD_SEEK |
    RIGHT_FD_FDSTAT_SET_FLAGS | RIGHT_FD_SYNC | RIGHT_FD_TELL | RIGHT_FD_WRITE |
    RIGHT_FD_ADVISE | RIGHT_FD_ALLOCATE | RIGHT_PATH_CREATE_DIRECTORY |
    RIGHT_PATH_CREATE_FILE | RIGHT_PATH_LINK_SOURCE | RIGHT_PATH_LINK_TARGET |
    RIGHT_PATH_OPEN | RIGHT_FD_READDIR | RIGHT_PATH_READLINK |
    RIGHT_PATH_RENAME_SOURCE | RIGHT_PATH_RENAME_TARGET |
    RIGHT_PATH_FILESTAT_GET | RIGHT_PATH_FILESTAT_SET_SIZE |
    RIGHT_PATH_FILESTAT_SET_TIMES | RIGHT_FD_FILESTAT_GET |
    RIGHT_FD_FILESTAT_SET_SIZE | RIGHT_FD_FILESTAT_SET_TIMES |
    RIGHT_PATH_SYMLINK | RIGHT_PATH_REMOVE_DIRECTORY | RIGHT_PATH_UNLINK_FILE |
    RIGHT_POLL_FD_READWRITE | RIGHT_SOCK_SHUTDOWN;

  var RIGHTS_REGULAR_FILE_BASE = RIGHT_FD_DATASYNC | RIGHT_FD_READ |
    RIGHT_FD_SEEK | RIGHT_FD_FDSTAT_SET_FLAGS | RIGHT_FD_SYNC |
    RIGHT_FD_TELL | RIGHT_FD_WRITE | RIGHT_FD_ADVISE | RIGHT_FD_ALLOCATE |
    RIGHT_FD_FILESTAT_GET | RIGHT_FD_FILESTAT_SET_SIZE |
    RIGHT_FD_FILESTAT_SET_TIMES | RIGHT_POLL_FD_READWRITE;

  var RIGHTS_REGULAR_FILE_INHERITING = 0;

  var RIGHTS_DIRECTORY_BASE = RIGHT_FD_FDSTAT_SET_FLAGS | RIGHT_FD_SYNC |
    RIGHT_FD_READDIR | RIGHT_PATH_CREATE_DIRECTORY | RIGHT_PATH_CREATE_FILE |
    RIGHT_PATH_LINK_SOURCE | RIGHT_PATH_LINK_TARGET | RIGHT_PATH_OPEN |
    RIGHT_PATH_READLINK | RIGHT_PATH_RENAME_SOURCE |
    RIGHT_PATH_RENAME_TARGET | RIGHT_PATH_FILESTAT_GET |
    RIGHT_PATH_FILESTAT_SET_SIZE | RIGHT_PATH_FILESTAT_SET_TIMES |
    RIGHT_PATH_SYMLINK | RIGHT_PATH_REMOVE_DIRECTORY | RIGHT_PATH_UNLINK_FILE |
    RIGHT_POLL_FD_READWRITE;

  var RIGHTS_DIRECTORY_INHERITING = RIGHTS_DIRECTORY_BASE | RIGHTS_REGULAR_FILE_BASE;

  var RIGHTS_BLOCK_DEVICE_BASE = RIGHTS_ALL;
  var RIGHTS_BLOCK_DEVICE_INHERITING = RIGHTS_ALL;

  var RIGHTS_CHARACTER_DEVICE_BASE = RIGHTS_ALL;
  var RIGHTS_CHARACTER_DEVICE_INHERITING = RIGHTS_ALL;

  // ── error classes ──
  function WASIError(errno) {
    var e = new Error('WASI error');
    e.errno = errno;
    e.name = 'WASIError';
    return e;
  }
  function WASIExitError(code) {
    var e = new Error('WASI Exit error: ' + code);
    e.code = code;
    e.name = 'WASIExitError';
    return e;
  }
  function WASIKillError(signal) {
    var e = new Error('WASI Kill signal: ' + signal);
    e.signal = signal;
    e.name = 'WASIKillError';
    return e;
  }

  // ── WASI class ──
  function WASI(options) {
    if (!(this instanceof WASI)) return new WASI(options);
    options = options || {};
    this._args = options.args || [];
    this._env = options.env || [];
    this._preopens = options.preopens || {};
    this._memory = null;
    this._fds = [];
    this._nextFd = 3; // 0=stdin, 1=stdout, 2=stderr

    // Set up preopens
    var self = this;
    var preopenNames = Object.keys(this._preopens);
    for (var i = 0; i < preopenNames.length; i++) {
      var name = preopenNames[i];
      self._fds.push({
        fd: self._nextFd,
        path: self._preopens[name],
        preopen: name,
        type: FILETYPE_DIRECTORY,
        rights: RIGHTS_DIRECTORY_BASE,
        rightsInheriting: RIGHTS_DIRECTORY_INHERITING,
        offset: 0
      });
      self._nextFd++;
    }
  }

  // start() and initialize() — set up the WASI imports on the instance
  WASI.prototype.start = function(instance) {
    var exports = instance.exports;
    if (exports.memory) this._setMemory(exports.memory);
    if (exports._start) {
      try {
        exports._start();
      } catch (e) {
        if (e && e.code !== undefined) throw new WASIExitError(e.code);
        throw e;
      }
    }
  };

  WASI.prototype.initialize = function(instance) {
    var exports = instance.exports;
    if (exports.memory) this._setMemory(exports.memory);
    if (exports._initialize) {
      exports._initialize();
    }
  };

  WASI.prototype._setMemory = function(memory) {
    this._memory = memory;
  };

  // ── wasiSnapshotPreview1 ──
  // Helper: read a null-terminated string from memory
  function readString(mem, ptr) {
    var view = new Uint8Array(mem.buffer);
    var end = ptr;
    while (view[end] !== 0) end++;
    return new TextDecoder().decode(view.slice(ptr, end));
  }

  // Helper: write a string to memory at ptr (null-terminated)
  function writeString(mem, ptr, str) {
    var view = new Uint8Array(mem.buffer);
    var encoded = new TextEncoder().encode(str);
    for (var i = 0; i < encoded.length; i++) view[ptr + i] = encoded[i];
    view[ptr + encoded.length] = 0;
  }

  // Helper: read a u32 from memory at ptr
  function readU32(mem, ptr) {
    var view = new DataView(mem.buffer);
    return view.getUint32(ptr, true);
  }

  // Helper: write a u32 to memory at ptr
  function writeU32(mem, ptr, val) {
    var view = new DataView(mem.buffer);
    view.setUint32(ptr, val, true);
  }

  // Helper: write a u64 to memory at ptr
  function writeU64(mem, ptr, hi, lo) {
    var view = new DataView(mem.buffer);
    view.setUint32(ptr, lo !== undefined ? lo : (hi & 0xFFFFFFFF), true);
    view.setUint32(ptr + 4, hi !== undefined ? (lo !== undefined ? hi : (hi / 0x100000000) | 0) : 0, true);
  }

  // Helper: read a u64 from memory at ptr, returns [hi, lo]
  function readU64(mem, ptr) {
    var view = new DataView(mem.buffer);
    var lo = view.getUint32(ptr, true);
    var hi = view.getUint32(ptr + 4, true);
    return [hi, lo];
  }

  var wasiSnapshotPreview1 = {
    args_get: function(argv, argv_buf) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var view = new Uint8Array(mem.buffer);
      var dv = new DataView(mem.buffer);
      var bufPtr = argv_buf;
      for (var i = 0; i < this._args.length; i++) {
        dv.setUint32(argv + i * 4, bufPtr, true);
        var encoded = new TextEncoder().encode(this._args[i]);
        for (var j = 0; j < encoded.length; j++) view[bufPtr + j] = encoded[j];
        view[bufPtr + encoded.length] = 0;
        bufPtr += encoded.length + 1;
      }
      return ESUCCESS;
    },

    args_sizes_get: function(argc, argv_buf_size) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      writeU32(mem, argc, this._args.length);
      var totalSize = 0;
      for (var i = 0; i < this._args.length; i++) {
        totalSize += new TextEncoder().encode(this._args[i]).length + 1;
      }
      writeU32(mem, argv_buf_size, totalSize);
      return ESUCCESS;
    },

    environ_get: function(environ, environ_buf) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var view = new Uint8Array(mem.buffer);
      var dv = new DataView(mem.buffer);
      var bufPtr = environ_buf;
      for (var i = 0; i < this._env.length; i++) {
        dv.setUint32(environ + i * 4, bufPtr, true);
        var encoded = new TextEncoder().encode(this._env[i]);
        for (var j = 0; j < encoded.length; j++) view[bufPtr + j] = encoded[j];
        view[bufPtr + encoded.length] = 0;
        bufPtr += encoded.length + 1;
      }
      return ESUCCESS;
    },

    environ_sizes_get: function(environc, environ_buf_size) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      writeU32(mem, environc, this._env.length);
      var totalSize = 0;
      for (var i = 0; i < this._env.length; i++) {
        totalSize += new TextEncoder().encode(this._env[i]).length + 1;
      }
      writeU32(mem, environ_buf_size, totalSize);
      return ESUCCESS;
    },

    clock_res_get: function(clockId, resolution) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (clockId === CLOCK_REALTIME || clockId === CLOCK_MONOTONIC) {
        writeU64(mem, resolution, 0, 1000000); // 1ms resolution
      } else {
        writeU64(mem, resolution, 0, 1000);
      }
      return ESUCCESS;
    },

    clock_time_get: function(clockId, precision, time) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var now;
      if (clockId === CLOCK_REALTIME) {
        now = Date.now() * 1000000; // milliseconds → nanoseconds
      } else if (clockId === CLOCK_MONOTONIC) {
        if (typeof performance !== 'undefined' && performance.now) {
          now = Math.floor(performance.now() * 1000000);
        } else {
          now = Date.now() * 1000000;
        }
      } else {
        now = Date.now() * 1000000;
      }
      var lo = now & 0xFFFFFFFF;
      var hi = (now / 0x100000000) | 0;
      writeU64(mem, time, hi, lo);
      return ESUCCESS;
    },

    fd_close: function(fd) {
      return ESUCCESS;
    },

    fd_datasync: function(fd) {
      return ESUCCESS;
    },

    fd_fdstat_get: function(fd, stat) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var dv = new DataView(mem.buffer);
      if (fd === 0 || fd === 1 || fd === 2) {
        dv.setUint8(stat, FILETYPE_CHARACTER_DEVICE);
        dv.setUint16(stat + 2, FDFLAG_APPEND, true);
        dv.setUint32(stat + 8, RIGHTS_ALL, true);
        dv.setUint32(stat + 12, 0, true);
        dv.setUint32(stat + 16, RIGHTS_ALL, true);
        dv.setUint32(stat + 20, 0, true);
        return ESUCCESS;
      }
      var fdesc = this._fds[fd - 3];
      if (!fdesc) return EBADF;
      dv.setUint8(stat, fdesc.type);
      dv.setUint16(stat + 2, 0, true);
      dv.setUint32(stat + 8, fdesc.rights >>> 0, true);
      dv.setUint32(stat + 12, (fdesc.rights / 0x100000000) | 0, true);
      dv.setUint32(stat + 16, fdesc.rightsInheriting >>> 0, true);
      dv.setUint32(stat + 20, (fdesc.rightsInheriting / 0x100000000) | 0, true);
      return ESUCCESS;
    },

    fd_fdstat_set_flags: function(fd, flags) {
      return ENOSYS;
    },

    fd_filestat_get: function(fd, buf) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var dv = new DataView(mem.buffer);
      // dev, ino, filetype, nlink, size, atim, mtim, ctim
      dv.setUint64(buf, 0, true);         // dev
      dv.setUint64(buf + 8, fd, true);    // ino
      if (fd === 0 || fd === 1 || fd === 2) {
        dv.setUint8(buf + 16, FILETYPE_CHARACTER_DEVICE);
      } else {
        var fdesc = this._fds[fd - 3];
        dv.setUint8(buf + 16, fdesc ? fdesc.type : FILETYPE_UNKNOWN);
      }
      dv.setUint64(buf + 24, 1, true);    // nlink
      dv.setUint64(buf + 32, 0, true);    // size
      var now = Date.now() * 1000000;
      var lo = now & 0xFFFFFFFF;
      var hi = (now / 0x100000000) | 0;
      dv.setUint32(buf + 40, lo, true);
      dv.setUint32(buf + 44, hi, true);
      dv.setUint32(buf + 48, lo, true);
      dv.setUint32(buf + 52, hi, true);
      dv.setUint32(buf + 56, lo, true);
      dv.setUint32(buf + 60, hi, true);
      return ESUCCESS;
    },

    fd_filestat_set_size: function(fd, size) {
      return ENOSYS;
    },

    fd_filestat_set_times: function(fd, atim, mtim, fst_flags) {
      return ENOSYS;
    },

    fd_prestat_get: function(fd, buf) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd < 3 || fd >= this._nextFd) return EBADF;
      var fdesc = this._fds[fd - 3];
      if (!fdesc || !fdesc.preopen) return EBADF;
      var dv = new DataView(mem.buffer);
      dv.setUint8(buf, FILETYPE_DIRECTORY);
      var nameLen = new TextEncoder().encode(fdesc.preopen).length;
      dv.setUint32(buf + 4, nameLen, true);
      return ESUCCESS;
    },

    fd_prestat_dir_name: function(fd, path, path_len) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd < 3 || fd >= this._nextFd) return EBADF;
      var fdesc = this._fds[fd - 3];
      if (!fdesc || !fdesc.preopen) return EBADF;
      var view = new Uint8Array(mem.buffer);
      var encoded = new TextEncoder().encode(fdesc.preopen);
      var n = Math.min(encoded.length, path_len);
      for (var i = 0; i < n; i++) view[path + i] = encoded[i];
      return ESUCCESS;
    },

    fd_read: function(fd, iovs, iovs_len, nread) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd !== 0) return EBADF; // only stdin for now
      writeU32(mem, nread, 0);
      return ESUCCESS;
    },

    fd_readdir: function(fd, buf, buf_len, cookie, bufused) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      writeU32(mem, bufused, 0);
      return ENOSYS;
    },

    fd_seek: function(fd, offset, whence, newoffset) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd < 3) return ESPIPE;
      var fdesc = this._fds[fd - 3];
      if (!fdesc) return EBADF;
      if (whence === 0) fdesc.offset = offset;
      else if (whence === 1) fdesc.offset += offset;
      else if (whence === 2) fdesc.offset = 0; // SEEK_END not supported
      writeU64(mem, newoffset, 0, fdesc.offset);
      return ESUCCESS;
    },

    fd_sync: function(fd) {
      return ESUCCESS;
    },

    fd_tell: function(fd, offset) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd < 3) return EBADF;
      var fdesc = this._fds[fd - 3];
      if (!fdesc) return EBADF;
      writeU64(mem, offset, 0, fdesc.offset || 0);
      return ESUCCESS;
    },

    fd_write: function(fd, iovs, iovs_len, nwritten) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      if (fd !== 1 && fd !== 2) return EBADF;
      var view = new Uint8Array(mem.buffer);
      var dv = new DataView(mem.buffer);
      var total = 0;
      for (var i = 0; i < iovs_len; i++) {
        var buf = dv.getUint32(iovs + i * 8, true);
        var bufLen = dv.getUint32(iovs + i * 8 + 4, true);
        var slice = new Uint8Array(mem.buffer, buf, bufLen);
        var str = '';
        for (var j = 0; j < bufLen; j++) str += String.fromCharCode(slice[j]);
        if (fd === 1) {
          if (typeof process !== 'undefined' && process.stdout && typeof process.stdout.write === 'function') {
            process.stdout.write(str);
          } else {
            // Use console.log as fallback
            console.log(str);
          }
        } else {
          if (typeof process !== 'undefined' && process.stderr && typeof process.stderr.write === 'function') {
            process.stderr.write(str);
          } else {
            console.error(str);
          }
        }
        total += bufLen;
      }
      writeU32(mem, nwritten, total);
      return ESUCCESS;
    },

    path_create_directory: function(fd, path, path_len) {
      return ENOSYS;
    },

    path_filestat_get: function(fd, flags, path, path_len, buf) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      return ENOSYS;
    },

    path_filestat_set_times: function(fd, flags, path, path_len, atim, mtim, fst_flags) {
      return ENOSYS;
    },

    path_link: function(old_fd, old_flags, old_path, old_path_len, new_fd, new_path, new_path_len) {
      return ENOSYS;
    },

    path_open: function(fd, dirflags, path, path_len, oflags, fs_rights_base, fs_rights_inheriting, fdflags, opened_fd) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var pathStr = readString(mem, path);
      var newFd = this._nextFd;
      this._fds.push({
        fd: newFd,
        path: pathStr,
        type: FILETYPE_REGULAR_FILE,
        rights: RIGHTS_REGULAR_FILE_BASE,
        rightsInheriting: RIGHTS_REGULAR_FILE_INHERITING,
        offset: 0
      });
      this._nextFd++;
      writeU32(mem, opened_fd, newFd);
      return ESUCCESS;
    },

    path_readlink: function(fd, path, path_len, buf, buf_len, bufused) {
      return ENOSYS;
    },

    path_remove_directory: function(fd, path, path_len) {
      return ENOSYS;
    },

    path_rename: function(old_fd, old_path, old_path_len, new_fd, new_path, new_path_len) {
      return ENOSYS;
    },

    path_symlink: function(old_path, old_path_len, fd, new_path, new_path_len) {
      return ENOSYS;
    },

    path_unlink_file: function(fd, path, path_len) {
      return ENOSYS;
    },

    poll_oneoff: function(in_ptr, out_ptr, nsubscriptions, nevents) {
      return ENOSYS;
    },

    proc_exit: function(rval) {
      throw { code: rval };
    },

    proc_raise: function(sig) {
      throw new WASIKillError(sig);
    },

    random_get: function(buf, buf_len) {
      var mem = this._memory;
      if (!mem) return EINVAL;
      var view = new Uint8Array(mem.buffer);
      if (typeof crypto !== 'undefined' && crypto.getRandomValues) {
        var arr = new Uint8Array(buf_len);
        crypto.getRandomValues(arr);
        for (var i = 0; i < buf_len; i++) view[buf + i] = arr[i];
      } else {
        for (var i = 0; i < buf_len; i++) view[buf + i] = (Math.random() * 256) | 0;
      }
      return ESUCCESS;
    },

    sched_yield: function() {
      return ESUCCESS;
    },

    sock_recv: function(fd, ri_data, ri_data_len, ri_flags, ro_datalen, ro_flags) {
      return ENOSYS;
    },

    sock_send: function(fd, si_data, si_data_len, si_flags, so_datalen) {
      return ENOSYS;
    },

    sock_shutdown: function(fd, how) {
      return ENOSYS;
    }
  };

  // Build the wasiSnapshotPreview1 with `this` bound to the WASI instance.
  // The WASI class wraps the raw wasiSnapshotPreview1 functions to pass _memory.
  var wasiPreview1Names = [
    'args_get', 'args_sizes_get', 'environ_get', 'environ_sizes_get',
    'clock_res_get', 'clock_time_get',
    'fd_close', 'fd_datasync', 'fd_fdstat_get', 'fd_fdstat_set_flags',
    'fd_filestat_get', 'fd_filestat_set_size', 'fd_filestat_set_times',
    'fd_prestat_get', 'fd_prestat_dir_name',
    'fd_read', 'fd_readdir', 'fd_seek', 'fd_sync', 'fd_tell', 'fd_write',
    'path_create_directory', 'path_filestat_get', 'path_filestat_set_times',
    'path_link', 'path_open', 'path_readlink', 'path_remove_directory',
    'path_rename', 'path_symlink', 'path_unlink_file',
    'poll_oneoff', 'proc_exit', 'proc_raise', 'random_get', 'sched_yield',
    'sock_recv', 'sock_send', 'sock_shutdown'
  ];

  WASI.prototype.wasiSnapshotPreview1 = function() {
    var self = this;
    var obj = {};
    for (var i = 0; i < wasiPreview1Names.length; i++) {
      (function(name) {
        obj[name] = function() {
          var args = Array.prototype.slice.call(arguments);
          return wasiSnapshotPreview1[name].apply(self, args);
        };
      })(wasiPreview1Names[i]);
    }
    return obj;
  };

  // Also expose wasiSnapshotPreview1 as a static object for require("wasi").wasiSnapshotPreview1
  var staticPreview1 = {};
  for (var i = 0; i < wasiPreview1Names.length; i++) {
    staticPreview1[wasiPreview1Names[i]] = (function(name) {
      return function() {
        return wasiSnapshotPreview1[name].apply(this, arguments);
      };
    })(wasiPreview1Names[i]);
  }

  return {
    WASI: WASI,
    WASIError: WASIError,
    WASIExitError: WASIExitError,
    WASIKillError: WASIKillError,
    wasiSnapshotPreview1: staticPreview1,
    // errno constants
    ESUCCESS: ESUCCESS, E2BIG: E2BIG, EACCES: EACCES,
    EADDRINUSE: EADDRINUSE, EADDRNOTAVAIL: EADDRNOTAVAIL,
    EAFNOSUPPORT: EAFNOSUPPORT, EAGAIN: EAGAIN, EALREADY: EALREADY,
    EBADF: EBADF, EBADMSG: EBADMSG, EBUSY: EBUSY, ECANCELED: ECANCELED,
    ECHILD: ECHILD, ECONNABORTED: ECONNABORTED, ECONNREFUSED: ECONNREFUSED,
    ECONNRESET: ECONNRESET, EDEADLK: EDEADLK, EDESTADDRREQ: EDESTADDRREQ,
    EDOM: EDOM, EDQUOT: EDQUOT, EEXIST: EEXIST, EFAULT: EFAULT,
    EFBIG: EFBIG, EHOSTUNREACH: EHOSTUNREACH, EIDRM: EIDRM,
    EILSEQ: EILSEQ, EINPROGRESS: EINPROGRESS, EINTR: EINTR,
    EINVAL: EINVAL, EIO: EIO, EISCONN: EISCONN, EISDIR: EISDIR,
    ELOOP: ELOOP, EMFILE: EMFILE, EMLINK: EMLINK, EMSGSIZE: EMSGSIZE,
    EMULTIHOP: EMULTIHOP, ENAMETOOLONG: ENAMETOOLONG, ENETDOWN: ENETDOWN,
    ENETRESET: ENETRESET, ENETUNREACH: ENETUNREACH, ENFILE: ENFILE,
    ENOBUFS: ENOBUFS, ENODEV: ENODEV, ENOENT: ENOENT, ENOEXEC: ENOEXEC,
    ENOLCK: ENOLCK, ENOLINK: ENOLINK, ENOMEM: ENOMEM, ENOMSG: ENOMSG,
    ENOPROTOOPT: ENOPROTOOPT, ENOSPC: ENOSPC, ENOSYS: ENOSYS,
    ENOTCONN: ENOTCONN, ENOTDIR: ENOTDIR, ENOTEMPTY: ENOTEMPTY,
    ENOTRECOVERABLE: ENOTRECOVERABLE, ENOTSOCK: ENOTSOCK,
    ENOTSUP: ENOTSUP, ENOTTY: ENOTTY, ENXIO: ENXIO,
    EOVERFLOW: EOVERFLOW, EOWNERDEAD: EOWNERDEAD, EPERM: EPERM,
    EPIPE: EPIPE, EPROTO: EPROTO, EPROTONOSUPPORT: EPROTONOSUPPORT,
    EPROTOTYPE: EPROTOTYPE, ERANGE: ERANGE, EROFS: EROFS,
    ESPIPE: ESPIPE, ESRCH: ESRCH, ESTALE: ESTALE, ETIMEDOUT: ETIMEDOUT,
    ETXTBSY: ETXTBSY, EXDEV: EXDEV, ENOTCAPABLE: ENOTCAPABLE,
    // file type constants
    FILETYPE_UNKNOWN: FILETYPE_UNKNOWN,
    FILETYPE_BLOCK_DEVICE: FILETYPE_BLOCK_DEVICE,
    FILETYPE_CHARACTER_DEVICE: FILETYPE_CHARACTER_DEVICE,
    FILETYPE_DIRECTORY: FILETYPE_DIRECTORY,
    FILETYPE_REGULAR_FILE: FILETYPE_REGULAR_FILE,
    FILETYPE_SYMBOLIC_LINK: FILETYPE_SYMBOLIC_LINK,
    FILETYPE_SOCKET_STREAM: FILETYPE_SOCKET_STREAM,
    FILETYPE_SOCKET_DGRAM: FILETYPE_SOCKET_DGRAM,
    // clock constants
    CLOCK_REALTIME: CLOCK_REALTIME,
    CLOCK_MONOTONIC: CLOCK_MONOTONIC,
    CLOCK_PROCESS_CPUTIME_ID: CLOCK_PROCESS_CPUTIME_ID,
    CLOCK_THREAD_CPUTIME_ID: CLOCK_THREAD_CPUTIME_ID,
    // stdio constants
    STDIN_FILENO: STDIN_FILENO,
    STDOUT_FILENO: STDOUT_FILENO,
    STDERR_FILENO: STDERR_FILENO,
    // fdflags
    FDFLAG_APPEND: FDFLAG_APPEND, FDFLAG_DSYNC: FDFLAG_DSYNC,
    FDFLAG_NONBLOCK: FDFLAG_NONBLOCK, FDFLAG_RSYNC: FDFLAG_RSYNC,
    FDFLAG_SYNC: FDFLAG_SYNC,
    // oflags
    OFLAG_CREAT: OFLAG_CREAT, OFLAG_DIRECTORY: OFLAG_DIRECTORY,
    OFLAG_EXCL: OFLAG_EXCL, OFLAG_TRUNC: OFLAG_TRUNC
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("node:wasi".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(WASI_SOURCE);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        rooted!(&in(cx) let exports_rooted = exports_obj);

        // Copy all named exports from the IIFE result onto the module object.
        // We enumerate the object's own properties to avoid hardcoding every
        // constant name.
        let mut ids = mozjs::rust::IdVector::new(cx_raw);
        let got = mozjs::jsapi::GetPropertyKeys(
            cx_raw,
            exports_rooted.handle().into(),
            mozjs::jsapi::JSITER_OWNONLY,
            ids.handle_mut(),
        );
        if got {
            for jsid in &*ids {
                if !jsid.is_string() {
                    continue;
                }
                let key_str_ptr = jsid.to_string();
                if key_str_ptr.is_null() {
                    continue;
                }

                let mut val = UndefinedValue();
                let id_h = Handle::<jsid> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: jsid as *const jsid as *mut jsid,
                };
                let val_h = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                };
                JS_GetPropertyById(cx_raw, exports_rooted.handle().into(), id_h, val_h);
                if val.is_undefined() {
                    continue;
                }
                rooted!(&in(cx) let val_root = val);

                let cname = ZBox::from_bytes(
                    mozjs::conversions::unsafe_jsstr_to_string(
                        cx_raw,
                        ::std::ptr::NonNull::new_unchecked(key_str_ptr),
                    )
                    .as_bytes(),
                );
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "wasi", mod_obj.get());
    }
}
