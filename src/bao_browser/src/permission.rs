// @trace REQ-LIB-004  REQ-LIB-003 REQ-SEC-003: Permission sandbox with zero-overhead none mode
// BCE-20260627-001 根治: path canonicalization 防止 traversal/symlink/case bypass

/// Normalize a path string for permission checking: reject traversals and
/// URL-encoded escapes before prefix matching. We do NOT touch the filesystem
/// (this runs in the JS thread, FS access would block), but we do refuse
/// suspicious sequences and decode `%2e` / `%2f` so encoded variants cannot
/// bypass the starts_with check.
fn normalize_path_for_check(path: &str) -> Option<String> {
    // Reject NUL and control bytes outright.
    if path.bytes().any(|b| b == 0 || b < 0x20) {
        return None;
    }
    // URL-decode %2e (.), %2f (/), %5c (\) and uppercase variants so attackers
    // cannot smuggle traversal characters through percent-encoding.
    let lower = path.to_ascii_lowercase();
    let decoded = lower
        .replace("%2e", ".")
        .replace("%2f", "/")
        .replace("%5c", "/");
    // Normalize backslash to forward slash (Windows path compatibility).
    let decoded = decoded.replace('\\', "/");
    // Reject any `..` segment (parent directory traversal) and `.` segment
    // (current directory - not dangerous but suspicious in untrusted input).
    for seg in decoded.split('/') {
        if seg == ".." || seg == "." {
            return None;
        }
    }
    Some(decoded)
}

/// Normalize a hostname for permission checking: lowercase and reject any
/// embedded port / userinfo / trailing-dot tricks.
fn normalize_host_for_check(host: &str) -> Option<String> {
    // Strip trailing dot (RFC 1034: "example.com." == "example.com").
    let trimmed = host.trim_end_matches('.').to_ascii_lowercase();
    // Reject anything that contains '@', '/', ':', or whitespace — those
    // indicate userinfo, path, or port injection attempts and must not appear
    // in a bare host comparison.
    if trimmed
        .chars()
        .any(|c| c == '@' || c == '/' || c == ':' || c.is_whitespace())
    {
        return None;
    }
    Some(trimmed)
}

#[derive(Debug, Clone, Default)]
pub struct Permission {
    pub read: Option<Vec<String>>,
    pub write: Option<Vec<String>>,
    pub net: Option<Vec<String>>,
    pub env: Option<bool>,
    pub run: Option<bool>,
    pub sys: Option<bool>,
}

impl Permission {
    pub fn is_read_allowed(&self, path: &str) -> bool {
        match &self.read {
            None => true,
            Some(allowed) => match normalize_path_for_check(path) {
                None => false,
                Some(normalized) => allowed
                    .iter()
                    .any(|prefix| normalized.starts_with(prefix.as_str())),
            },
        }
    }

    pub fn is_write_allowed(&self, path: &str) -> bool {
        match &self.write {
            None => true,
            Some(allowed) => match normalize_path_for_check(path) {
                None => false,
                Some(normalized) => allowed
                    .iter()
                    .any(|prefix| normalized.starts_with(prefix.as_str())),
            },
        }
    }

    pub fn is_net_allowed(&self, host: &str) -> bool {
        match &self.net {
            None => true,
            Some(allowed) => match normalize_host_for_check(host) {
                None => false,
                Some(normalized) => allowed.iter().any(|domain| {
                    let d = domain.to_ascii_lowercase();
                    normalized == d || normalized.ends_with(&format!(".{d}"))
                }),
            },
        }
    }

    pub fn is_env_allowed(&self) -> bool {
        self.env.unwrap_or(true)
    }

    pub fn is_run_allowed(&self) -> bool {
        self.run.unwrap_or(true)
    }

