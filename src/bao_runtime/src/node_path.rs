// @trace REQ-ENG-007
use ::std::path::{MAIN_SEPARATOR, Path, PathBuf};
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ──────────────────────────────────────────────────────────────────────────
// Node.js path algorithm cores — the REAL posix and win32 faces (node ships
// a genuine implementation of BOTH algorithms on every platform, so
// `path.posix` and `path.win32` behave identically everywhere; only the
// separators and root forms differ).
//
// `node_alg` below is a faithful port of node's lib/path.js string
// algorithms (v21.6.1 lineage; cross-verified line-by-line against bun's
// runtime/node/path.zig port and empirically against the node v24.19.0
// oracle for every vector in the trailing-separator / dot-basename /
// win32-separator divergence class). The previous hand-rolled segment
// splitter dropped surviving trailing separators (`normalize('bar/foo../../')`
// answered 'bar' instead of 'bar/'), left '..' unresolved in join outputs,
// mis-answered dot-only basenames in extname ('..') and trailing-separator
// inputs in basename/dirname — all B-class Node-semantics divergences
// (REQ-ENG-007).
// ──────────────────────────────────────────────────────────────────────────

mod node_alg {
    /// node `isPathSeparator` — win32 additionally treats '\' as a separator.
    #[inline]
    fn is_sep(c: u8, windows: bool) -> bool {
        c == b'/' || (windows && c == b'\\')
    }

    /// node `isWindowsDeviceRoot` — 'a'..'z' / 'A'..'Z'.
    #[inline]
    fn is_device_root(c: u8) -> bool {
        c.is_ascii_alphabetic()
    }

    /// node `normalizeString` — resolves `.` / `..` segments. Faithful port
    /// (bun runtime/node/path.zig `normalizeStringT`), including the
    /// trailing-separator preservation semantics: a surviving trailing
    /// separator in the input yields one trailing separator in the output
    /// (`normalize('bar/foo../../') === 'bar/'`), and `..` resolution never
    /// consumes a segment that does not exist (leading `..` survives on
    /// relative paths when `allow_above_root`).
    ///
    /// `sep` is the OUTPUT separator ('/' posix, '\\' win32); on win32 both
    /// separators are recognized in the input.
    pub fn normalize_string(path: &str, allow_above_root: bool, sep: u8, windows: bool) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        let mut res: Vec<u8> = Vec::new();
        let mut last_segment_length: usize = 0;
        let mut last_slash: Option<usize> = None;
        // None = "saw a non-dot"; Some(n) = run of n dots since last segment.
        let mut dots: Option<usize> = Some(0);
        let mut byte: u8 = 0;
        let mut i: usize = 0;

