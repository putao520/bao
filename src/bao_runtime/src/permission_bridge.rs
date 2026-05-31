// @trace REQ-LIB-004
use ::std::cell::RefCell;

thread_local! {
    static PERMISSION_GUARD: RefCell<Option<PermissionCheck>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub struct PermissionCheck {
    pub read_paths: Option<Vec<String>>,
    pub write_paths: Option<Vec<String>>,
    pub net_hosts: Option<Vec<String>>,
    pub env_allowed: bool,
    pub run_allowed: bool,
}

pub fn set_permission(check: Option<PermissionCheck>) {
    PERMISSION_GUARD.with(|g| *g.borrow_mut() = check);
}

pub fn check_fs_read(path: &str) -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| {
        match g.borrow().as_ref() {
            None => Ok(()),
            Some(perm) => match &perm.read_paths {
                None => Ok(()),
                Some(allowed) => {
                    if allowed.iter().any(|prefix| path.starts_with(prefix.as_str())) {
                        Ok(())
                    } else {
                        Err(format!("Permission denied: read on {}", path))
                    }
                }
            }
        }
    })
}

pub fn check_fs_write(path: &str) -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| {
        match g.borrow().as_ref() {
            None => Ok(()),
            Some(perm) => match &perm.write_paths {
                None => Ok(()),
                Some(allowed) => {
                    if allowed.iter().any(|prefix| path.starts_with(prefix.as_str())) {
                        Ok(())
                    } else {
                        Err(format!("Permission denied: write on {}", path))
                    }
                }
            }
        }
    })
}

pub fn check_net(host: &str) -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| {
        match g.borrow().as_ref() {
            None => Ok(()),
            Some(perm) => match &perm.net_hosts {
                None => Ok(()),
                Some(allowed) => {
                    if allowed.iter().any(|domain| host == domain || host.ends_with(&format!(".{}", domain))) {
                        Ok(())
                    } else {
                        Err(format!("Permission denied: net on {}", host))
                    }
                }
            }
        }
    })
}

pub fn check_env() -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| {
        match g.borrow().as_ref() {
            None => Ok(()),
            Some(perm) => {
                if perm.env_allowed { Ok(()) }
                else { Err("Permission denied: env".into()) }
            }
        }
    })
}

pub fn check_run() -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| {
        match g.borrow().as_ref() {
            None => Ok(()),
            Some(perm) => {
                if perm.run_allowed { Ok(()) }
                else { Err("Permission denied: run".into()) }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleanup() {
        set_permission(None);
    }

    #[test]
    fn test_no_permission_allows_all() {
        cleanup();
        assert!(check_fs_read("/etc/passwd").is_ok());
        assert!(check_fs_write("/tmp/test").is_ok());
        assert!(check_net("evil.com").is_ok());
        assert!(check_env().is_ok());
        assert!(check_run().is_ok());
    }

    #[test]
    fn test_fs_read_allowed_prefix() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/home".into(), "/tmp".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read("/home/user/file").is_ok());
        assert!(check_fs_read("/tmp/data").is_ok());
        assert!(check_fs_read("/etc/passwd").is_err());
        cleanup();
    }

    #[test]
    fn test_fs_read_none_allows_all() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read("/anything").is_ok());
        cleanup();
    }

    #[test]
    fn test_fs_write_allowed_prefix() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: Some(vec!["/tmp".into()]),
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_write("/tmp/output").is_ok());
        assert!(check_fs_write("/etc/shadow").is_err());
        cleanup();
    }

    #[test]
    fn test_net_exact_match() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["example.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("example.com").is_ok());
        assert!(check_net("evil.com").is_err());
        cleanup();
    }

    #[test]
    fn test_net_subdomain_match() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["example.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("sub.example.com").is_ok());
        assert!(check_net("deep.sub.example.com").is_ok());
        cleanup();
    }

    #[test]
    fn test_net_partial_mismatch() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["example.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("notexample.com").is_err());
        assert!(check_net("xnotexample.com").is_err());
        cleanup();
    }

    #[test]
    fn test_env_allowed() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: false,
        }));
        assert!(check_env().is_ok());
        assert!(check_run().is_err());
        cleanup();
    }

    #[test]
    fn test_run_denied() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: false,
            run_allowed: false,
        }));
        assert!(check_env().is_err());
        assert!(check_run().is_err());
        cleanup();
    }

    #[test]
    fn test_set_permission_overrides() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/safe".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read("/safe/file").is_ok());
        assert!(check_fs_read("/unsafe").is_err());

        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/unsafe".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read("/unsafe/file").is_ok());
        assert!(check_fs_read("/safe/file").is_err());
        cleanup();
    }

    #[test]
    fn test_error_messages_descriptive() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/allowed".into()]),
            write_paths: Some(vec!["/allowed".into()]),
            net_hosts: Some(vec!["safe.com".into()]),
            env_allowed: false,
            run_allowed: false,
        }));
        assert!(check_fs_read("/denied").unwrap_err().contains("read on /denied"));
        assert!(check_fs_write("/denied").unwrap_err().contains("write on /denied"));
        assert!(check_net("evil.com").unwrap_err().contains("net on evil.com"));
        assert_eq!(check_env().unwrap_err(), "Permission denied: env");
        assert_eq!(check_run().unwrap_err(), "Permission denied: run");
        cleanup();
    }
}
