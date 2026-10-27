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
    PERMISSION_GUARD.with(|g| match g.borrow().as_ref() {
        None => Ok(()),
        Some(perm) => match &perm.read_paths {
            None => Ok(()),
            Some(allowed) => {
                if allowed
                    .iter()
                    .any(|prefix| path.starts_with(prefix.as_str()))
                {
                    Ok(())
                } else {
                    Err(format!("Permission denied: read on {}", path))
                }
            }
        },
    })
}

pub fn check_fs_write(path: &str) -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| match g.borrow().as_ref() {
        None => Ok(()),
        Some(perm) => match &perm.write_paths {
            None => Ok(()),
            Some(allowed) => {
                if allowed
                    .iter()
                    .any(|prefix| path.starts_with(prefix.as_str()))
                {
                    Ok(())
                } else {
                    Err(format!("Permission denied: write on {}", path))
                }
            }
        },
    })
}

pub fn check_net(host: &str) -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| match g.borrow().as_ref() {
        None => Ok(()),
        Some(perm) => match &perm.net_hosts {
            None => Ok(()),
            Some(allowed) => {
                if allowed
                    .iter()
                    .any(|domain| host == domain || host.ends_with(&format!(".{}", domain)))
                {
                    Ok(())
                } else {
                    Err(format!("Permission denied: net on {}", host))
                }
            }
        },
    })
}

/// Shared boolean-permission gate: `check_env`/`check_run` differ only in
/// which flag they consult and the denial label they report
/// (REQ-ARCH-001 duplicate convergence).
fn check_flag(allowed: bool, denied: &str) -> ::std::result::Result<(), String> {
    if allowed {
        Ok(())
    } else {
        Err(format!("Permission denied: {}", denied).into())
    }
}

pub fn check_env() -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| match g.borrow().as_ref() {
        None => Ok(()),
        Some(perm) => check_flag(perm.env_allowed, "env"),
    })
}

pub fn check_run() -> ::std::result::Result<(), String> {
    PERMISSION_GUARD.with(|g| match g.borrow().as_ref() {
        None => Ok(()),
        Some(perm) => check_flag(perm.run_allowed, "run"),
    })
}

// ─── node-parity CLI permission model (`process.permission` face) ──────────
// @trace REQ-ENG-007 [criterion:7]
//
// node v24 `--permission` 启动旗标铸出进程级权限模型,`process.permission.has`
// 是其唯一观察面(oracle v24.19.0:无旗标 = `process.permission` 为 undefined;
// 有旗标 = 对象仅含 `has`)。与 PERMISSION_GUARD(Bao.run/per-Page 沙箱,
// thread-local)刻意分离 —— 写 guard 会破坏 permission_options.rs 模块头的
// base=None 栈式恢复不变量(Bao.run scope Drop 会把 CLI 模型冲回 None)。
// 模型 = 进程级单槽:CLI dispatch 时一次写入,has() 任意线程读。

/// node `--permission` 模型的 bao 形态:显式 allow 清单(真语义,非伪造恒真)。
#[derive(Debug, Clone, PartialEq)]
pub struct CliPermissionModel {
    /// `--allow-fs-read` 收集;条目已按 node posix resolve 语义规范化(相对
    /// 路径 = 启动 cwd 解析,oracle:rel grant 对 resolve 后绝对路径判 true);
    /// `*` 原样保留 = node `--allow-fs-read=*` 通配全允许(oracle:对任意路径 true)。
    pub read_paths: ::std::vec::Vec<::std::string::String>,
    /// `--allow-fs-write` 收集(bao CLI 未接线该旗标,恒空 → has('fs.write',..)
    /// 恒 false;诚实边界:零授权如实呈现,非伪造)。
    pub write_paths: ::std::vec::Vec<::std::string::String>,
}

static CLI_PERMISSION_MODEL: ::std::sync::Mutex<::std::option::Option<CliPermissionModel>> =
    ::std::sync::Mutex::new(::std::option::Option::None);

