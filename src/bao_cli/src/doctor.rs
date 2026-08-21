// `bao doctor` — environment diagnostics for the Bao native stack.
//
// A programmable browser runtime built on Servo + SpiderMonkey has a heavy
// native toolchain (nightly Rust, clang/C++, mozjs compiled from source) plus
// a system-library link surface: the Bao-layer *_sys build scripts link
// libc-ares/zlib/libarchive/libdeflate, and the Servo stack links
// fontconfig/freetype/glib/GStreamer. `bao doctor` walks the environment and
// reports what's present/missing so a new contributor can see at a glance why
// their build fails — without having to understand the whole monorepo.
//
// Note on TLS: Bao vendors BoringSSL and compiles it from source — the system
// OpenSSL/libssl-dev is NOT a build dependency. The lib checks below mirror
// the direct DT_NEEDED entries of the built `bao` binary (readelf ground
// truth), each probed via its pkg-config module.

use std::process::Command;
use std::time::Duration;

/// One check result. `ok` is the binary pass/fail; `detail` carries version
/// strings or a fix hint.
struct Check {
    label: &'static str,
    ok: bool,
    detail: String,
}

pub fn run() -> Result<(), i32> {
    let version = env!("CARGO_PKG_VERSION");
    println!("Bao doctor v{}\n", version);

    let checks = vec![
        rustc_check(),
        cargo_check(),
        clang_check(),
        clang_toolchain_check(),
        pkg_config_check(),
        system_lib_check("libc-ares", "libcares", "cares_sys", "libc-ares-dev"),
        system_lib_check("zlib", "zlib", "lsquic_sys/uws_sys", "zlib1g-dev"),
        system_lib_check("libarchive", "libarchive", "libarchive", "libarchive-dev"),
        system_lib_check("libdeflate", "libdeflate", "uws_sys", "libdeflate-dev"),
        system_lib_check("fontconfig", "fontconfig", "servo font stack", "libfontconfig-dev"),
        system_lib_check("freetype", "freetype2", "servo font stack", "libfreetype-dev"),
        system_lib_check("glib-2.0", "glib-2.0", "servo (glib)", "libglib2.0-dev"),
        system_lib_check("GStreamer core", "gstreamer-1.0", "servo media stack", "libgstreamer1.0-dev"),
        system_lib_check(
            "GStreamer plugins-base",
            "gstreamer-plugins-base-1.0",
            "servo media stack",
            "libgstreamer-plugins-base1.0-dev",
        ),
        system_lib_check("GStreamer GL", "gstreamer-gl-1.0", "servo media stack", "libgstreamer-plugins-base1.0-dev"),
        system_lib_check("GStreamer webrtc", "gstreamer-webrtc-1.0", "servo media stack", "libgstreamer-plugins-bad1.0-dev"),
        system_lib_check("GStreamer play", "gstreamer-play-1.0", "servo media stack", "libgstreamer-plugins-bad1.0-dev"),
        display_check(),
        mozjs_artifact_check(),
        cdp_port_check(),
    ];

    let mut all_ok = true;
    let width = checks.iter().map(|c| c.label.len()).max().unwrap_or(8);
    for c in &checks {
        let mark = if c.ok { "✓" } else { "✗" };
        println!(
            "  {} {:<width$}  {}",
            mark,
            c.label,
            c.detail,
            width = width
        );
        if !c.ok {
            all_ok = false;
        }
    }

    println!();
    if all_ok {
        println!("All checks passed. Bao should build and run on this machine.");
        Ok(())
    } else {
        println!("Some checks failed — see the ✗ lines above for fix hints.");
        // Doctor itself is informational; never hard-fail the process so it can
        // be piped into a bug report.
        Ok(())
    }
}

/// Run `cmd --version` (or equivalent) and return the first stdout line,
/// trimmed. `None` if the binary is absent or the command fails.
fn version_line(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
}

fn rustc_check() -> Check {
    match version_line("rustc", &["--version"]) {
        Some(v) => {
            // Bao requires nightly. rustc prints e.g. "rustc 1.90.0-nightly (...)".
            let is_nightly = v.contains("nightly");
            Check {
                label: "Rust (rustc)",
                ok: is_nightly,
                detail: if is_nightly {
                    v
                } else {
                    format!("{}  ← Bao requires nightly; run: rustup default nightly", v)
                },
            }
        }
        None => Check {
            label: "Rust (rustc)",
            ok: false,
            detail: "not found — install via https://rustup.rs".into(),
        },
    }
}

fn cargo_check() -> Check {
    match version_line("cargo", &["--version"]) {
        Some(v) => Check {
            label: "Cargo",
            ok: true,
            detail: v,
        },
        None => Check {
            label: "Cargo",
            ok: false,
            detail: "not found — ships with rustup".into(),
        },
    }
}

fn clang_check() -> Check {
    // mozjs compiles SpiderMonkey (C++) from source; a C/C++ compiler is mandatory.
    for c in ["clang", "gcc", "cc"] {
        if let Some(v) = version_line(c, &["--version"]) {
            return Check {
                label: "C/C++ compiler",
                ok: true,
                detail: format!("{}: {}", c, first_token(&v)),
            };
        }
    }
    Check {
        label: "C/C++ compiler",
        ok: false,
        detail: "none of clang/gcc/cc found — mozjs needs a C++ compiler".into(),
    }
}