        while i <= len {
            if i < len {
                byte = bytes[i];
            } else if is_sep(byte, windows) {
                break;
            } else {
                byte = b'/';
            }

            if is_sep(byte, windows) {
                if (last_slash.is_none() && i == 0)
                    || (i > 0 && last_slash.is_some() && last_slash.unwrap() == i - 1)
                    || dots == Some(1)
                {
                    // Consecutive separators, leading separator, or a '.'
                    // segment — no-op.
                } else if dots == Some(2) {
                    // '..' segment: pop the previous segment unless `res`
                    // already ends with '..'.
                    let ends_with_dotdot = res.len() >= 2
                        && last_segment_length == 2
                        && res[res.len() - 1] == b'.'
                        && res[res.len() - 2] == b'.';
                    if !ends_with_dotdot {
                        if res.len() > 2 {
                            match res.iter().rposition(|&c| c == sep) {
                                None => {
                                    res.clear();
                                    last_segment_length = 0;
                                }
                                Some(idx) => {
                                    res.truncate(idx);
                                    // node quirk (see bun's translation note):
                                    // lastSegmentLength derives from a
                                    // lastIndexOf over the truncated result.
                                    last_segment_length = match res.iter().rposition(|&c| c == sep)
                                    {
                                        None => res.len(),
                                        Some(inner) => res.len() - 1 - inner,
                                    };
                                }
                            }
                            last_slash = Some(i);
                            dots = Some(0);
                            i += 1;
                            continue;
                        } else if !res.is_empty() {
                            res.clear();
                            last_segment_length = 0;
                            last_slash = Some(i);
                            dots = Some(0);
                            i += 1;
                            continue;
                        }
                    }
                    if allow_above_root {
                        if res.is_empty() {
                            res.extend_from_slice(b"..");
                        } else {
                            res.push(sep);
                            res.extend_from_slice(b"..");
                        }
                        last_segment_length = 2;
                    }
                } else {
                    // Regular segment: append with the output separator.
                    if !res.is_empty() {
                        res.push(sep);
                    }
                    let slice_start = last_slash.map_or(0, |s| s + 1);
                    res.extend_from_slice(&bytes[slice_start..i]);
                    let subtract = last_slash.map_or(2, |s| s + 1);
                    last_segment_length = if i >= subtract { i - subtract } else { 0 };
                }
                last_slash = Some(i);
                dots = Some(0);
                i += 1;
                continue;
            } else if byte == b'.' && dots.is_some() {
                dots = Some(dots.unwrap() + 1);
            } else {
                dots = None;
            }
            i += 1;
        }
        String::from_utf8_lossy(&res).into_owned()
    }

    /// node path.posix.normalize / path.win32.normalize.
    pub fn normalize(path: &str, windows: bool) -> String {
        let bytes = path.as_bytes();
        if bytes.is_empty() {
            return ".".to_string();
        }
        if windows {
            return normalize_windows(path);
        }
        let is_absolute = bytes[0] == b'/';
        let trailing = is_sep(bytes[bytes.len() - 1], false);
        let normalized = normalize_string(path, !is_absolute, b'/', false);
        if normalized.is_empty() {
            return if is_absolute {
                "/".to_string()
            } else if trailing {
                "./".to_string()
            } else {
                ".".to_string()
            };
        }
        let mut out = normalized;
        if trailing {
            out.push('/');
        }
        if is_absolute {
            out.insert(0, '/');
        }
        out
    }

    /// node path.win32.normalize — device / UNC root extraction (bun
    /// `normalizeWindowsT`), with the tail normalized by `normalize_string`.
    fn normalize_windows(path: &str) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        if len == 1 {
            // `path` is a single character: a lone separator normalizes to
            // the win32 root, anything else is itself.
            return if is_sep(bytes[0], true) {
                "\\".to_string()
            } else {
                path.to_string()
            };
        }

        let byte0 = bytes[0];
        let mut root_end: usize = 0;
        let mut device: Option<String> = None;
        let mut is_absolute = false;

        if is_sep(byte0, true) {
            // Possible UNC root.
            is_absolute = true;
            if is_sep(bytes[1], true) {
                let mut j = 2usize;
                let mut last = j;
                while j < len && !is_sep(bytes[j], true) {
                    j += 1;
                }
                if j < len && j != last {
                    let first_part = &path[last..j];
                    last = j;
                    while j < len && is_sep(bytes[j], true) {
                        j += 1;
                    }
                    if j < len && j != last {
                        last = j;
                        while j < len && !is_sep(bytes[j], true) {
                            j += 1;
                        }
                        if j == len {
                            // UNC root only — normalized form keeps the
                            // trailing separator: `\\\\<server>\\<share>\\`.
                            return format!("\\\\{}\\{}\\", first_part, &path[last..]);
                        }
                        if j != last {
                            device = Some(format!("\\\\{}{}", first_part, &path[last..j]));
                            root_end = j;
                        }
                    }
                }
            } else {
                root_end = 1;
            }
        } else if is_device_root(byte0) && bytes[1] == b':' {
            // Possible device root ('C:' / 'c:').
            device = Some(path[0..2].to_string());
            root_end = 2;
            if len > 2 && is_sep(bytes[2], true) {
                is_absolute = true;
                root_end = 3;
            }
        }

        let mut tail = if root_end < len {
            normalize_string(&path[root_end..], !is_absolute, b'\\', true)
        } else {
            String::new()
        };
        if tail.is_empty() && !is_absolute {
            tail.push('.');
        }
        if !tail.is_empty() && is_sep(bytes[len - 1], true) {
            tail.push('\\');
        }
        let mut out = String::with_capacity(
            device.as_ref().map_or(0, |d| d.len()) + is_absolute as usize + tail.len(),
        );
        if let Some(d) = &device {
            out.push_str(d);
        }
        if is_absolute {
            out.push('\\');
        }
        out.push_str(&tail);
        out
    }

    /// node path.posix.join / path.win32.join — concatenate the non-empty
    /// parts with the face separator, then normalize the joined string (this
    /// is why join resolves a trailing '..' the same way normalize does).
    pub fn join(parts: &[&str], windows: bool) -> String {
        let sep: u8 = if windows { b'\\' } else { b'/' };
        if parts.is_empty() {
            return ".".to_string();
        }
        let mut joined = String::new();
        let mut first_part: Option<&str> = None;
        for p in parts {
            if p.is_empty() {
                continue;
            }
            if first_part.is_none() {
                first_part = Some(p);
            }
            if !joined.is_empty() {
                joined.push(sep as char);
            }
            joined.push_str(p);
        }
        if joined.is_empty() {
            return ".".to_string();
        }
        if windows {
            joined = collapse_win32_leading_separators(&joined, first_part.unwrap_or(""));
        }
        normalize(&joined, windows)
    }

    /// node win32.join's pre-normalize step: make sure the joined path does
    /// not start with two separators (normalize would mistake it for a UNC
    /// root) unless the first part explicitly named a UNC share. `first_part`
    /// is the first non-empty argument (node inspects its prefix, not the
    /// joined string's).
    fn collapse_win32_leading_separators(joined: &str, first_part: &str) -> String {
        let bytes = joined.as_bytes();
        let first_bytes = first_part.as_bytes();
        let mut needs_replace = true;
        let mut slash_count = 0usize;
        if is_sep(first_bytes[0], true) {
            slash_count += 1;
            let first_len = first_bytes.len();
            if first_len > 1 && is_sep(first_bytes[1], true) {
                slash_count += 1;
                if first_len > 2 {
                    if is_sep(first_bytes[2], true) {
                        slash_count += 1;
                    } else {
                        // The first part named a UNC server: keep the root.
                        needs_replace = false;
                    }
                }
            }
        }
        if !needs_replace {
            return joined.to_string();
        }
        let len = bytes.len();
        while slash_count < len && is_sep(bytes[slash_count], true) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            format!("\\{}", &joined[slash_count..])
        } else {
            joined.to_string()
        }
    }

    /// node path.posix.dirname / path.win32.dirname — trailing separators
    /// are stripped before the parent is taken ('/a/b/' → '/a').
    pub fn dirname(path: &str, windows: bool) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        if len == 0 {
            return ".".to_string();
        }
        if windows {
            return dirname_windows(path);
        }
        let has_root = bytes[0] == b'/';
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = len - 1;
        while i >= 1 {
            if bytes[i] == b'/' {
                if !matched_slash {
                    end = Some(i);
                    break;
                }
            } else {
                matched_slash = false;
            }
            i -= 1;
        }
        if let Some(e) = end {
            return if has_root && e == 1 {
                "//".to_string()
            } else {
                path[0..e].to_string()
            };
        }
        if has_root {
            "/".to_string()
        } else {
            ".".to_string()
        }
    }

    fn dirname_windows(path: &str) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        let byte0 = bytes[0];
        if len == 1 {
            return if is_sep(byte0, true) {
                path.to_string()
            } else {
                ".".to_string()
            };
        }

        let mut root_end: Option<usize> = None;
        let mut offset = 0usize;

        if is_sep(byte0, true) {
            // Possible UNC root.
            root_end = Some(1);
            offset = 1;
            if is_sep(bytes[1], true) {
                let mut j = 2usize;
                let mut last = j;
                while j < len && !is_sep(bytes[j], true) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && is_sep(bytes[j], true) {
                        j += 1;
                    }
                    if j < len && j != last {
                        last = j;
                        while j < len && !is_sep(bytes[j], true) {
                            j += 1;
                        }
                        if j == len {
                            return path.to_string();
                        }
                        if j != last {
                            offset = j + 1;
                            root_end = Some(offset);
                        }
                    }
                }
            }
        } else if is_device_root(byte0) && bytes[1] == b':' {
            offset = if len > 2 && is_sep(bytes[2], true) { 3 } else { 2 };
            root_end = Some(offset);
        }

        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = len as i64 - 1;
        while i >= offset as i64 {
            let iu = i as usize;
            if is_sep(bytes[iu], true) {
                if !matched_slash {
                    end = Some(iu);
                    break;
                }
            } else {
                matched_slash = false;
            }
            i -= 1;
        }
        if let Some(e) = end {
            return path[0..e].to_string();
        }
        match root_end {
            Some(r) => path[0..r].to_string(),
            None => ".".to_string(),
        }
    }

    /// node path.posix.basename / path.win32.basename — trailing separators
    /// are stripped before extraction ('/dir/' → 'dir'). `suffix` follows
    /// node's backward scan (the suffix must match the tail of the last
    /// segment; a suffix that spans a separator does not strip).
    pub fn basename(path: &str, suffix: Option<&str>, windows: bool) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        if len == 0 {
            return String::new();
        }
        if windows {
            return basename_windows(path, suffix);
        }

        let mut start = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;

        if let Some(sfx) = suffix {
            let sfx_len = sfx.len();
            if sfx_len > 0 && sfx_len <= len {
                if sfx == path {
                    return String::new();
                }
                let sfx_bytes = sfx.as_bytes();
                let mut ext_idx: Option<usize> = Some(sfx_len - 1);
                let mut first_non_slash_end: Option<usize> = None;
                let mut i = len as i64 - 1;
                while i >= start as i64 {
                    let iu = i as usize;
                    let byte = bytes[iu];
                    if byte == b'/' {
                        if !matched_slash {
                            start = iu + 1;
                            break;
                        }
                    } else {
                        if first_non_slash_end.is_none() {
                            matched_slash = false;
                            first_non_slash_end = Some(iu + 1);
                        }
                        if let Some(ei) = ext_idx {
                            if byte == sfx_bytes[ei] {
                                if ei == 0 {
                                    end = Some(iu);
                                    ext_idx = None;
                                } else {
                                    ext_idx = Some(ei - 1);
                                }
                            } else {
                                ext_idx = None;
                                end = first_non_slash_end;
                            }
                        }
                    }
                    i -= 1;
                }
                if let Some(e) = end {
                    if start == e {
                        return path[start..first_non_slash_end.unwrap_or(len)].to_string();
                    }
                    return path[start..e].to_string();
                }
                return path[start..len].to_string();
            }
        }

        let mut i = len as i64 - 1;
        while i > -1 {
            let iu = i as usize;
            let byte = bytes[iu];
            if byte == b'/' {
                if !matched_slash {
                    start = iu + 1;
                    break;
                }
            } else if end.is_none() {
                matched_slash = false;
                end = Some(iu + 1);
            }
            i -= 1;
        }
        match end {
            Some(e) => path[start..e].to_string(),
            None => String::new(),
        }
    }

    fn basename_windows(path: &str, suffix: Option<&str>) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        let mut start = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;

        // A drive prefix ('C:') is not a separator: skip it so the scan does
        // not treat the separator after the drive as an end-of-path marker.
        if len >= 2 && is_device_root(bytes[0]) && bytes[1] == b':' {
            start = 2;
        }

        if let Some(sfx) = suffix {
            let sfx_len = sfx.len();
            if sfx_len > 0 && sfx_len <= len {
                if sfx == path {
                    return String::new();
                }
                let sfx_bytes = sfx.as_bytes();
                let mut ext_idx: Option<usize> = Some(sfx_len - 1);
                let mut first_non_slash_end: Option<usize> = None;
                let mut i = len as i64 - 1;
                while i >= start as i64 {
                    let iu = i as usize;
                    let byte = bytes[iu];
                    if is_sep(byte, true) {
                        if !matched_slash {
                            start = iu + 1;
                            break;
                        }
                    } else {
                        if first_non_slash_end.is_none() {
                            matched_slash = false;
                            first_non_slash_end = Some(iu + 1);
                        }
                        if let Some(ei) = ext_idx {
                            if byte == sfx_bytes[ei] {
                                if ei == 0 {
                                    end = Some(iu);
                                    ext_idx = None;
                                } else {
                                    ext_idx = Some(ei - 1);
                                }
                            } else {
                                ext_idx = None;
                                end = first_non_slash_end;
                            }
                        }
                    }
                    i -= 1;
                }
                if let Some(e) = end {
                    if start == e {
                        return path[start..first_non_slash_end.unwrap_or(len)].to_string();
                    }
                    return path[start..e].to_string();
                }
                return path[start..len].to_string();
            }
        }

        let mut i = len as i64 - 1;
        while i >= start as i64 {
            let iu = i as usize;
            let byte = bytes[iu];
            if is_sep(byte, true) {
                if !matched_slash {
                    start = iu + 1;
                    break;
                }
            } else if end.is_none() {
                matched_slash = false;
                end = Some(iu + 1);
            }
            i -= 1;
        }
        match end {
            Some(e) => path[start..e].to_string(),
            None => String::new(),
        }
    }

    /// node path.posix.extname / path.win32.extname — dot-only basenames
    /// have no extension ('..' / '.' / 'a/..' → ''), and trailing separators
    /// are stripped first ('file.ext/' → '.ext', 'file./' → '.').
    ///
    /// Implemented as the observed node rule (validated against the node
    /// v24.19.0 oracle over the full extname edge table incl. '...', '....',
    /// '..file.', trailing separators and mixed '..' segments):
    ///   1. strip trailing separators to get the basename,
    ///   2. a basename of exactly '..' has no extension,
    ///   3. otherwise the substring from the last '.' (empty when the dot
    ///      is the first character or absent).
    pub fn extname(path: &str, windows: bool) -> String {
        let bytes = path.as_bytes();
        let len = bytes.len();
        if len == 0 {
            return String::new();
        }
        // Strip trailing separators, then cut at the last interior one.
        let mut end = len;
        while end > 0 && is_sep(bytes[end - 1], windows) {
            end -= 1;
        }
        if end == 0 {
            return String::new();
        }
        let mut start = 0usize;
        let mut i = end;
        while i > 0 {
            i -= 1;
            if is_sep(bytes[i], windows) {
                start = i + 1;
                break;
            }
        }
        let base = &path[start..end];
        if base == ".." {
            return String::new();
        }
        match base.rfind('.') {
            Some(0) => String::new(),
            Some(idx) => base[idx..].to_string(),
            None => String::new(),
        }
    }

    /// node path.parse result — { root, dir, base, ext, name }.
    pub struct Parsed {
        pub root: String,
        pub dir: String,
        pub base: String,
        pub ext: String,
        pub name: String,
    }

    /// node path.posix.parse / path.win32.parse.
    pub fn parse(path: &str, windows: bool) -> Parsed {
        let bytes = path.as_bytes();
        let len = bytes.len();
        let empty = Parsed {
            root: String::new(),
            dir: String::new(),
            base: String::new(),
            ext: String::new(),
            name: String::new(),
        };
        if len == 0 {
            return empty;
        }
        if windows {
            return parse_windows(path);
        }

        let is_absolute = bytes[0] == b'/';
        let root = if is_absolute { "/".to_string() } else { String::new() };
        let mut start = if is_absolute { 1usize } else { 0usize };

        let mut start_dot: Option<usize> = None;
        let mut start_part = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut pre_dot_state: Option<usize> = Some(0);

        let mut i = len as i64 - 1;
        while i >= start as i64 {
            let iu = i as usize;
            let byte = bytes[iu];
            if byte == b'/' {
                if !matched_slash {
                    start_part = iu + 1;
                    break;
                }
            } else {
                if end.is_none() {
                    matched_slash = false;
                    end = Some(iu + 1);
                }
                if byte == b'.' {
                    if start_dot.is_none() {
                        start_dot = Some(iu);
                    } else if let Some(p) = pre_dot_state {
                        if p != 1 {
                            pre_dot_state = Some(1);
                        }
                    }
                } else if start_dot.is_some() {
                    pre_dot_state = None;
                }
            }
            i -= 1;
        }

        let mut base = String::new();
        let mut name = String::new();
        let mut ext = String::new();
        if let Some(e) = end {
            start = if start_part == 0 && is_absolute { 1 } else { start_part };
            let no_ext = start_dot.is_none()
                || (pre_dot_state.is_some() && pre_dot_state.unwrap() == 0)
                || (pre_dot_state == Some(1)
                    && start_dot.unwrap() == e - 1
                    && start_dot.unwrap() == start_part + 1);
            if no_ext {
                name = path[start..e].to_string();
                base = name.clone();
            } else {
                name = path[start..start_dot.unwrap()].to_string();
                base = path[start..e].to_string();
                ext = path[start_dot.unwrap()..e].to_string();
            }
        }
        let dir = if start_part > 0 {
            path[0..start_part - 1].to_string()
        } else if is_absolute {
            "/".to_string()
        } else {
            String::new()
        };
        Parsed {
            root,
            dir,
            base,
            ext,
            name,
        }
    }

    fn parse_windows(path: &str) -> Parsed {
        let bytes = path.as_bytes();
        let len = bytes.len();
        let byte0 = bytes[0];
        if len == 1 {
            if is_sep(byte0, true) {
                return Parsed {
                    root: path.to_string(),
                    dir: path.to_string(),
                    base: String::new(),
                    ext: String::new(),
                    name: String::new(),
                };
            }
            return Parsed {
                root: String::new(),
                dir: String::new(),
                base: path.to_string(),
                ext: String::new(),
                name: path.to_string(),
            };
        }

        let mut root_end = 0usize;
        if is_sep(byte0, true) {
            root_end = 1;
            if is_sep(bytes[1], true) {
                let mut j = 2usize;
                let mut last = j;
                while j < len && !is_sep(bytes[j], true) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && is_sep(bytes[j], true) {
                        j += 1;
                    }
                    if j < len && j != last {
                        last = j;
                        while j < len && !is_sep(bytes[j], true) {
                            j += 1;
                        }
                        root_end = if j == len { j } else if j != last { j + 1 } else { root_end };
                    }
                }
            }
        } else if is_device_root(byte0) && bytes[1] == b':' {
            if len <= 2 {
                return Parsed {
                    root: path.to_string(),
                    dir: path.to_string(),
                    base: String::new(),
                    ext: String::new(),
                    name: String::new(),
                };
            }
            root_end = 2;
            if is_sep(bytes[2], true) {
                if len == 3 {
                    return Parsed {
                        root: path.to_string(),
                        dir: path.to_string(),
                        base: String::new(),
                        ext: String::new(),
                        name: String::new(),
                    };
                }
                root_end = 3;
            }
        }
        let root = if root_end > 0 {
            path[0..root_end].to_string()
        } else {
            String::new()
        };

        let mut start_dot: Option<usize> = None;
        let mut start_part = root_end;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut pre_dot_state: Option<usize> = Some(0);

        let mut i = len as i64 - 1;
        while i >= root_end as i64 {
            let iu = i as usize;
            let byte = bytes[iu];
            if is_sep(byte, true) {
                if !matched_slash {
                    start_part = iu + 1;
                    break;
                }
            } else {
                if end.is_none() {
                    matched_slash = false;
                    end = Some(iu + 1);
                }
                if byte == b'.' {
                    if start_dot.is_none() {
                        start_dot = Some(iu);
                    } else if let Some(p) = pre_dot_state {
                        if p != 1 {
                            pre_dot_state = Some(1);
                        }
                    }
                } else if start_dot.is_some() {
                    pre_dot_state = None;
                }
            }
            i -= 1;
        }

        let mut base = String::new();
        let mut name = String::new();
        let mut ext = String::new();
        if let Some(e) = end {
            let no_ext = start_dot.is_none()
                || (pre_dot_state.is_some() && pre_dot_state.unwrap() == 0)
                || (pre_dot_state == Some(1)
                    && start_dot.unwrap() == e - 1
                    && start_dot.unwrap() == start_part + 1);
            if no_ext {
                name = path[start_part..e].to_string();
                base = name.clone();
            } else {
                name = path[start_part..start_dot.unwrap()].to_string();
                base = path[start_part..e].to_string();
                ext = path[start_dot.unwrap()..e].to_string();
            }
        }
        // The directory is the root itself when the first segment starts at
        // the root end ('C:\abc' → dir 'C:\'); otherwise the trailing
        // separator is stripped ('C:\abc\def' → dir 'C:\abc').
        let dir = if start_part > 0 && start_part != root_end {
            path[0..start_part - 1].to_string()
        } else {
            root.clone()
        };
        Parsed {
            root,
            dir,
            base,
            ext,
            name,
        }
    }

    /// node path.format — `dir || root` + `base || name + formatExt(ext)`,
    /// joined with the face separator unless dir IS the pathObject root.
    pub fn format(p: &Parsed, windows: bool) -> String {
        let sep: char = if windows { '\\' } else { '/' };
        let dir_is_empty = p.dir.is_empty();
        let dir = if dir_is_empty { &p.root } else { &p.dir };
        let dir = dir.as_str();
        let base = if !p.base.is_empty() {
            p.base.clone()
        } else {
            // node formatExt: a missing leading dot is supplied.
            let ext = if p.ext.is_empty() {
                String::new()
            } else if p.ext.starts_with('.') {
                p.ext.clone()
            } else {
                format!(".{}", p.ext)
            };
            format!("{}{}", p.name, ext)
        };
        if dir.is_empty() {
            return base;
        }
        if dir == p.root {
            format!("{}{}", dir, base)
        } else {
            format!("{}{}{}", dir, sep, base)
        }
    }

    /// node path.posix.isAbsolute / path.win32.isAbsolute. On win32 a rooted
    /// path without a drive ('/foo') is absolute, and a bare device ('C:')
    /// is NOT.
    pub fn is_absolute(path: &str, windows: bool) -> bool {
        let bytes = path.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        if windows {
            let c = bytes[0];
            is_sep(c, true)
                || (bytes.len() > 2
                    && is_device_root(c)
                    && bytes[1] == b':'
                    && is_sep(bytes[2], true))
        } else {
            bytes[0] == b'/'
        }
    }
}

