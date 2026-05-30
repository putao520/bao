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