fn pkg_config_check() -> Check {
    match version_line("pkg-config", &["--version"]) {
        Some(v) => Check {
            label: "pkg-config",
            ok: true,
            detail: v,
        },
        None => Check {
            label: "pkg-config",
            ok: false,
            detail: "not found — install pkg-config (apt: pkg-config)".into(),
        },
    }
}

/// Check one system library on the native link surface. The Bao layer emits
/// `cargo:rustc-link-lib=<lib>` from the *_sys build scripts (cares/z/
/// archive/deflate); the Servo stack links its system deps (fontconfig/
/// freetype/glib/GStreamer) via pkg-config in its own build. Either way the
/// -dev package (headers + unversioned .so + .pc) must be installed for the
/// final binary to link. `pkg-config --modversion` resolves exactly when that
/// is true — the same probe a fresh build would rely on.
fn system_lib_check(label: &'static str, pc_name: &str, linked_by: &str, apt_pkg: &str) -> Check {
    match version_line("pkg-config", &["--modversion", pc_name]) {
        Some(v) => Check {
            label,
            ok: true,
            detail: v,
        },
        None if version_line("pkg-config", &["--version"]).is_none() => Check {
            // The probe itself is gone — report unverifiable instead of
            // guessing "missing" (the pkg-config check above is the fix).
            label,
            ok: false,
            detail: "unverifiable — install pkg-config first".into(),
        },
        None => Check {
            label,
            ok: false,
            detail: format!("not found — linked by {linked_by} (apt: {apt_pkg})"),
        },
    }
}

fn clang_toolchain_check() -> Check {
    // The vendored C/C++ builds hardcode the compiler: boringssl_sys and
    // uws_sys use `clang++`, lsquic_sys and uws_sys use `clang`. gcc cannot
    // substitute (the generic C/C++ check above only covers mozjs).
    let c = version_line("clang", &["--version"]);
    let cpp = version_line("clang++", &["--version"]);
    let label = "Clang (vendored builds)";
    match (&c, &cpp) {
        (Some(cv), Some(_)) => Check {
            label,
            ok: true,
            detail: format!("clang {} + clang++", version_token(cv)),
        },
        (Some(cv), None) => Check {
            label,
            ok: false,
            detail: format!(
                "clang {} present, clang++ missing — boringssl/uws need both (apt: clang)",
                version_token(cv)
            ),
        },
        (None, _) => Check {
            label,
            ok: false,
            detail: "clang not found — boringssl/lsquic/uws build scripts require it (apt: clang)"
                .into(),
        },
    }
}

/// Version token from a `clang --version` line ("Ubuntu clang version
/// 14.0.0-1ubuntu1" → "14.0.0-1ubuntu1"); falls back to the first token.
fn version_token(line: &str) -> String {
    line.split_once("version")
        .map(|(_, rest)| {
            rest.trim()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| first_token(line))
}

fn display_check() -> Check {
    // Servo/WebRender need a DISPLAY (headless via Xvfb is fine). No DISPLAY
    // is not fatal for a pure `bao run` script, but blocks browser rendering.
    let has_display = std::env::var_os("DISPLAY").is_some();
    Check {
        label: "DISPLAY (rendering)",
        ok: has_display,
        detail: if has_display {
            std::env::var("DISPLAY").unwrap_or_default()
        } else {
            "unset — browser rendering needs X (or Xvfb :99 for headless)".into()
        },
    }
}

fn mozjs_artifact_check() -> Check {
    // If target/ has a compiled libmozjs_sys rlib, the first build (the slow
    // part) is already done. This is just a hint, not a requirement.
    let found = walk_target_for_mozjs();
    Check {
        label: "SpiderMonkey (built)",
        ok: true, // informational only — absence is not a failure
        detail: if found {
            "libmozjs_sys artifact present (incremental build will be fast)".into()
        } else {
            "no cached artifact — first build will compile SpiderMonkey (slow)".into()
        },
    }
}

/// Scan target/ for any compiled mozjs artifact. Best-effort, non-recursive
/// beyond a shallow check — we only want a presence hint.
fn walk_target_for_mozjs() -> bool {
    let Ok(entries) = std::fs::read_dir("target") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_mentions_mozjs(&path.join("debug")) {
                return true;
            }
            if dir_mentions_mozjs(&path.join("release")) {
                return true;
            }
        }
    }
    false
}

fn dir_mentions_mozjs(dir: &std::path::Path) -> bool {
    // Look in deps/ and incremental/ shallowly for a mozjs-named file.
    for sub in ["deps", "incremental"] {
        let p = dir.join(sub);
        if let Ok(it) = std::fs::read_dir(&p) {
            for e in it.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.contains("mozjs") {
                    return true;
                }
            }
        }
    }
    false
}

fn cdp_port_check() -> Check {
    // If something answers on 9222, a Bao (or Chrome) CDP server is likely
    // already running. Connect-only, short timeout — never start a server here.
    let addr = "127.0.0.1:9222";
    match std::net::TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(300)) {
        Ok(_) => Check {
            label: "CDP :9222",
            ok: true,
            detail: "reachable — a CDP server is already listening".into(),
        },
        Err(_) => Check {
            label: "CDP :9222",
            ok: true, // not listening is the normal state, not a failure
            detail: "not listening (normal unless `bao browser` is running)".into(),
        },
    }
}

/// First whitespace-delimited token of a version string (drops the long
/// commit hash / target triple noise).
fn first_token(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}