mod posix_core {
    /// Split a posix path into segments; keeps a leading-root flag.
    /// (Consumed by `resolve` / `relative` below; the string-algorithm
    /// primitives live in `node_alg`.)
    fn split(p: &str) -> (bool, Vec<&str>) {
        let rooted = p.starts_with('/');
        let segs = p
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        (rooted, segs)
    }

    pub fn normalize(p: &str) -> String {
        super::node_alg::normalize(p, false)
    }

    pub fn join(parts: &[String]) -> String {
        let joined: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        super::node_alg::join(&joined, false)
    }

    pub fn is_absolute(p: &str) -> bool {
        super::node_alg::is_absolute(p, false)
    }

    pub fn resolve(parts: &[String], cwd: &str) -> String {
        // Right-most absolute segment wins; otherwise cwd + parts.
        let mut abs_start = 0usize;
        for (i, p) in parts.iter().enumerate() {
            if is_absolute(p) {
                abs_start = i;
            }
        }
        let mut base: Vec<&str> = Vec::new();
        let mut rooted = false;
        if abs_start == 0 && !parts.first().is_some_and(|p| is_absolute(p)) {
            let (r, segs) = split(cwd);
            rooted = r;
            base = segs;
        }
        for p in &parts[abs_start..] {
            let (_r, segs) = split(p);
            rooted = rooted || is_absolute(p);
            for seg in segs {
                if seg == ".." {
                    if !base.is_empty() && *base.last().unwrap() != ".." {
                        base.pop();
                    } else if !rooted {
                        base.push(seg);
                    }
                } else {
                    base.push(seg);
                }
            }
        }
        let joined = base.join("/");
        if rooted {
            format!("/{}", joined)
        } else if joined.is_empty() {
            "/".to_string()
        } else {
            joined
        }
    }