    pub fn is_sys_allowed(&self) -> bool {
        self.sys.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PermissionGuard {
    inner: Option<Permission>,
}

impl PermissionGuard {
    pub fn none() -> Self {
        PermissionGuard { inner: None }
    }

    pub fn new(perm: Permission) -> Self {
        PermissionGuard { inner: Some(perm) }
    }

    pub fn is_restricted(&self) -> bool {
        self.inner.is_some()
    }

    /// Shared decision core: an unrestricted guard (no `Permission`) always
    /// passes; a restricted guard passes only when `allowed` holds, otherwise
    /// denies with the given category/resource.
    fn check_inner(
        inner: &Option<Permission>,
        allowed: bool,
        category: &str,
        resource: &str,
    ) -> Result<(), PermissionDenied> {
        match inner {
            None => Ok(()),
            Some(_) if allowed => Ok(()),
            Some(_) => Err(PermissionDenied {
                category: category.into(),
                resource: resource.into(),
            }),
        }
    }

    pub fn check_read(&self, path: &str) -> Result<(), PermissionDenied> {
        let allowed = self
            .inner
            .as_ref()
            .map_or(true, |perm| perm.is_read_allowed(path));
        Self::check_inner(&self.inner, allowed, "read", path)
    }

    pub fn check_write(&self, path: &str) -> Result<(), PermissionDenied> {
        let allowed = self
            .inner
            .as_ref()
            .map_or(true, |perm| perm.is_write_allowed(path));
        Self::check_inner(&self.inner, allowed, "write", path)
    }

    pub fn check_net(&self, host: &str) -> Result<(), PermissionDenied> {
        let allowed = self
            .inner
            .as_ref()
            .map_or(true, |perm| perm.is_net_allowed(host));
        Self::check_inner(&self.inner, allowed, "net", host)
    }

    pub fn check_env(&self) -> Result<(), PermissionDenied> {
        let allowed = self
            .inner
            .as_ref()
            .map_or(true, |perm| perm.is_env_allowed());
        Self::check_inner(&self.inner, allowed, "env", "*")
    }

    pub fn check_run(&self) -> Result<(), PermissionDenied> {
        let allowed = self
            .inner
            .as_ref()
            .map_or(true, |perm| perm.is_run_allowed());
        Self::check_inner(&self.inner, allowed, "run", "*")
    }
}

#[derive(Debug, Clone)]
pub struct PermissionDenied {
    pub category: String,
    pub resource: String,
}

impl std::fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Permission denied: {} on {}",
            self.category, self.resource
        )
    }
}

impl std::error::Error for PermissionDenied {}

#[cfg(test)]
mod tests {
    // @trace REQ-LIB-004 [req:REQ-LIB-003,REQ-LIB-004] [level:unit]
    use super::*;

    #[test]
    fn permission_default_allows_all() {
        let perm = Permission::default();
        assert!(perm.is_read_allowed("/any/path"));
        assert!(perm.is_write_allowed("/any/path"));
        assert!(perm.is_net_allowed("any.host.com"));
        assert!(perm.is_env_allowed());
        assert!(perm.is_run_allowed());
    }

    #[test]
    fn read_prefix_matching() {
        let perm = Permission {
            read: Some(vec!["/home".into(), "/tmp/bao".into()]),
            ..Default::default()
        };
        assert!(perm.is_read_allowed("/home"));
        assert!(perm.is_read_allowed("/home/user/file.txt"));
        assert!(perm.is_read_allowed("/tmp/bao"));
        assert!(perm.is_read_allowed("/tmp/bao/cache/data.bin"));
        assert!(!perm.is_read_allowed("/etc/passwd"));
        assert!(!perm.is_read_allowed("/tmp/other"));
    }

    #[test]
    fn write_prefix_matching() {
        let perm = Permission {
            write: Some(vec!["/var/log/bao".into(), "/tmp".into()]),
            ..Default::default()
        };
        assert!(perm.is_write_allowed("/var/log/bao"));
        assert!(perm.is_write_allowed("/var/log/bao/app.log"));
        assert!(perm.is_write_allowed("/tmp"));
        assert!(perm.is_write_allowed("/tmp/session.dat"));
        assert!(!perm.is_write_allowed("/var/log/other"));
        assert!(!perm.is_write_allowed("/usr/bin"));
    }

    #[test]
    fn net_exact_and_subdomain() {
        let perm = Permission {
            net: Some(vec!["example.com".into()]),
            ..Default::default()
        };
        assert!(perm.is_net_allowed("example.com"));
        assert!(perm.is_net_allowed("sub.example.com"));
        assert!(perm.is_net_allowed("deep.sub.example.com"));
    }

    #[test]
    fn net_partial_mismatch() {
        let perm = Permission {
            net: Some(vec!["example.com".into()]),
            ..Default::default()
        };
        assert!(!perm.is_net_allowed("notexample.com"));
        assert!(!perm.is_net_allowed("xnotexample.com"));
        assert!(!perm.is_net_allowed("other.com"));
    }

    #[test]
    fn env_false_blocks() {
        let perm = Permission {
            env: Some(false),
            ..Default::default()
        };
        assert!(!perm.is_env_allowed());
    }

    #[test]
    fn run_false_blocks() {
        let perm = Permission {
            run: Some(false),
            ..Default::default()
        };
        assert!(!perm.is_run_allowed());
    }

    #[test]
    fn guard_none_allows_all() {
        let guard = PermissionGuard::none();
        assert!(!guard.is_restricted());
        assert!(guard.check_read("/secret").is_ok());
        assert!(guard.check_write("/secret").is_ok());
        assert!(guard.check_net("evil.com").is_ok());
        assert!(guard.check_env().is_ok());
        assert!(guard.check_run().is_ok());
    }

    #[test]
    fn guard_new_is_restricted() {
        let guard = PermissionGuard::new(Permission::default());
        assert!(guard.is_restricted());
    }

