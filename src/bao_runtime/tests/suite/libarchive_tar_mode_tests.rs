// Tar mode-preservation assertions exercise POSIX permission bits
// (std::os::unix + bun_libarchive::directory_mode) — unix-only surface.
#![cfg(unix)]

// @trace TEST-CLI-001-TAR-DIR-MODE [req:REQ-CLI-001] [level:unit]
// Upstream bun 5c26a6cca5 "libarchive: mask tar directory modes instead of
// narrowing them through i32 (#41742)" — regression assertions from
// test/js/bun/archive.test.ts, re-expressed on the Rust surface (the JS
// binding surface src/runtime/api/Archive.rs is not hosted in bao; the
// install arm `TarballStream::make_directory` shares the same
// `directory_mode()` helper):
//
// 1. A tar directory entry whose GNU base-256 mode field encodes 2^31 or
//    more must not abort extraction (`i32::try_from(entry.perm())` used to
//    panic with TryFromIntError(PosOverflow) on Linux, killing
//    `Bun.Archive#extract()`, `bun create <owner>/<repo>`, and `bun install`
//    of a `github:` dependency). `directory_mode()` masks the permission to
//    0o7777 so every representable mode reaches the kernel unchanged.
// 2. node-tar mode-fix (https://github.com/npm/node-tar/blob/main/lib/mode-fix.js):
//    a readable directory becomes listable (0o400 -> 0o500 on disk).
//
// The tar bytes follow the upstream JS repro: ustar header for "d/" with
// base-256 mode bytes [0x80, 0, 0, 0, 0x80, 0, 0, 0] (low 32 bits = 2^31),
// size 0, mtime 0, typeflag '5', plus two zero end-of-archive blocks.
//
// Hosted in the bao_runtime suite (not bun_libarchive's own test binary)
// because `bun_core`'s upward-resolved `__bun_crash_handler_dump_stack_trace`
// needs the full-stack link line — same reason bun_install parser tests live
// here (see bun_install::parse_text_lockfile_for_tests docs).

use std::os::unix::fs::PermissionsExt as _;

use bun_libarchive::{Archiver, ExtractOptions};
use bun_sys::FdExt;

/// One ustar header for a directory `d/` with the given raw 8-byte mode
/// field, followed by the two zero blocks that end a tar stream.
fn dir_tar_with_mode_field(mode_field: &[u8; 8]) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    h[..2].copy_from_slice(b"d/");
    h[100..108].copy_from_slice(mode_field);
    // size = 0, mtime = 0 (11 octal digits + NUL each)
    h[124..136].copy_from_slice(b"00000000000\0");
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = b'5'; // typeflag: directory
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // checksum: spaces while summing, then 6 octal digits + NUL + space
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let chk = format!("{:06o}\0 ", sum);
    h[148..156].copy_from_slice(chk.as_bytes());
    let mut buf = h;
    buf.extend_from_slice(&[0u8; 1024]);
    buf
}

/// GNU base-256 mode field encoding `value` (first byte high bit set,
/// remaining 7 bytes big-endian) — same encoding class as the upstream
/// repro, for values that fit in i32.
fn base256_mode_field(value: u64) -> [u8; 8] {
    let mut f = [0u8; 8];
    f[0] = 0x80;
    f[1..].copy_from_slice(&value.to_be_bytes()[1..]);
    f
}

/// Best-effort recursive delete that survives permission-less directories
/// the extraction created on purpose (a mode-0 `d/` blocks plain
/// `remove_dir_all`, which needs r+x to enumerate before unlinking).
fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::metadata(dir).and_then(|m| {
        let mut perms = m.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// Extracts `tar_bytes` into a fresh per-`name` temp dir and returns
/// (count, dir path). Each call gets its own directory: a previous
/// extraction's mode-0 entry must not shadow this one's mkdir (EEXIST
/// would leave the stale directory in place).
fn extract(name: &str, tar_bytes: &[u8]) -> (u32, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "bao-libarchive-dir-mode-{}-{name}",
        std::process::id()
    ));
    cleanup(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let fd =
        bun_sys::open_dir_absolute(dir.to_str().expect("temp dir is utf-8").as_bytes())
            .expect("open temp dir");
    let count = Archiver::extract_to_dir(
        tar_bytes,
        fd,
        None,
        &mut (),
        ExtractOptions {
            depth_to_skip: 0,
            ..Default::default()
        },
    )
    .expect("extract_to_dir succeeds");
    fd.close();
    (count, dir)
}

#[test]
fn directory_mode_masks_base256_mode_above_i31() {
    // 2^31 exactly (i32::try_from used to fail) and 2^32-1 (used to
    // narrow-wrap) both mask into 0o7777 with no panic.
    assert_eq!(bun_libarchive::directory_mode(0x8000_0000), 0, "2^31 masks to mode 0");
    assert_eq!(bun_libarchive::directory_mode(u32::MAX), 0o7777);
}

#[test]
fn directory_mode_applies_node_tar_mode_fix() {
    // readable => listable, per node-tar mode-fix.js
    assert_eq!(bun_libarchive::directory_mode(0o400), 0o500);
    assert_eq!(bun_libarchive::directory_mode(0o40), 0o50);
    assert_eq!(bun_libarchive::directory_mode(0o4), 0o5);
    // already fully permissioned bits stay (0o644 -> 0o755)
    assert_eq!(bun_libarchive::directory_mode(0o644), 0o755);
}

#[test]
fn extract_base256_directory_mode_does_not_panic() {
    // Upstream repro bytes: base-256 mode whose low 32 bits are 2^31.
    // Before the fix this aborted the process with
    // `int cast: TryFromIntError(PosOverflow)`; now it extracts one
    // directory entry and returns count 1.
    let tar = dir_tar_with_mode_field(&[0x80, 0, 0, 0, 0x80, 0, 0, 0]);
    let (count, dir) = extract("base256-2-31", &tar);
    assert_eq!(count, 1, "exactly the one directory entry is extracted");
    assert!(
        dir.join("d").is_dir(),
        "the directory entry is created on disk"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_readable_directory_becomes_listable() {
    // mode 0o400 on disk must gain the execute bit (0o500), the node-tar
    // mode-fix both extraction sites share.
    let tar = dir_tar_with_mode_field(&base256_mode_field(0o400));
    let (count, dir) = extract("mode-fix-0400", &tar);
    assert_eq!(count, 1);
    let mode = std::fs::metadata(dir.join("d"))
        .expect("created dir stat")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o500,
        0o500,
        "readable directory is listable after mode-fix (raw mode {mode:o})"
    );
    cleanup(&dir);
}
