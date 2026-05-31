// @trace REQ-LIB-004  REQ-LIB-003: Permission sandbox with zero-overhead none mode

#[derive(Debug, Clone, Default)]
pub struct Permission {
    pub read: Option<Vec<String>>,
    pub write: Option<Vec<String>>,
    pub net: Option<Vec<String>>,
    pub env: Option<bool>,
    pub run: Option<bool>,
}

impl Permission {
    pub fn is_read_allowed(&self, path: &str) -> bool {
        match &self.read {
            None => true,
            Some(allowed) => allowed.iter().any(|prefix| path.starts_with(prefix)),
        }
    }

    pub fn is_write_allowed(&self, path: &str) -> bool {
        match &self.write {
            None => true,
            Some(allowed) => allowed.iter().any(|prefix| path.starts_with(prefix)),
        }
    }

    pub fn is_net_allowed(&self, host: &str) -> bool {
        match &self.net {
            None => true,
            Some(allowed) => allowed.iter().any(|domain| {
                host == domain || host.ends_with(&format!(".{domain}"))
            }),
        }
    }

    pub fn is_env_allowed(&self) -> bool {
        self.env.unwrap_or(true)
    }

    pub fn is_run_allowed(&self) -> bool {
        self.run.unwrap_or(true)
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

    pub fn check_read(&self, path: &str) -> Result<(), PermissionDenied> {
        match &self.inner {
            None => Ok(()),
            Some(perm) => {
                if perm.is_read_allowed(path) {
                    Ok(())
                } else {
                    Err(PermissionDenied {
                        category: "read".into(),
                        resource: path.into(),
                    })
                }
            }
        }
    }

    pub fn check_write(&self, path: &str) -> Result<(), PermissionDenied> {
        match &self.inner {
            None => Ok(()),
            Some(perm) => {
                if perm.is_write_allowed(path) {
                    Ok(())
                } else {
                    Err(PermissionDenied {
                        category: "write".into(),
                        resource: path.into(),
                    })
                }
            }
        }
    }

    pub fn check_net(&self, host: &str) -> Result<(), PermissionDenied> {
        match &self.inner {
            None => Ok(()),
            Some(perm) => {
                if perm.is_net_allowed(host) {
                    Ok(())
                } else {
                    Err(PermissionDenied {
                        category: "net".into(),
                        resource: host.into(),
                    })
                }
            }
        }
    }

    pub fn check_env(&self) -> Result<(), PermissionDenied> {
        match &self.inner {
            None => Ok(()),
            Some(perm) => {
                if perm.is_env_allowed() {
                    Ok(())
                } else {
                    Err(PermissionDenied {
                        category: "env".into(),
                        resource: "*".into(),
                    })
                }
            }
        }
    }

    pub fn check_run(&self) -> Result<(), PermissionDenied> {
        match &self.inner {
            None => Ok(()),
            Some(perm) => {
                if perm.is_run_allowed() {
                    Ok(())
                } else {
                    Err(PermissionDenied {
                        category: "run".into(),
                        resource: "*".into(),
                    })
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionDenied {
    pub category: String,
    pub resource: String,
}

impl std::fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Permission denied: {} on {}", self.category, self.resource)
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
        };
        assert!(!perm.is_read_allowed("/any"));
        assert!(!perm.is_write_allowed("/any"));
        assert!(!perm.is_net_allowed("any.com"));
    }
}