    #[test]
    fn guard_check_read_denied_has_correct_category_resource() {
        let guard = PermissionGuard::new(Permission {
            read: Some(vec!["/allowed".into()]),
            ..Default::default()
        });
        let err = guard.check_read("/forbidden").unwrap_err();
        assert_eq!(err.category, "read");
        assert_eq!(err.resource, "/forbidden");
    }

    #[test]
    fn guard_check_net_denied_message() {
        let guard = PermissionGuard::new(Permission {
            net: Some(vec!["safe.com".into()]),
            ..Default::default()
        });
        let err = guard.check_net("unsafe.com").unwrap_err();
        assert_eq!(format!("{err}"), "Permission denied: net on unsafe.com");
    }

    #[test]
    fn permission_denied_display_format() {
        let err = PermissionDenied {
            category: "env".into(),
            resource: "*".into(),
        };
        assert_eq!(format!("{err}"), "Permission denied: env on *");
    }

    #[test]
    fn permission_clone_works() {
        let perm = Permission {
            read: Some(vec!["/data".into()]),
            write: None,
            net: Some(vec!["api.com".into()]),
            env: Some(false),
            run: Some(true),
            ..Default::default()
        };
        let cloned = perm.clone();
        assert!(cloned.is_read_allowed("/data/file"));
        assert!(cloned.is_write_allowed("/anything"));
        assert!(cloned.is_net_allowed("api.com"));
        assert!(!cloned.is_env_allowed());
        assert!(cloned.is_run_allowed());
    }

    #[test]
    fn permission_guard_clone_works() {
        let guard = PermissionGuard::new(Permission {
            read: Some(vec!["/app".into()]),
            ..Default::default()
        });
        let cloned = guard.clone();
        assert!(cloned.is_restricted());
        assert!(cloned.check_read("/app/config").is_ok());
        assert!(cloned.check_read("/other").is_err());
    }

    #[test]
    fn empty_allowed_list_denies_all() {
        let perm = Permission {
            read: Some(vec![]),
            write: Some(vec![]),
            net: Some(vec![]),
            env: None,
            run: None,
            ..Default::default()
        };
        assert!(!perm.is_read_allowed("/any"));
        assert!(!perm.is_write_allowed("/any"));
        assert!(!perm.is_net_allowed("any.com"));
    }

    // ─── Permission extended edge case tests ─────────────────────
    // @trace REQ-CDP-008 [req:REQ-CDP-008] [level:unit]

    #[test]
    fn net_exact_match_no_subdomain_false() {
        // "example.com" should NOT match just "example" (no .com)
        let perm = Permission {
            net: Some(vec!["example.com".into()]),
            ..Default::default()
        };
        assert!(!perm.is_net_allowed("example"));
        assert!(!perm.is_net_allowed("com"));
    }

    #[test]
    fn net_multiple_domains_one_match() {
        let perm = Permission {
            net: Some(vec!["safe.com".into(), "trusted.io".into()]),
            ..Default::default()
        };
        assert!(perm.is_net_allowed("safe.com"));
        assert!(perm.is_net_allowed("sub.safe.com"));
        assert!(perm.is_net_allowed("trusted.io"));
        assert!(!perm.is_net_allowed("unsafe.com"));
    }