/// 安装进程级权限模型(CLI dispatch 时一次;后写覆盖)。条目逐个规范化:
/// `*` 保留;cwd 不可得 → Err(fail-closed,CLI 报错退出,不静默降级)。
pub fn install_cli_permission_model(
    read_paths: ::std::vec::Vec<::std::string::String>,
    write_paths: ::std::vec::Vec<::std::string::String>,
) -> ::std::result::Result<(), ::std::string::String> {
    fn resolve_entries(
        entries: &[::std::string::String],
    ) -> ::std::result::Result<::std::vec::Vec<::std::string::String>, ::std::string::String> {
        let mut out = ::std::vec::Vec::with_capacity(entries.len());
        for entry in entries {
            if entry == "*" {
                out.push(entry.clone());
            } else {
                match crate::node_path::resolve_against_cwd(entry) {
                    ::std::option::Option::Some(resolved) => out.push(resolved),
                    ::std::option::Option::None => {
                        return ::std::result::Result::Err(::std::format!(
                            "--permission: cannot resolve grant path {:?} (cwd unavailable)",
                            entry
                        ));
                    }
                }
            }
        }
        ::std::result::Result::Ok(out)
    }

    let model = CliPermissionModel {
        read_paths: resolve_entries(&read_paths)?,
        write_paths: resolve_entries(&write_paths)?,
    };
    *CLI_PERMISSION_MODEL
        .lock()
        .expect("CLI permission model mutex poisoned") = ::std::option::Option::Some(model);
    ::std::result::Result::Ok(())
}

/// 模型在位?`populate_process_object` 据此决定 `process.permission` 是否定义
/// (node parity:无 `--permission` = undefined,oracle v24.19.0 钉死)。
pub fn cli_permission_model() -> ::std::option::Option<CliPermissionModel> {
    CLI_PERMISSION_MODEL
        .lock()
        .expect("CLI permission model mutex poisoned")
        .clone()
}

/// `process.permission.has(scope[, reference])` 判定核心(node v24.19.0 oracle):
/// - fs.read/fs.write:reference 缺省(None)→ false(oracle:有授权无 reference
///   仍 false);否则双边 resolve 规范化后边界包含 —— `ref == grant ||
///   ref.starts_with(grant + '/')`(oracle:'/tmp'→'/tmpx' false、'/tmp/' true、
///   '/tmp/../etc' false);`*` 全允许。
/// - child/worker/wasi/addons/inspector:bao 未接线任何授权旗标,恒 false(诚实)。
/// - 未知 scope:false(oracle:'fs.readXXX' → false,不抛)。
pub fn cli_permission_has(scope: &str, reference: ::std::option::Option<&str>) -> bool {
    let guard = CLI_PERMISSION_MODEL
        .lock()
        .expect("CLI permission model mutex poisoned");
    // 结构性不变量:JS `process.permission` 面只在模型在位时定义(同源读),
    // has() 可达即模型必在;缺位 = 不变量被破坏,崩塌显形优于静默答 false。
    let model = guard
        .as_ref()
        .expect("process.permission.has reachable but CLI permission model absent");
    match scope {
        "fs.read" => path_granted(&model.read_paths, reference),
        "fs.write" => path_granted(&model.write_paths, reference),
        // bao 无 --allow-child-process / --allow-worker / ... 授权旗标接线
        "child" | "worker" | "wasi" | "addons" | "inspector" => false,
        _ => false,
    }
}

/// fs 域路径包含判定:reference 规范化后对授权清单做边界包含
/// (exact or `grant/` 前缀;禁止裸 starts_with —— '/tmp' 不得覆盖 '/tmpx')。
fn path_granted(grants: &[::std::string::String], reference: ::std::option::Option<&str>) -> bool {
    let reference = match reference {
        ::std::option::Option::Some(r) => r,
        // node:有授权但无 reference → false(oracle v24.19.0 实测)
        ::std::option::Option::None => return false,
    };
    if grants.iter().any(|grant| grant == "*") {
        return true;
    }
    match crate::node_path::resolve_against_cwd(reference) {
        ::std::option::Option::Some(resolved) => grants.iter().any(|grant| {
            grant == "*" || resolved == grant.as_str() || resolved.starts_with(&format!("{grant}/"))
        }),
        // 规范化不可得(cwd 不可用)= 判定不可做 → fail-closed 拒绝
        ::std::option::Option::None => false,
    }
}

