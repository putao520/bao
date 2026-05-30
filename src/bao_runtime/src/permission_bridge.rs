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
