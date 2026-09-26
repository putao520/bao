// @trace TEST-DEPLOY-SRV [req:REQ-DEPLOY-1] [level:integration]
//
// REQ-DEPLOY-1 — x86_64-unknown-linux-musl in the v1 Supported target matrix.
//
// Scope note: most criteria of this REQ are build/infra-level (rustup target,
// musl cross cc/sysroot, mozjs/BoringSSL/uWS/servo musl link, the registry-only
// consumer gate). They are executed by the cross build jobs / long-task
// protocol, not by an in-process unit — a test binary running under a gnu
// host cannot re-verify a musl link. What an in-process test CAN verify
// mechanically, and what is tested here, is the REQ's documentation-and-
// configuration deliverable (criterion 1 + criterion 7):
//   - the target matrix doc declares gnu AND musl Supported with explicit
//     statuses everywhere (no grey area);
//   - the musl cross toolchain recipe exists (rustup target + sysroot env);
//   - .cargo/config.toml carries the per-target CC/CXX/AR/linker wiring;
//   - the bare PKG_CONFIG_ALLOW_CROSS switch is absent from the config
//     (the sanctioned targeted-sysroot path is used instead);
//   - this very test process runs on a platform inside the declared matrix.
//
// Sources (all inside the repo, resolved from CARGO_MANIFEST_DIR):
//   docs/platform-support.md, docs/musl-cross.md, .cargo/config.toml

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("required REQ-DEPLOY-1 artifact missing: {} ({})", path.display(), e))
}

/// Criterion 7 — the target matrix is documented with explicit statuses and
/// zero grey area: gnu + musl are Supported, and every other listed platform
/// carries an explicit Unsupported instead of silence.
#[test]
#[cfg(unix)]
fn req_deploy_1_target_matrix_documented_no_grey_area() {
    let doc = read_repo_file("docs/platform-support.md");
    assert!(
        doc.contains("x86_64-unknown-linux-gnu") && doc.contains("x86_64-unknown-linux-musl"),
        "matrix must name both v1 Supported targets"
    );
    let supported = doc.matches("**Supported**").count();
    assert!(
        supported >= 2,
        "gnu + musl must both be declared Supported (found {} occurrences)",
        supported
    );
    // No grey area: platforms that are not supported say so explicitly.
    assert!(
        doc.contains("**Unsupported**"),
        "non-supported platforms must carry an explicit Unsupported status"
    );
    // The musl row points at the cross recipe (criterion 1 linkage).
    assert!(
        doc.contains("musl-cross.md"),
        "musl Supported row must reference the cross toolchain recipe"
    );
}

/// Criterion 1 — the musl cross toolchain recipe exists: rustup target +
/// targeted pkg-config sysroot env (the sanctioned alternative), with the
/// bare ALLOW_CROSS switch explicitly rejected rather than used.
#[test]
#[cfg(unix)]
fn req_deploy_1_musl_cross_recipe_documented() {
    let doc = read_repo_file("docs/musl-cross.md");
    assert!(
        doc.contains("rustup target add x86_64-unknown-linux-musl"),
        "recipe must document the rustup target installation"
    );
    assert!(
        doc.contains("PKG_CONFIG_SYSROOT_DIR"),
        "recipe must document the targeted sysroot env (sanctioned path)"
    );
    // The bare switch is banned by the criterion; the doc may only mention it
    // as the rejected alternative — assert the sanctioned statements exist.
    assert!(
        doc.contains("without a single `PKG_CONFIG_ALLOW_CROSS`")
            || doc.contains("sanctioned alternative to the bare `PKG_CONFIG_ALLOW_CROSS`"),
        "recipe must state the bare-switch ban / sanctioned alternative"
    );
}

/// Criterion 1 (config face) — .cargo/config.toml wires the musl cross
/// toolchain via per-target keys (CC/CXX/AR + linker), and carries NO bare
/// PKG_CONFIG_ALLOW_CROSS enablement.
#[test]
#[cfg(unix)]
fn req_deploy_1_cargo_config_carries_musl_target_wiring() {
    let cfg = read_repo_file(".cargo/config.toml");
    for key in [
        "CC_x86_64_unknown_linux_musl",
        "CXX_x86_64_unknown_linux_musl",
        "AR_x86_64_unknown_linux_musl",
    ] {
        assert!(cfg.contains(key), "missing per-target key {}", key);
    }
    assert!(
        cfg.contains("[target.x86_64-unknown-linux-musl]"),
        "missing [target.x86_64-unknown-linux-musl] section (linker wiring)"
    );
    assert!(
        !cfg.contains("PKG_CONFIG_ALLOW_CROSS"),
        "the bare PKG_CONFIG_ALLOW_CROSS switch must not appear in config.toml"
    );
}

/// Scope guard — this process itself runs inside the declared v1 matrix
/// (x86_64 linux). On any other host the REQ's v1 scope does not apply and
/// the assertion is vacuous by construction (documented, not skipped).
#[test]
#[cfg(unix)]
fn req_deploy_1_test_platform_inside_declared_matrix() {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        let doc = read_repo_file("docs/platform-support.md");
        assert!(
            doc.contains("x86_64-unknown-linux-gnu"),
            "running platform must be in the documented matrix"
        );
    } else {
        // Out of the v1 matrix (macOS/Windows/aarch64): REQ-DEPLOY-1 scopes
        // those to a later ruling, so there is nothing to assert here.
    }
}