#[cfg(test)]
pub(crate) fn clear_cli_permission_model() {
    *CLI_PERMISSION_MODEL
        .lock()
        .expect("CLI permission model mutex poisoned") = ::std::option::Option::None;
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
        assert!(
            check_fs_read("/denied")
                .unwrap_err()
                .contains("read on /denied")
        );
        assert!(
            check_fs_write("/denied")
                .unwrap_err()
                .contains("write on /denied")
        );
        assert!(
            check_net("evil.com")
                .unwrap_err()
                .contains("net on evil.com")
        );
        assert_eq!(check_env().unwrap_err(), "Permission denied: env");
        assert_eq!(check_run().unwrap_err(), "Permission denied: run");
        cleanup();
    }

    #[test]
    fn test_fs_read_empty_path() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/tmp".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(
            check_fs_read("").is_err(),
            "empty path should not match /tmp prefix"
        );
        cleanup();
    }

    #[test]
    fn test_fs_read_exact_prefix_match() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/tmp".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(
            check_fs_read("/tmp").is_ok(),
            "exact prefix match should be allowed"
        );
        assert!(
            check_fs_read("/tmp/").is_ok(),
            "prefix + slash should be allowed"
        );
        cleanup();
    }

    #[test]
    fn test_net_empty_host() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["example.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(
            check_net("").is_err(),
            "empty host should not match any domain"
        );
        cleanup();
    }

    #[test]
    fn test_net_none_allows_all() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("any.host.com").is_ok());
        cleanup();
    }

    #[test]
    fn test_multiple_net_hosts() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["a.com".into(), "b.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("a.com").is_ok());
        assert!(check_net("b.com").is_ok());
        assert!(check_net("sub.a.com").is_ok());
        assert!(check_net("c.com").is_err());
        cleanup();
    }

    #[test]
    fn test_fs_write_multiple_prefixes() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: Some(vec!["/tmp".into(), "/home".into()]),
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_write("/tmp/out").is_ok());
        assert!(check_fs_write("/home/user/data").is_ok());
        assert!(check_fs_write("/var/log").is_err());
        cleanup();
    }

    // ─── PermissionCheck extended edge case tests ──────────────────
    // @trace REQ-LIB-004 [req:REQ-LIB-004] [level:unit]

    #[test]
    fn test_permission_check_debug_clone() {
        let check = PermissionCheck {
            read_paths: Some(vec!["/a".into()]),
            write_paths: Some(vec!["/b".into()]),
            net_hosts: Some(vec!["c.com".into()]),
            env_allowed: false,
            run_allowed: true,
        };
        let cloned = check.clone();
        assert_eq!(cloned.read_paths, check.read_paths);
        assert_eq!(cloned.write_paths, check.write_paths);
        assert_eq!(cloned.net_hosts, check.net_hosts);
        assert_eq!(cloned.env_allowed, check.env_allowed);
        assert_eq!(cloned.run_allowed, check.run_allowed);
        let debug = format!("{:?}", check);
        assert!(debug.contains("PermissionCheck"));
    }

    #[test]
    fn test_fs_read_empty_allowed_list_denies_all() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec![]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read("/any/path").is_err());
        cleanup();
    }

    #[test]
    fn test_fs_write_empty_allowed_list_denies_all() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: Some(vec![]),
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_write("/any/path").is_err());
        cleanup();
    }

    #[test]
    fn test_net_empty_allowed_list_denies_all() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec![]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("any.com").is_err());
        cleanup();
    }

    #[test]
    fn test_fs_read_path_traversal_prefix_match() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/allowed".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        // Prefix match: /allowed/../secret starts with /allowed
        assert!(check_fs_read("/allowed/../secret").is_ok());
        cleanup();
    }

    #[test]
    fn test_env_run_independent() {
        cleanup();
        // env=true, run=false
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: false,
        }));
        assert!(check_env().is_ok());
        assert!(check_run().is_err());
        // env=false, run=true
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: None,
            env_allowed: false,
            run_allowed: true,
        }));
        assert!(check_env().is_err());
        assert!(check_run().is_ok());
        cleanup();
    }

    #[test]
    fn test_set_permission_none_clears_all() {
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/only".into()]),
            write_paths: Some(vec!["/only".into()]),
            net_hosts: Some(vec!["only.com".into()]),
            env_allowed: false,
            run_allowed: false,
        }));
        set_permission(None);
        assert!(check_fs_read("/anything").is_ok());
        assert!(check_fs_write("/anything").is_ok());
        assert!(check_net("anything.com").is_ok());
        assert!(check_env().is_ok());
        assert!(check_run().is_ok());
    }

    #[test]
    fn test_net_unicode_domain() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["例え.jp".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net("例え.jp").is_ok());
        assert!(check_net("sub.例え.jp").is_ok());
        assert!(check_net("other.jp").is_err());
        cleanup();
    }

    #[test]
    fn test_fs_read_very_long_path() {
        cleanup();
        let long_path = format!("/tmp/{}", "a".repeat(1000));
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/tmp".into()]),
            write_paths: None,
            net_hosts: None,
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_fs_read(&long_path).is_ok());
        cleanup();
    }

    #[test]
    fn test_net_very_long_host() {
        cleanup();
        let long_host = "a".repeat(500) + ".example.com";
        set_permission(Some(PermissionCheck {
            read_paths: None,
            write_paths: None,
            net_hosts: Some(vec!["example.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        assert!(check_net(&long_host).is_ok());
        cleanup();
    }

    #[test]
    fn test_cross_field_independence() {
        cleanup();
        set_permission(Some(PermissionCheck {
            read_paths: Some(vec!["/read".into()]),
            write_paths: Some(vec!["/write".into()]),
            net_hosts: Some(vec!["net.com".into()]),
            env_allowed: true,
            run_allowed: true,
        }));
        // read path allowed for read, denied for write
        assert!(check_fs_read("/read/file").is_ok());
        assert!(check_fs_write("/read/file").is_err());
        // write path allowed for write, denied for read
        assert!(check_fs_write("/write/file").is_ok());
        assert!(check_fs_read("/write/file").is_err());
        cleanup();
    }

    // ─── CLI permission model (process.permission.has 判定核心) ──────────
    // @trace REQ-ENG-007 [criterion:7] [level:unit]
    // 进程级单槽跨测试共享:TEST_SER 串行化本组测试。

    static TEST_SER: ::std::sync::Mutex<()> = ::std::sync::Mutex::new(());

    fn install_model(read: &[&str], write: &[&str]) {
        let to_vec = |items: &[&str]| -> ::std::vec::Vec<::std::string::String> {
            items.iter().map(|s| (*s).to_string()).collect()
        };
        // 测试条目全部用绝对路径,绕开 resolve_against_cwd 的 cwd 依赖;
        // 相对条目 → cwd 的解析行为由 node_path tests 覆盖。
        install_cli_permission_model(to_vec(read), to_vec(write)).expect("install");
    }

    #[test]
    fn test_cli_permission_has_containment_boundaries() {
        let _ser = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        install_model(&["/tmp"], &[]);
        // oracle v24.19.0 逐格:grant /tmp。
        assert!(cli_permission_has("fs.read", Some("/tmp")));
        assert!(cli_permission_has("fs.read", Some("/tmp/")));
        assert!(cli_permission_has("fs.read", Some("/tmp/sub/deep")));
        // 前缀必须落在路径边界:'/tmpx' 不被 '/tmp' 覆盖。
        assert!(!cli_permission_has("fs.read", Some("/tmpx")));
        assert!(!cli_permission_has("fs.read", Some("/etc")));
        // 越根 `..` 规范化后判包含:'/tmp/../etc' → '/etc' → false。
        assert!(!cli_permission_has("fs.read", Some("/tmp/../etc")));
        // 有授权但 reference 缺省 → false。
        assert!(!cli_permission_has("fs.read", None));
        // write 域零授权 → false(诚实边界)。
        assert!(!cli_permission_has("fs.write", Some("/tmp")));
        assert!(!cli_permission_has("fs.write", None));
        clear_cli_permission_model();
    }

    #[test]
    fn test_cli_permission_has_wildcard_and_non_fs_scopes() {
        let _ser = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        install_model(&["*"], &[]);
        assert!(cli_permission_has("fs.read", Some("/etc/passwd")));
        assert!(cli_permission_has("fs.read", Some("/")));
        clear_cli_permission_model();

        // 非 fs 域:bao 未接线授权旗标,恒 false(oracle:'child' 无旗标 → false)。
        install_model(&["/tmp"], &[]);
        assert!(!cli_permission_has("child", None));
        assert!(!cli_permission_has("worker", Some("/tmp")));
        assert!(!cli_permission_has("wasi", None));
        assert!(!cli_permission_has("addons", None));
        assert!(!cli_permission_has("inspector", None));
        // 未知 scope → false,不抛(oracle:'fs.readXXX' → false)。
        assert!(!cli_permission_has("fs.readXXX", Some("/tmp")));
        clear_cli_permission_model();
    }

    #[test]
    fn test_cli_permission_install_resolves_and_fails_closed() {
        let _ser = TEST_SER.lock().unwrap_or_else(|e| e.into_inner());
        // 相对条目 → 启动 cwd 解析(oracle:rel grant 对 resolve 后绝对路径判 true)。
        let cwd = ::std::env::current_dir().expect("cwd");
        let cwd = cwd.to_string_lossy().into_owned();
        install_model(&["docs", "/abs"], &[]);
        let model = cli_permission_model().expect("model");
        assert_eq!(
            model.read_paths,
            ::std::vec![::std::format!("{cwd}/docs"), "/abs".to_string()]
        );
        // '*' 原样保留(通配哨兵,不做路径解析)。
        install_model(&["*"], &[]);
        assert_eq!(
            cli_permission_model().expect("model").read_paths,
            ::std::vec!["*".to_string()]
        );
        assert!(cli_permission_has("fs.read", Some("/any/where")));
        clear_cli_permission_model();
    }
}
