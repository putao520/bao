// @trace REQ-ENG-001
//! Node.js `path` module — pure Rust path utilities for `bun_jsc::NodePath` compatibility.
//!
//! JS bindings are in `bao_runtime::node_path`. This module provides
//! the pure-logic path manipulation functions.

/// POSIX path join — correctly handles `.`, `..`, absolute segments.
pub fn posix_join(paths: &[&str]) -> String {
    let mut result = String::new();
    let mut is_absolute = false;
    let mut segments: Vec<&str> = Vec::new();

    for path in paths {
        if path.starts_with('/') {
            is_absolute = true;
            segments.clear();
        }
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                _ => segments.push(part),
            }
        }
    }

    if is_absolute {
        result.push('/');
    }
    result.push_str(&segments.join("/"));
    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

/// Normalize a POSIX path — resolve `.` and `..`, remove redundant separators.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let is_absolute = path.starts_with('/');
    let trailing_slash = path.ends_with('/') && path.len() > 1;

    let mut segments: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().map_or(false, |s| *s != "..") {
                    segments.pop();
                } else if !is_absolute {
                    segments.push("..");
                }
            }
            _ => segments.push(part),
        }
    }

    let mut result = String::new();
    if is_absolute {
        result.push('/');
    }
    result.push_str(&segments.join("/"));

    if result.is_empty() {
        if is_absolute {
            "/".to_string()
        } else {
            ".".to_string()
        }
    } else if trailing_slash && !result.ends_with('/') && result != "/" {
        result.push('/');
        result
    } else {
        result
    }
}

/// Return the directory name of a path.
pub fn dirname(path: &str) -> String {
    if path.is_empty() || path == "." || path == ".." {
        return ".".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        ".".to_string()
    }
}

/// Return the last portion of a path, optionally stripping an extension.
pub fn basename(path: &str, ext: Option<&str>) -> String {
    let trimmed = path.trim_end_matches('/');
    let name = if let Some(pos) = trimmed.rfind('/') {
        &trimmed[pos + 1..]
    } else {
        trimmed
    };
    if name.is_empty() {
        return String::new();
    }
    if let Some(e) = ext {
        if name.ends_with(e) && name != e {
            name[..name.len() - e.len()].to_string()
        } else {
            name.to_string()
        }
    } else {
        name.to_string()
    }
}

/// Return the extension of the path, from the last `.` to end of filename.
pub fn extname(path: &str) -> String {
    let name = basename(path, None);
    if name.starts_with('.') && name.chars().filter(|&c| c == '.').count() == 1 {
        return String::new();
    }
    if let Some(pos) = name.rfind('.') {
        name[pos..].to_string()
    } else {
        String::new()
    }
}

/// Check if a path is absolute.
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// POSIX path separator.
pub const SEP: &str = "/";

/// POSIX path delimiter (for PATH env var).
pub const DELIMITER: &str = ":";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_basic() {
        assert_eq!(posix_join(&["a", "b", "c"]), "a/b/c");
    }

    #[test]
    fn join_absolute() {
        assert_eq!(posix_join(&["/a", "b", "c"]), "/a/b/c");
    }

    #[test]
    fn join_dotdot() {
        assert_eq!(posix_join(&["a", "b", "..", "c"]), "a/c");
    }

    #[test]
    fn join_dot() {
        assert_eq!(posix_join(&["a", ".", "b"]), "a/b");
    }

    #[test]
    fn join_empty() {
        assert_eq!(posix_join(&[]), ".");
    }

    #[test]
    fn normalize_dots() {
        assert_eq!(normalize("a/./b/../c"), "a/c");
    }

    #[test]
    fn normalize_absolute() {
        assert_eq!(normalize("/a/b/../c"), "/a/c");
    }

    #[test]
    fn normalize_trailing_slash() {
        assert_eq!(normalize("a/b/"), "a/b/");
    }

    #[test]
    fn dirname_basic() {
        assert_eq!(dirname("/a/b/c"), "/a/b");
    }

    #[test]
    fn dirname_root() {
        assert_eq!(dirname("/a"), "/");
    }

    #[test]
    fn basename_basic() {
        assert_eq!(basename("/a/b/c.txt", None), "c.txt");
    }

    #[test]
    fn basename_strip_ext() {
        assert_eq!(basename("/a/b/c.txt", Some(".txt")), "c");
    }

    #[test]
    fn extname_basic() {
        assert_eq!(extname("file.txt"), ".txt");
    }

    #[test]
    fn extname_dotfile() {
        assert_eq!(extname(".bashrc"), "");
    }

    #[test]
    fn is_absolute_posix() {
        assert!(is_absolute("/usr/bin"));
        assert!(!is_absolute("usr/bin"));
    }
}