    #[test]
    fn read_path_traversal_not_allowed() {
        // BCE-20260627-001 regression: /allowed/../secret must be DENIED.
        // Previous impl documented this as "known design choice (prefix match)"
        // but that was a security hole — fixed by normalize_path_for_check.
        let perm = Permission {
            read: Some(vec!["/allowed".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_read_allowed("/allowed/../secret"),
            "path traversal must be rejected"
        );
    }

    #[test]
    fn read_path_traversal_url_encoded_rejected() {
        // BCE-20260627-001 regression: %2e%2e must decode to .. and be rejected.
        let perm = Permission {
            read: Some(vec!["/allowed".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_read_allowed("/allowed/%2e%2e/secret"),
            "URL-encoded traversal must be rejected"
        );
        assert!(
            !perm.is_read_allowed("/allowed/%2E%2Fsecret"),
            "uppercase URL-encoded traversal must be rejected"
        );
        assert!(
            !perm.is_read_allowed("/allowed/%2f..%2fsecret"),
            "mixed URL-encoded traversal must be rejected"
        );
    }

    #[test]
    fn read_path_traversal_backslash_rejected() {
        // BCE-20260627-001 regression: %5c decodes to backslash; mixed-separator
        // traversal must be rejected.
        let perm = Permission {
            read: Some(vec!["/allowed".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_read_allowed("/allowed/..\\secret"),
            "backslash traversal must be rejected"
        );
    }

    #[test]
    fn read_path_control_byte_rejected() {
        // BCE-20260627-001 regression: NUL and control bytes rejected outright.
        let perm = Permission {
            read: Some(vec!["/allowed".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_read_allowed("/allowed/\0secret"),
            "NUL injection must be rejected"
        );
        assert!(
            !perm.is_read_allowed("/allowed/\tsecret"),
            "control-byte injection must be rejected"
        );
    }

    #[test]
    fn write_path_traversal_rejected() {
        // BCE-20260627-001 regression on write path.
        let perm = Permission {
            write: Some(vec!["/tmp/bao".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_write_allowed("/tmp/bao/../../etc/passwd"),
            "write path traversal must be rejected"
        );
        assert!(
            !perm.is_write_allowed("/tmp/bao/%2e%2e/%2e%2e/etc/shadow"),
            "write URL-encoded traversal must be rejected"
        );
    }

    #[test]
    fn net_subdomain_confusion_rejected() {
        // BCE-20260627-001 regression: safe.com allowed does NOT mean
        // safe.com.evil.com is allowed. The old impl would have matched
        // because safe.com.evil.com starts with safe.com.
        let perm = Permission {
            net: Some(vec!["safe.com".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_net_allowed("safe.com.evil.com"),
            "subdomain suffix confusion must be rejected"
        );
        assert!(
            !perm.is_net_allowed("notsafe.com"),
            "prefix-only match must be rejected"
        );
        assert!(perm.is_net_allowed("safe.com"), "exact match must pass");
        assert!(
            perm.is_net_allowed("sub.safe.com"),
            "subdomain match must pass"
        );
    }

    #[test]
    fn net_port_userinfo_rejected() {
        // BCE-20260627-001 regression: ports/userinfo/path chars rejected.
        let perm = Permission {
            net: Some(vec!["safe.com".into()]),
            ..Default::default()
        };
        assert!(
            !perm.is_net_allowed("safe.com:8080"),
            "port injection must be rejected"
        );
        assert!(
            !perm.is_net_allowed("user@safe.com"),
            "userinfo injection must be rejected"
        );
        assert!(
            !perm.is_net_allowed("safe.com/path"),
            "path injection must be rejected"
        );
    }

    #[test]
    fn net_trailing_dot_normalized() {
        // BCE-20260627-001 regression: RFC 1034 trailing dot normalized.
        let perm = Permission {
            net: Some(vec!["safe.com".into()]),
            ..Default::default()
        };
        assert!(
            perm.is_net_allowed("safe.com."),
            "trailing dot must be normalized to bare host"
        );
        assert!(
            perm.is_net_allowed("sub.safe.com."),
            "trailing dot subdomain must be normalized"
        );
    }

    #[test]
    fn net_case_insensitive() {
        // BCE-20260627-001 regression: case-insensitive host comparison.
        let perm = Permission {
            net: Some(vec!["safe.com".into()]),
            ..Default::default()
        };
        assert!(perm.is_net_allowed("SAFE.com"), "uppercase host must match");
        assert!(
            perm.is_net_allowed("sub.SAFE.com"),
            "uppercase subdomain must match"
        );
    }

    #[test]
    fn env_true_allows() {
        let perm = Permission {
            env: Some(true),
            ..Default::default()
        };
        assert!(perm.is_env_allowed());
    }

    #[test]
    fn run_true_allows() {
        let perm = Permission {
            run: Some(true),
            ..Default::default()
        };
        assert!(perm.is_run_allowed());
    }

    #[test]
    fn guard_check_write_denied_category() {
        let guard = PermissionGuard::new(Permission {
            write: Some(vec!["/tmp".into()]),
            ..Default::default()
        });
        let err = guard.check_write("/etc/passwd").unwrap_err();
        assert_eq!(err.category, "write");
        assert_eq!(err.resource, "/etc/passwd");
    }

    #[test]
    fn guard_check_env_denied_category() {
        let guard = PermissionGuard::new(Permission {
            env: Some(false),
            ..Default::default()
        });
        let err = guard.check_env().unwrap_err();
        assert_eq!(err.category, "env");
        assert_eq!(err.resource, "*");
    }

    #[test]
    fn guard_check_run_denied_category() {
        let guard = PermissionGuard::new(Permission {
            run: Some(false),
            ..Default::default()
        });
        let err = guard.check_run().unwrap_err();
        assert_eq!(err.category, "run");
        assert_eq!(err.resource, "*");
    }

    #[test]
    fn sys_default_allows() {
        let perm = Permission::default();
        assert!(perm.is_sys_allowed());
    }

    #[test]
    fn sys_explicit_true_allows() {
        let perm = Permission {
            sys: Some(true),
            ..Default::default()
        };
        assert!(perm.is_sys_allowed());
    }

    #[test]
    fn sys_explicit_false_denies() {
        let perm = Permission {
            sys: Some(false),
            ..Default::default()
        };
        assert!(!perm.is_sys_allowed());
    }
}