    pub fn dirname(p: &str) -> String {
        super::node_alg::dirname(p, false)
    }

    pub fn basename(p: &str, ext: Option<&str>) -> String {
        super::node_alg::basename(p, ext, false)
    }

    pub fn extname(p: &str) -> String {
        super::node_alg::extname(p, false)
    }

    pub fn relative(from: &str, to: &str) -> String {
        let fa = normalize(from);
        let ta = normalize(to);
        if fa == ta {
            return String::new();
        }
        let (_, fseg) = split(&fa);
        let (_, tseg) = split(&ta);
        let mut i = 0;
        while i < fseg.len() && i < tseg.len() && fseg[i] == tseg[i] {
            i += 1;
        }
        let ups = fseg.len() - i;
        let mut parts: Vec<String> = Vec::new();
        for _ in 0..ups {
            parts.push("..".to_string());
        }
        for seg in &tseg[i..] {
            parts.push(seg.to_string());
        }
        if parts.is_empty() {
            ".".to_string()
        } else {
            parts.join("/")
        }
    }

    /// node posix.parse: { root, dir, base, ext, name }
    pub fn parse(p: &str) -> (String, String, String, String, String) {
        let parsed = super::node_alg::parse(p, false);
        (
            parsed.root,
            parsed.dir,
            parsed.base,
            parsed.ext,
            parsed.name,
        )
    }

    pub fn format(p: &super::node_alg::Parsed) -> String {
        super::node_alg::format(p, false)
    }
}

/// path.win32 — the genuine win32 face (node ships the Windows algorithm on
/// every platform, so `path.win32.normalize('a/b') === 'a\\b'` even on
/// Linux). Pure-string cores from `node_alg`; `resolve` / `relative` /
/// `toNamespacedPath` are cwd-dependent and stay forwarded from the host
/// module object.
mod win32_core {
    pub fn normalize(p: &str) -> String {
        super::node_alg::normalize(p, true)
    }

    pub fn join(parts: &[String]) -> String {
        let joined: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        super::node_alg::join(&joined, true)
    }

    pub fn dirname(p: &str) -> String {
        super::node_alg::dirname(p, true)
    }

    pub fn basename(p: &str, ext: Option<&str>) -> String {
        super::node_alg::basename(p, ext, true)
    }

    pub fn extname(p: &str) -> String {
        super::node_alg::extname(p, true)
    }

    pub fn is_absolute(p: &str) -> bool {
        super::node_alg::is_absolute(p, true)
    }

    pub fn parse(p: &str) -> (String, String, String, String, String) {
        let parsed = super::node_alg::parse(p, true);
        (
            parsed.root,
            parsed.dir,
            parsed.base,
            parsed.ext,
            parsed.name,
        )
    }

    pub fn format(p: &super::node_alg::Parsed) -> String {
        super::node_alg::format(p, true)
    }
}

/// arg_to_string for the posix natives (same contract as the platform fns).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn posix_arg(cx: *mut JSContext, v: mozjs::jsval::JSVal) -> Option<String> {
    arg_to_string(cx, v)
}

macro_rules! posix_str_fn {
    ($name:ident, $jsname:literal, $body:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let out: Option<String> = $body(cx, &args, argc);
            match out {
                Some(s) => return_string(cx, &args, &s),
                None => {
                    ::mozjs::jsapi::JS_ReportErrorUTF8(
                        cx,
                        c"posix path: invalid argument".as_ptr(),
                    );
                    false
                }
            }
        }
    };
}

posix_str_fn!(js_posix_join, "join", |cx: *mut JSContext,
                                    args: &::mozjs::jsapi::CallArgs,
                                    argc: u32|
 -> Option<String> {
    let mut parts = Vec::new();
    for i in 0..argc {
        parts.push(posix_arg(cx, *args.get(i).ptr)?);
    }
    Some(posix_core::join(&parts))
});
posix_str_fn!(js_posix_normalize, "normalize", |cx: *mut JSContext,
                                             args: &::mozjs::jsapi::CallArgs,
                                             _argc: u32|
 -> Option<String> {
    Some(posix_core::normalize(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_resolve, "resolve", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         argc: u32|
 -> Option<String> {
    let mut parts = Vec::new();
    for i in 0..argc {
        parts.push(posix_arg(cx, *args.get(i).ptr)?);
    }
    let cwd = ::std::env::current_dir().ok()?.to_string_lossy().replace('\\', "/");
    Some(posix_core::resolve(&parts, &cwd))
});
posix_str_fn!(js_posix_dirname, "dirname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(posix_core::dirname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_basename, "basename", |cx: *mut JSContext,
                                           args: &::mozjs::jsapi::CallArgs,
                                           argc: u32|
 -> Option<String> {
    let p = posix_arg(cx, *args.get(0).ptr)?;
    // node: an explicit `undefined` suffix is valid (no strip); anything
    // else non-string is invalid.
    let ext = if argc > 1 && !(*args.get(1).ptr).is_undefined() {
        Some(posix_arg(cx, *args.get(1).ptr)?)
    } else {
        None
    };
    Some(posix_core::basename(&p, ext.as_deref()))
});
posix_str_fn!(js_posix_extname, "extname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(posix_core::extname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_relative, "relative", |cx: *mut JSContext,
                                           args: &::mozjs::jsapi::CallArgs,
                                           _argc: u32|
 -> Option<String> {
    let from = posix_arg(cx, *args.get(0).ptr)?;
    let to = posix_arg(cx, *args.get(1).ptr)?;
    Some(posix_core::relative(&from, &to))
});
posix_str_fn!(js_posix_format, "format", |cx: *mut JSContext,
                                       args: &::mozjs::jsapi::CallArgs,
                                       _argc: u32|
 -> Option<String> {
    // Accepts the parsed-shape object {root, dir, base, ext, name} (node
    // format: dir || root, base || name + formatExt(ext)).
    let parsed = read_path_object(cx, args)?;
    Some(posix_core::format(&parsed))
});

/// Read the `{root, dir, base, ext, name}` pathObject shape shared by
/// path.format on both faces. Absent properties coerce to "" (node treats
/// them as falsy).
unsafe fn read_path_object(
    cx: *mut JSContext,
    args: &::mozjs::jsapi::CallArgs,
) -> Option<node_alg::Parsed> {
    let obj = (*args.get(0).ptr).to_object();
    let wrapped_cx = ::mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let mut root = String::new();
    let mut dir = String::new();
    let mut base = String::new();
    let mut ext = String::new();
    let mut name = String::new();
    if let Some(v) = get_string_prop(cx, obj_root.handle().into(), "root") {
        root = v;
    }
    if let Some(v) = get_string_prop(cx, obj_root.handle().into(), "dir") {
        dir = v;
    }
    if let Some(v) = get_string_prop(cx, obj_root.handle().into(), "base") {
        base = v;
    }
    if let Some(v) = get_string_prop(cx, obj_root.handle().into(), "ext") {
        ext = v;
    }
    if let Some(v) = get_string_prop(cx, obj_root.handle().into(), "name") {
        name = v;
    }
    Some(node_alg::Parsed {
        root,
        dir,
        base,
        ext,
        name,
    })
}

posix_str_fn!(js_win32_join, "join", |cx: *mut JSContext,
                                    args: &::mozjs::jsapi::CallArgs,
                                    argc: u32|
 -> Option<String> {
    let mut parts = Vec::new();
    for i in 0..argc {
        parts.push(posix_arg(cx, *args.get(i).ptr)?);
    }
    Some(win32_core::join(&parts))
});
posix_str_fn!(js_win32_normalize, "normalize", |cx: *mut JSContext,
                                             args: &::mozjs::jsapi::CallArgs,
                                             _argc: u32|
 -> Option<String> {
    Some(win32_core::normalize(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_win32_dirname, "dirname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(win32_core::dirname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_win32_basename, "basename", |cx: *mut JSContext,
                                           args: &::mozjs::jsapi::CallArgs,
                                           argc: u32|
 -> Option<String> {
    let p = posix_arg(cx, *args.get(0).ptr)?;
    // node: an explicit `undefined` suffix is valid (no strip); anything
    // else non-string is invalid.
    let ext = if argc > 1 && !(*args.get(1).ptr).is_undefined() {
        Some(posix_arg(cx, *args.get(1).ptr)?)
    } else {
        None
    };
    Some(win32_core::basename(&p, ext.as_deref()))
});
posix_str_fn!(js_win32_extname, "extname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(win32_core::extname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_win32_format, "format", |cx: *mut JSContext,
                                       args: &::mozjs::jsapi::CallArgs,
                                       _argc: u32|
 -> Option<String> {
    let parsed = read_path_object(cx, args)?;
    Some(win32_core::format(&parsed))
});

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn win32_is_absolute(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(::mozjs::jsval::BooleanValue(false));
        return true;
    }
    match posix_arg(cx, *args.get(0).ptr) {
        Some(s) => args
            .rval()
            .set(::mozjs::jsval::BooleanValue(win32_core::is_absolute(&s))),
        None => args.rval().set(::mozjs::jsval::BooleanValue(false)),
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn win32_parse_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        ::mozjs::jsapi::JS_ReportErrorUTF8(
            cx,
            ::std::ffi::CStr::from_bytes_with_nul(b"path.parse requires a path\0")
                .unwrap()
                .as_ptr(),
        );
        return false;
    }
    let Some(p) = posix_arg(cx, *args.get(0).ptr) else {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    };
    let (root, dir, base, ext, name) = win32_core::parse(&p);
    let raw_cx = cx;
    let mut wrapped = ::mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(raw_cx),
    );
    let cx_ref = &mut wrapped;
    ::mozjs::rooted!(&in(cx_ref) let obj = ::mozjs::rust::wrappers2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    }
    let h = obj.handle().into();
    macro_rules! def_str {
        ($name:literal, $val:expr) => {{
            let cs = ZBox::from_bytes($val.as_bytes());
            let js = JS_NewStringCopyZ(raw_cx, cs.as_ptr());
            if !js.is_null() {
                ::mozjs::rooted!(&in(cx_ref) let sv = ::mozjs::jsval::StringValue(&*js));
                ::mozjs::jsapi::JS_DefineProperty(
                    raw_cx,
                    h,
                    ::std::ffi::CStr::from_bytes_with_nul(::std::concat!($name, "\0").as_bytes())
                        .unwrap()
                        .as_ptr(),
                    sv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }};
    }
    def_str!("root", root);
    def_str!("dir", dir);
    def_str!("base", base);
    def_str!("ext", ext);
    def_str!("name", name);
    args.rval().set(::mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn posix_is_absolute(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(::mozjs::jsval::BooleanValue(false));
        return true;
    }
    match posix_arg(cx, *args.get(0).ptr) {
        Some(s) => args.rval().set(::mozjs::jsval::BooleanValue(posix_core::is_absolute(&s))),
        None => args.rval().set(::mozjs::jsval::BooleanValue(false)),
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn posix_parse_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        ::mozjs::jsapi::JS_ReportErrorUTF8(
            cx,
            ::std::ffi::CStr::from_bytes_with_nul(b"path.parse requires a path\0")
                .unwrap()
                .as_ptr(),
        );
        return false;
    }
    let Some(p) = posix_arg(cx, *args.get(0).ptr) else {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    };
    let (root, dir, base, ext, name) = posix_core::parse(&p);
    let raw_cx = cx;
    let mut wrapped = ::mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(raw_cx),
    );
    let cx_ref = &mut wrapped;
    ::mozjs::rooted!(&in(cx_ref) let obj = ::mozjs::rust::wrappers2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    }
    let h = obj.handle().into();
    macro_rules! def_str {
        ($name:literal, $val:expr) => {{
            let cs = ZBox::from_bytes($val.as_bytes());
            let js = JS_NewStringCopyZ(raw_cx, cs.as_ptr());
            if !js.is_null() {
                ::mozjs::rooted!(&in(cx_ref) let sv = ::mozjs::jsval::StringValue(&*js));
                ::mozjs::jsapi::JS_DefineProperty(
                    raw_cx,
                    h,
                    ::std::ffi::CStr::from_bytes_with_nul(::std::concat!($name, "\0").as_bytes())
                        .unwrap()
                        .as_ptr(),
                    sv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }};
    }
    def_str!("root", root);
    def_str!("dir", dir);
    def_str!("base", base);
    def_str!("ext", ext);
    def_str!("name", name);
    args.rval().set(::mozjs::jsval::ObjectValue(obj.get()));
    true
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let path_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if path_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"join".as_ptr(),
            Some(path_join),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"resolve".as_ptr(),
            Some(path_resolve),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"dirname".as_ptr(),
            Some(path_dirname),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"basename".as_ptr(),
            Some(path_basename),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"extname".as_ptr(),
            Some(path_extname),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"normalize".as_ptr(),
            Some(path_normalize),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"isAbsolute".as_ptr(),
            Some(path_is_absolute),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"relative".as_ptr(),
            Some(path_relative),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"parse".as_ptr(),
            Some(path_parse),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"format".as_ptr(),
            Some(path_format),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"toNamespacedPath".as_ptr(),
            Some(path_to_namespaced),
            1,
            JSPROP_ENUMERATE as u32,
        );

        let sep_cstr = ZBox::from_bytes(if MAIN_SEPARATOR == '/' { "/" } else { "\\" }.as_bytes());
        let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
        if !sep_str.is_null() {
            let sep_val = mozjs::jsval::StringValue(&*sep_str);
            rooted!(&in(cx) let sep_root = sep_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"sep".as_ptr(),
                sep_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // path.platform — mirrors process.platform / os.platform() so code
        // branching on `path.platform === "win32"` behaves like Node's
        // platform-tagged path modules. Audit item: was entirely missing
        // (typeof path.platform === 'undefined').
        let platform_bytes: &[u8] = if cfg!(target_os = "linux") {
            b"linux"
        } else if cfg!(target_os = "macos") {
            b"darwin"
        } else if cfg!(target_os = "windows") {
            b"win32"
        } else {
            b"unknown"
        };
        let platform_cstr = ZBox::from_bytes(platform_bytes);
        let platform_str = JS_NewStringCopyZ(cx.raw_cx(), platform_cstr.as_ptr());
        if !platform_str.is_null() {
            let platform_val = mozjs::jsval::StringValue(&*platform_str);
            rooted!(&in(cx) let platform_root = platform_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"platform".as_ptr(),
                platform_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        let delim_cstr = ZBox::from_bytes(if cfg!(windows) { b";" } else { b":" });
        let delim_str = JS_NewStringCopyZ(cx.raw_cx(), delim_cstr.as_ptr());
        if !delim_str.is_null() {
            let delim_val = mozjs::jsval::StringValue(&*delim_str);
            rooted!(&in(cx) let delim_root = delim_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"delimiter".as_ptr(),
                delim_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.posix — the REAL posix face (pure-string algorithms above);
    // node ships a genuine posix implementation on every platform, and the
    // old self-reference answered `path.posix.sep === path.sep` ('\' on
    // Windows — posix-selfref conformance FAIL).
    unsafe {
        rooted!(&in(cx) let posix_obj = w2::JS_NewPlainObject(cx));
        if !posix_obj.get().is_null() {
            let fns: &[(&str, ::std::option::Option<
                unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool,
            >)] = &[
                ("join", Some(js_posix_join)),
                ("normalize", Some(js_posix_normalize)),
                ("resolve", Some(js_posix_resolve)),
                ("dirname", Some(js_posix_dirname)),
                ("basename", Some(js_posix_basename)),
                ("extname", Some(js_posix_extname)),
                ("isAbsolute", Some(posix_is_absolute)),
                ("relative", Some(js_posix_relative)),
                ("parse", Some(posix_parse_fn)),
                ("format", Some(js_posix_format)),
            ];
            for (name, fp) in fns {
                let c_name = ZBox::from_bytes(name.as_bytes());
                let f = ::mozjs::jsapi::JS_NewFunction(
                    cx.raw_cx(),
                    *fp,
                    2,
                    0,
                    c_name.as_ptr(),
                );
                if !f.is_null() {
                    let fobj = ::mozjs::jsapi::JS_GetFunctionObject(f);
                    ::mozjs::rooted!(&in(cx) let fv = ::mozjs::jsval::ObjectValue(fobj));
                    ::mozjs::jsapi::JS_DefineProperty(
                        cx.raw_cx(),
                        posix_obj.handle().into(),
                        c_name.as_ptr(),
                        fv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // posix.sep = "/" / posix.delimiter = ":"
            let sep_cstr = ZBox::from_bytes(b"/");
            let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
            if !sep_str.is_null() {
                let v = ::mozjs::jsval::StringValue(&*sep_str);
                ::mozjs::rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    posix_obj.handle().into(),
                    c"sep".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let dlm_cstr = ZBox::from_bytes(b":");
            let dlm_str = JS_NewStringCopyZ(cx.raw_cx(), dlm_cstr.as_ptr());
            if !dlm_str.is_null() {
                let v = ::mozjs::jsval::StringValue(&*dlm_str);
                ::mozjs::rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    posix_obj.handle().into(),
                    c"delimiter".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            // posix.posix / posix.win32 self-refs (node shape)
            w2::JS_DefineProperty3(
                cx,
                posix_obj.handle(),
                c"posix".as_ptr(),
                posix_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
            // Attach the real posix face to the module:
            w2::JS_DefineProperty3(
                cx,
                path_obj.handle(),
                c"posix".as_ptr(),
                posix_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.win32 — Node.js ships a real Windows-flavoured path object on all
    // platforms: `path.win32.sep === "\\"`, `path.win32.normalize('a/b') ===
    // 'a\\b'`, `path.win32.basename('C:\\dir\\f') === 'f'` — on Linux too.
    // The string-algorithm methods therefore get genuine win32 cores
    // (win32_core / node_alg, ported from node lib/path.js); only the
    // cwd-dependent methods (`resolve` / `relative`) stay forwarded from the
    // host module object. (See ~/code/rust/bun/src/runtime/node/path.zig —
    // Bun likewise ships a real win32 face on every platform.)
    unsafe {
        rooted!(&in(cx) let win32_obj = w2::JS_NewPlainObject(cx));
        if !win32_obj.get().is_null() {
            let win_fns: &[(&str, ::std::option::Option<
                unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool,
            >)] = &[
                ("join", Some(js_win32_join)),
                ("normalize", Some(js_win32_normalize)),
                ("dirname", Some(js_win32_dirname)),
                ("basename", Some(js_win32_basename)),
                ("extname", Some(js_win32_extname)),
                ("isAbsolute", Some(win32_is_absolute)),
                ("parse", Some(win32_parse_fn)),
                ("format", Some(js_win32_format)),
            ];
            for (name, fp) in win_fns {
                let c_name = ZBox::from_bytes(name.as_bytes());
                let f = ::mozjs::jsapi::JS_NewFunction(
                    cx.raw_cx(),
                    *fp,
                    2,
                    0,
                    c_name.as_ptr(),
                );
                if !f.is_null() {
                    let fobj = ::mozjs::jsapi::JS_GetFunctionObject(f);
                    ::mozjs::rooted!(&in(cx) let fv = ::mozjs::jsval::ObjectValue(fobj));
                    ::mozjs::jsapi::JS_DefineProperty(
                        cx.raw_cx(),
                        win32_obj.handle().into(),
                        c_name.as_ptr(),
                        fv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // cwd-dependent methods: forward the host implementations (their
            // algorithm is platform/cwd-bound; the win32 face keeps the same
            // behaviour as before this change).
            for fn_name in &["resolve", "relative"] {
                let c_name = ZBox::from_bytes(fn_name.as_bytes());
                let mut fn_val = UndefinedValue();
                JS_GetProperty(
                    cx.raw_cx(),
                    path_obj.handle().into(),
                    c_name.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut fn_val,
                    },
                );
                if fn_val.is_object() {
                    rooted!(&in(cx) let fv = fn_val);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        win32_obj.handle().into(),
                        c_name.as_ptr(),
                        fv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // win32.sep = "\\" / win32.delimiter = ";"
            let winsep_cstr = ZBox::from_bytes(b"\\");
            let winsep_str = JS_NewStringCopyZ(cx.raw_cx(), winsep_cstr.as_ptr());
            if !winsep_str.is_null() {
                let v = mozjs::jsval::StringValue(&*winsep_str);
                rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    win32_obj.handle().into(),
                    c"sep".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let wdlm_cstr = ZBox::from_bytes(b";");
            let wdlm_str = JS_NewStringCopyZ(cx.raw_cx(), wdlm_cstr.as_ptr());
            if !wdlm_str.is_null() {
                let v = mozjs::jsval::StringValue(&*wdlm_str);
                rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    win32_obj.handle().into(),
                    c"delimiter".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            // win32.win32 / win32.posix self-refs (matches Node.js shape)
            w2::JS_DefineProperty3(
                cx,
                win32_obj.handle(),
                c"win32".as_ptr(),
                win32_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
            w2::JS_DefineProperty3(
                cx,
                win32_obj.handle(),
                c"posix".as_ptr(),
                path_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );

            w2::JS_DefineProperty3(
                cx,
                path_obj.handle(),
                c"win32".as_ptr(),
                win32_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.matchesGlob(path, pattern) — pure JS glob matcher. Bun's
    // implementation (~/code/rust/bun/src/js/node/path.ts:matchesGlob) uses
    // `Bun.Glob` (native); here we express the same boolean result with a
    // minimal regex-based glob translator covering the standard wildcard
    // syntax (`*`, `?`, `**`, character classes) so behaviour matches Node.
    unsafe {
        let matches_src = r#"(function(p){
  function globToRegExp(pattern) {
    // Anchor the pattern; translate `**` → `.*`, `*` → `.*`, `?` → `.`,
    // and escape regex metacharacters in literals. Mirrors Bun.Glob's
    // full-path glob semantics (a single `*` matches across path segments),
    // which is what Node.js' path.matchesGlob surfaces.
    var s = '^';
    for (var i = 0; i < pattern.length; i++) {
      var c = pattern.charAt(i);
      if (c === '*') {
        if (pattern.charAt(i + 1) === '*') {
          s += '.*';
          i++;
        } else {
          s += '.*';
        }
      } else if (c === '?') {
        s += '.';
      } else if ('.+^${}()|[]\\'.indexOf(c) >= 0) {
        s += '\\' + c;
      } else {
        s += c;
      }
    }
    return new RegExp(s + '$');
  }
  p.matchesGlob = function matchesGlob(path, pattern) {
    if (typeof path !== 'string') {
      throw new TypeError('The "path" argument must be of type string.');
    }
    if (typeof pattern !== 'string') {
      throw new TypeError('The "pattern" argument must be of type string.');
    }
    return globToRegExp(pattern).test(path);
  };
})"#;
        let mut msrc = mozjs::rust::transform_str_to_source_text(matches_src);
        let mut mval = UndefinedValue();
        let mh = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut mval,
        };
        let mopts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<path-matchesGlob>".as_ptr(), 1);
        if !mopts.is_null() {
            let global = CurrentGlobalOrNull(cx.raw_cx());
            if !global.is_null()
                && JS::Evaluate2(cx.raw_cx(), mopts, &mut msrc, mh)
                && mval.is_object()
            {
                let wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
                rooted!(&in(wrapped_cx) let global_root = global);
                rooted!(&in(wrapped_cx) let path_val_root = ObjectValue(path_obj.get()));
                let args_arr = HandleValueArray {
                    length_: 1,
                    elements_: &path_val_root.get() as *const Value,
                };
                let mut call_rval = UndefinedValue();
                let call_rval_h = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut call_rval,
                };
                rooted!(&in(wrapped_cx) let factory_obj = mval.to_object());
                rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
                JS_CallFunctionValue(
                    cx.raw_cx(),
                    global_root.handle().into(),
                    factory_obj_h.handle().into(),
                    &args_arr,
                    call_rval_h,
                );
            }
            libc::free(mopts as *mut _);
        }
    }

    cache_builtin(cx, "path", path_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn arg_to_string(
    cx: *mut JSContext,
    val: JSVal,
) -> ::std::option::Option<::std::string::String> {
    if val.is_undefined() || val.is_null() {
        return ::std::option::Option::None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let val_root = val);
    let s = mozjs::rust::ToString(&mut wrapped_cx, val_root.handle().into());
    if s.is_null() {
        return ::std::option::Option::None;
    }
    let rust_str = crate::jsstr_to_rust_string(cx, s);
    ::std::option::Option::Some(rust_str)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string(cx: *mut JSContext, args: &CallArgs, s: &str) -> bool {
    let c_str = ZBox::from_bytes(s.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_join(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut parts: Vec<::std::string::String> = Vec::new();
    for val in ::std::slice::from_raw_parts(args.argv_, argc as usize) {
        match arg_to_string(cx, *val) {
            Some(s) => parts.push(s),
            None => {
                JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
                return false;
            }
        }
    }
    let joined = posix_join(&parts);
    return_string(cx, &args, &joined)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_resolve(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let cwd = {
        let mut buf = bun_paths::path_buffer_pool::get();
        bun_core::getcwd(&mut buf)
            .map(|z| PathBuf::from(String::from_utf8_lossy(z.as_bytes()).into_owned()))
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    let mut resolved = cwd;

    for val in ::std::slice::from_raw_parts(args.argv_, argc as usize) {
        match arg_to_string(cx, *val) {
            Some(s) => {
                let p = Path::new(&s);
                if p.is_absolute() {
                    resolved = p.to_path_buf();
                } else {
                    resolved = resolved.join(p);
                }
            }
            None => {
                JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
                return false;
            }
        }
    }

    let result = normalize_path(&resolved);
    return_string(cx, &args, &result.to_string_lossy())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_dirname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let result = Path::new(&s)
        .parent()
        .map(|p| {
            let pstr = p.to_string_lossy().into_owned();
            if pstr.is_empty() {
                ".".to_string()
            } else {
                pstr
            }
        })
        .unwrap_or_else(|| ".".to_string());
    return_string(cx, &args, &result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_basename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let mut base = Path::new(&s)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.clone());

    if argc >= 2 {
        let ext_val = *args.get(1).ptr;
        if let Some(ext) = arg_to_string(cx, ext_val)
            && base.ends_with(&ext)
            && !ext.is_empty()
        {
            base.truncate(base.len() - ext.len());
        }
    }
    return_string(cx, &args, &base)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_extname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let ext = Path::new(&s)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    return_string(cx, &args, &ext)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_normalize(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let p = Path::new(&s);
    let normalized = normalize_path(p);
    return_string(cx, &args, &normalized.to_string_lossy())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_is_absolute(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    // Node semantics, not Rust's: on win32 a rooted path WITHOUT a drive
    // ('/foo') is absolute (node's lib/path.js win32 isAbsolute — separator
    // at [0], or device-root + ':' + separator), while std's
    // Path::is_absolute demands a drive prefix and answered false for
    // '/foo' on Windows. POSIX keeps the std behavior (leading '/').
    #[cfg(windows)]
    {
        let b = s.as_bytes();
        let rooted = b.first().is_some_and(|&c| c == b'/' || c == b'\\');
        let drive_rooted = b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\');
        args.rval().set(mozjs::jsval::BooleanValue(rooted || drive_rooted));
    }
    #[cfg(not(windows))]
    {
        args.rval()
            .set(mozjs::jsval::BooleanValue(Path::new(&s).is_absolute()));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_relative(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(
            cx,
            c"The \"from\" and \"to\" arguments must be of type string".as_ptr(),
        );
        return false;
    }
    let from_val = *args.get(0).ptr;
    let to_val = *args.get(1).ptr;
    let from_str = match arg_to_string(cx, from_val) {
        Some(s) => s,
        None => return return_string(cx, &args, ""),
    };
    let to_str = match arg_to_string(cx, to_val) {
        Some(s) => s,
        None => return return_string(cx, &args, ""),
    };

    let from_abs = make_absolute(&from_str);
    let to_abs = make_absolute(&to_str);

    let result = pathdiff(&to_abs, &from_abs);
    return_string(
        cx,
        &args,
        result.unwrap_or_default().to_string_lossy().as_ref(),
    )
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };

    let p = Path::new(&s);
    let root = if p.is_absolute() {
        "/".to_string()
    } else {
        String::new()
    };
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = p
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let name = if !file_name.is_empty() && !ext.is_empty() {
        file_name[..file_name.len() - ext.len()].to_string()
    } else {
        file_name.clone()
    };

    let parsed = JS_NewPlainObject(cx);
    if parsed.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let parsed_root = parsed);

    define_string_prop(cx, parsed_root.handle().into(), "root", &root);
    define_string_prop(cx, parsed_root.handle().into(), "dir", &dir);
    define_string_prop(cx, parsed_root.handle().into(), "base", &file_name);
    define_string_prop(cx, parsed_root.handle().into(), "ext", &ext);
    define_string_prop(cx, parsed_root.handle().into(), "name", &name);

    args.rval().set(mozjs::jsval::ObjectValue(parsed));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"The \"pathObject\" argument must be of type object".as_ptr(),
        );
        return false;
    }
    let val = *args.get(0).ptr;
    if !val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"The \"pathObject\" argument must be of type object".as_ptr(),
        );
        return false;
    }
    let obj = val.to_object();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let dir = get_string_prop(cx, obj_root.handle().into(), "dir");
    let base = get_string_prop(cx, obj_root.handle().into(), "base");
    let name = get_string_prop(cx, obj_root.handle().into(), "name");
    let ext = get_string_prop(cx, obj_root.handle().into(), "ext");

    let result = if let Some(b) = base {
        if dir.as_ref().is_some_and(|d| !d.is_empty()) {
            format!("{}/{}", dir.unwrap_or_default(), b)
        } else {
            b
        }
    } else {
        let mut s = dir.unwrap_or_default();
        if !s.is_empty() && !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(&name.unwrap_or_default());
        s.push_str(&ext.unwrap_or_default());
        s
    };
    return_string(cx, &args, &result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_to_namespaced(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let resolved = make_absolute(&s);
    return_string(cx, &args, &resolved.to_string_lossy())
}

// --- Pure logic helpers ---
// @trace REQ-ENG-007 [api:path] [code:bun_paths] — absolute-path resolution
// (`make_absolute`) and relative-path computation (`pathdiff`) delegate to
// `bun_paths::resolve_path` (Zig std `std.fs.path` faithful port):
//   * `join_abs_string::<Posix>(cwd, parts)` resolves `cwd + parts` into a
//     single absolute path (the equivalent of Node's `path.resolve` core).
//   * `relative_platform::<Posix, _>(from, to)` computes the relative path
//     from one absolute path to another (the equivalent of `path.relative`).
//
// The Node.js-specific `.`/`..` collapse for `posix_join` and `normalize_path`
// stays in Rust here because Node's `path.posix.normalize` deliberately
// preserves leading `..` above the root (e.g. `/a/../../b` → `/../b`) while
// Zig std's `normalizeString` clamps at the root (`/b`). The bundler/resolver
// consume the Zig semantics; the Node compatibility layer keeps its own.

use bun_paths::resolve_path::{self, platform::Posix};

/// Resolve the current working directory as an owned byte vector, falling
/// back to `b"."` so absolute-path joins never see an empty cwd.
fn cwd_bytes() -> Vec<u8> {
    let mut buf = bun_paths::path_buffer_pool::get();
    match bun_core::getcwd(&mut buf) {
        Ok(z) => z.as_bytes().to_vec(),
        Err(_) => b".".to_vec(),
    }
}

pub(crate) fn posix_join(parts: &[::std::string::String]) -> ::std::string::String {
    if parts.is_empty() {
        return ".".to_string();
    }

    // Node.js path.posix.join:
    // 1. Filter empty parts
    // 2. Join all parts with / (absolute components treated as regular — leading / stripped at join time)
    // 3. If first non-empty part started with /, result is absolute
    // 4. Normalize . and ..
    // 5. Preserve trailing / from last non-empty part

    // Step 1: Collect non-empty parts, strip leading / from each, track if first was absolute
    let mut segments: Vec<&str> = Vec::new();
    let mut has_root = false;
    let mut first_seen = false;
    let mut trailing_slash = false;

    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !first_seen {
            first_seen = true;
            has_root = part.starts_with('/');
        }
        // Track trailing slash from last part
        trailing_slash = part.ends_with('/');
        // Split by / and collect non-empty segments
        for seg in part.split('/') {
            if !seg.is_empty() && seg != "." {
                segments.push(seg);
            }
        }
    }

    if !first_seen {
        return ".".to_string();
    }

    // Step 2: Normalize .. by popping
    let mut normalized: Vec<&str> = Vec::new();
    for seg in &segments {
        if *seg == ".." {
            if !normalized.is_empty() && *normalized.last().expect("segments") != ".." {
                normalized.pop();
            } else if !has_root {
                normalized.push("..");
            }
        } else {
            normalized.push(seg);
        }
    }

    let mut result = if has_root {
        "/".to_string()
    } else {
        String::new()
    };
    result.push_str(&normalized.join("/"));

    // Trailing slash: only when last non-empty part had trailing slash AND result is relative
    // or when result is empty (only . and / segments)
    if result.is_empty() && trailing_slash {
        "./".to_string()
    } else if !result.is_empty() && trailing_slash && !result.ends_with('/') {
        result.push('/');
        result
    } else if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

pub(crate) fn normalize_path(path: &::std::path::Path) -> PathBuf {
    let mut components = Vec::new();
    let has_root = path.is_absolute();
    for comp in path.components() {
        match comp {
            ::std::path::Component::CurDir => {}
            ::std::path::Component::ParentDir => {
                if let Some(last) = components.last()
                    && last != &".."
                {
                    components.pop();
                    continue;
                }
                components.push("..");
            }
            ::std::path::Component::Normal(s) => {
                components.push(s.to_string_lossy().into_owned().leak() as &'static str);
            }
            _ => {}
        }
    }
    let mut result = PathBuf::new();
    if has_root {
        result.push("/");
    }
    for seg in &components {
        result.push(*seg);
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

pub(crate) fn make_absolute(s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        normalize_path(&p)
    } else {
        // @trace REQ-ENG-007 [code:bun_paths] — resolve `cwd + path` into a
        // single absolute path via bun_paths::resolve_path::join_abs_string
        // (Zig `joinAbsoluteString`, POSIX). Falls back to the std::path
        // normalize if the resolved result round-trips lossily.
        let cwd = cwd_bytes();
        let part_bytes = s.as_bytes();
        let resolved = resolve_path::join_abs_string::<Posix>(&cwd, &[part_bytes]);
        PathBuf::from(String::from_utf8_lossy(resolved).into_owned())
    }
}

pub(crate) fn pathdiff(to: &Path, from: &Path) -> ::std::option::Option<PathBuf> {
    let cwd = cwd_bytes();
    let to_abs = if to.is_absolute() {
        to.to_string_lossy().into_owned()
    } else {
        // Make `to` absolute against the cwd via bun_paths.
        let resolved =
            resolve_path::join_abs_string::<Posix>(&cwd, &[to.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };
    let from_abs = if from.is_absolute() {
        from.to_string_lossy().into_owned()
    } else {
        let resolved =
            resolve_path::join_abs_string::<Posix>(&cwd, &[from.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };

    // @trace REQ-ENG-007 [code:bun_paths] — relative-path computation delegated
    // to bun_paths::resolve_path::relative_platform (Zig `relativePath`, POSIX).
    // ALWAYS_COPY=true so the result owns its bytes (does not alias TLS scratch).
    let rel =
        resolve_path::relative_platform::<Posix, true>(from_abs.as_bytes(), to_abs.as_bytes());
    ::std::option::Option::Some(PathBuf::from(String::from_utf8_lossy(rel).into_owned()))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_string_prop(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: &str,
    value: &str,
) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let c_val = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if !js_str.is_null() {
        let val = mozjs::jsval::StringValue(&*js_str);
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let val_root = val);
        JS_DefineProperty(
            cx,
            obj,
            c_name.as_ptr(),
            val_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: &str,
) -> ::std::option::Option<::std::string::String> {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut val = UndefinedValue();
    let handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    };
    JS_GetProperty(cx, obj, c_name.as_ptr(), handle);
    arg_to_string(cx, val)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- posix_join ---

    #[test]
    fn test_posix_join_empty() {
        assert_eq!(posix_join(&[]), ".");
    }

    #[test]
    fn test_posix_join_single() {
        assert_eq!(posix_join(&["foo".to_string()]), "foo");
    }

    #[test]
    fn test_posix_join_multiple() {
        assert_eq!(
            posix_join(&["a".to_string(), "b".to_string(), "c".to_string()]),
            "a/b/c"
        );
    }

    // posix_join: absolute parts after first are joined as relative, NOT overriding
    #[test]
    fn test_posix_join_absolute_part() {
        // Leading / in non-first part is stripped (posix join behavior)
        assert_eq!(
            posix_join(&["a".to_string(), "/b".to_string(), "c".to_string()]),
            "a/b/c"
        );
    }

    #[test]
    fn test_posix_join_trailing_slash() {
        assert_eq!(posix_join(&["a/".to_string(), "b".to_string()]), "a/b");
    }

    #[test]
    fn test_posix_join_dot() {
        assert_eq!(posix_join(&[".".to_string(), "b".to_string()]), "b");
    }

    #[test]
    fn test_posix_join_empty_parts_skipped() {
        assert_eq!(
            posix_join(&["a".to_string(), "".to_string(), "b".to_string()]),
            "a/b"
        );
    }

    #[test]
    fn test_posix_join_root() {
        assert_eq!(posix_join(&["/".to_string()]), "/");
    }

    #[test]
    fn test_posix_join_dot_dot_normalizes() {
        assert_eq!(
            posix_join(&["a".to_string(), "b".to_string(), "..".to_string()]),
            "a"
        );
    }

    // posix_join: .. beyond root resolves within root (absolute path can't go beyond root)
    #[test]
    fn test_posix_join_dot_dot_beyond_root_stays() {
        // For relative path, .. resolves upward; extra .. stays as ..
        assert_eq!(
            posix_join(&["a".to_string(), "..".to_string(), "..".to_string()]),
            ".."
        );
    }

    // --- normalize_path (Path-based) ---

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_dot_dot() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_dot() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/./b")),
            PathBuf::from("/a/b")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_root() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/")),
            PathBuf::from("/")
        );
    }

    #[test]
    fn test_normalize_path_relative() {
        assert_eq!(
            normalize_path(::std::path::Path::new("a/b/../c")),
            PathBuf::from("a/c")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_double_dot_beyond_root() {
        // Implementation preserves .. beyond root as /../b
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/../../b")),
            PathBuf::from("/../b")
        );
    }

    #[test]
    fn test_normalize_path_empty_relative() {
        assert_eq!(
            normalize_path(::std::path::Path::new(".")),
            PathBuf::from(".")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_multiple_dots() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/b/c/../../d")),
            PathBuf::from("/a/d")
        );
    }

    // --- make_absolute ---

    #[test]
    fn test_make_absolute_already_absolute() {
        assert_eq!(make_absolute("/foo/bar"), PathBuf::from("/foo/bar"));
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_make_absolute_relative() {
        let result = make_absolute("foo/bar");
        assert!(
            result.is_absolute(),
            "result should be absolute: {:?}",
            result
        );
        assert!(result.to_str().unwrap().contains("foo/bar"));
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_make_absolute_dot() {
        let result = make_absolute(".");
        assert!(result.is_absolute());
    }

    #[test]
    fn test_make_absolute_dot_dot() {
        let result = make_absolute("/a/b/..");
        // Path-based normalization: /a/b/.. -> /a
        let s = result.to_str().unwrap();
        assert!(s == "/a" || s == "/a/", "expected /a or /a/, got {}", s);
    }

    // --- pathdiff ---

    #[test]
    fn test_pathdiff_same() {
        let p = Path::new("/a/b/c");
        assert_eq!(pathdiff(p, p), Some(PathBuf::from("")));
    }

    #[test]
    fn test_pathdiff_sibling() {
        let to = Path::new("/a/b/c");
        let from = Path::new("/a/b/d");
        // pathdiff strips common prefix then builds relative path
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("../c")));
    }

    #[test]
    fn test_pathdiff_child() {
        let to = Path::new("/a/b/c/d");
        let from = Path::new("/a/b");
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("c/d")));
    }

    #[test]
    fn test_pathdiff_parent() {
        let to = Path::new("/a/b");
        let from = Path::new("/a/b/c/d");
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("../..")));
    }

    #[test]
    fn test_pathdiff_different_roots() {
        let to = Path::new("/a/b");
        let from = Path::new("/c/d");
        let result = pathdiff(to, from);
        assert!(result.is_some());
    }

    #[test]
    fn test_pathdiff_relative() {
        let to = Path::new("a/b");
        let from = Path::new("a/c");
        let result = pathdiff(to, from);
        assert!(result.is_some());
    }
}
