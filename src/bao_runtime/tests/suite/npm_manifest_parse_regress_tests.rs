// @trace TEST-CLI-001-NPM-MANIFEST-PARSE [req:REQ-CLI-001] [level:unit]
// Upstream bun install manifest-parser regressions, re-expressed on the
// Rust surface (upstream test/cli/install/bun-install-registry.test.ts and
// bun-install.test.ts are not absorbed; their assertions run against
// `PackageManifest::parse`, the registry-manifest parser both waves fixed).
//
// Hosted in the bun_runtime suite because bun_install's own test binary
// cannot link (see bun_install::parse_text_lockfile_for_tests docs).

use bun_install::Npm::PackageManifest;
use bun_install::npm::registry::Scope;
use bun_install::bin;

/// `directories.bin` path longer than any string-buffer slack: "./"
/// repeated 256 times then the folder name (same shape and scale as the
/// upstream tests).
fn long_bin_dir() -> String {
    let mut dir = String::with_capacity(523);
    for _ in 0..256 {
        dir.push_str("./");
    }
    dir.push_str("bins");
    dir
}

fn parse_packument(json: &str) -> PackageManifest {
    let mut log = bun_ast::Log::default();
    let scope = Scope::default();
    PackageManifest::parse(
        &scope,
        &mut log,
        json.as_bytes(),
        b"evil",
        b"",
        b"",
        0,
        false,
    )
    .expect("parse call succeeds")
    .expect("manifest parses")
}

/// Upstream bun 0ffd5b2d38 "install: count directories.bin when bin is an
/// empty string (#43145)" — the counting pass stopped at the empty `bin`
/// string and reserved nothing, so the build pass's `directories.bin`
/// append ran past the end of the string buffer
/// (`range end index 516 out of range for slice of length 0` abort on every
/// build, release included). Both parsers that size a buffer from a
/// counting pass had the mismatch; this exercises the registry-manifest
/// arm (`PackageManifest::parse`).
mod bin_empty_string_directories {
    use super::*;

    #[test]
    fn empty_bin_string_falls_through_to_directories_bin() {
        let dir = long_bin_dir();
        let packument = format!(
            r#"{{
                "name": "evil",
                "versions": {{
                    "1.0.0": {{
                        "name": "evil",
                        "version": "1.0.0",
                        "bin": "",
                        "directories": {{ "bin": {dir:?} }},
                        "dist": {{ "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }}
                    }}
                }},
                "dist-tags": {{ "latest": "1.0.0" }}
            }}"#
        );
        // Pre-fix this aborted the process (counting/build pass mismatch);
        // now the directories.bin string is counted and appended.
        let manifest = parse_packument(&packument);
        assert_eq!(manifest.package_versions.len(), 1);
        let version = &manifest.package_versions[0];
        assert_eq!(version.bin.tag, bin::Tag::Dir);
        // SAFETY: tag is Dir, the active union arm is `dir`.
        let stored = unsafe { version.bin.value.dir };
        assert_eq!(
            stored.slice(&manifest.string_buf),
            dir.as_bytes(),
            "directories.bin survives the two-pass parse byte-for-byte"
        );
    }

    #[test]
    fn non_empty_bin_string_still_wins_over_directories() {
        let dir = long_bin_dir();
        let packument = format!(
            r#"{{
                "name": "evil",
                "versions": {{
                    "1.0.0": {{
                        "name": "evil",
                        "version": "1.0.0",
                        "bin": "./cli.js",
                        "directories": {{ "bin": {dir:?} }},
                        "dist": {{ "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }}
                    }}
                }},
                "dist-tags": {{ "latest": "1.0.0" }}
            }}"#
        );
        let manifest = parse_packument(&packument);
        let version = &manifest.package_versions[0];
        assert_eq!(version.bin.tag, bin::Tag::File);
        // SAFETY: tag is File, the active union arm is `file`.
        let stored = unsafe { version.bin.value.file };
        assert_eq!(stored.slice(&manifest.string_buf), b"./cli.js");
    }
}

/// Upstream bun 8ce165a1e8 "install: stop asserting on registry manifest
/// contents in debug builds (#43035)" — debug-only checks on remote data
/// aborted `bun install` on debug builds where the release path next to each
/// check already handles the case. Assertions from
/// test/cli/install/bun-install.test.ts ("unexpected shape" tests), running
/// here on a debug-assertions build (cargo test dev profile) so each removed
/// `debug_assert!` / `unreachable!` is exercised:
///
/// - a non-string dependency value (null) is skipped, not
///   `unreachable!("non-value Expr from JSON parser")`;
/// - a `versions` key that is not a version ("not-a-version") is skipped
///   with at most a verbose warning, not `debug_assert!(parsed_version.valid)`
///   nor a hard log error that fails the install;
/// - equal-version neighbours ("1.0.0" and "01.0.0") sort without
///   `debug_assert!(order == Ordering::Greater)`.
mod unexpected_manifest_shapes {
    use super::*;

    #[test]
    fn non_string_dependency_value_is_skipped() {
        let packument = r#"{
            "name": "evil",
            "versions": {
                "1.0.0": {
                    "name": "evil",
                    "version": "1.0.0",
                    "dependencies": { "good": null, "ok": "1.0.1" },
                    "dist": { "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }
                }
            },
            "dist-tags": { "latest": "1.0.0" }
        }"#;
        let manifest = parse_packument(packument);
        assert_eq!(manifest.package_versions.len(), 1);
        let version = &manifest.package_versions[0];
        let names: Vec<&[u8]> = version
            .dependencies
            .name
            .get(&manifest.external_strings)
            .iter()
            .map(|s| s.slice(&manifest.string_buf))
            .collect();
        assert_eq!(
            names,
            vec![b"ok".as_slice()],
            "the null dependency value is skipped; the string one is kept"
        );
    }

    #[test]
    fn non_version_versions_key_is_skipped_silently() {
        let packument = r#"{
            "name": "evil",
            "versions": {
                "not-a-version": { "name": "evil" },
                "1.0.0": {
                    "name": "evil",
                    "version": "1.0.0",
                    "dist": { "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }
                }
            },
            "dist-tags": { "latest": "1.0.0" }
        }"#;
        // Pre-fix: debug_assert!(parsed_version.valid) aborted here; the
        // error-log form also failed installs on release. Post-fix the key
        // is skipped (npm parity: npm skips such keys in silence).
        let manifest = parse_packument(packument);
        assert_eq!(
            manifest.package_versions.len(),
            1,
            "only the valid 1.0.0 version is kept"
        );
    }

    #[test]
    fn equal_neighbour_versions_do_not_assert_on_sort_order() {
        let packument = r#"{
            "name": "evil",
            "versions": {
                "1.0.0": {
                    "name": "evil",
                    "version": "1.0.0",
                    "dist": { "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }
                },
                "01.0.0": {
                    "name": "evil",
                    "version": "1.0.0",
                    "dist": { "tarball": "http://127.0.0.1/reg/evil/-/evil-1.0.0.tgz" }
                }
            },
            "dist-tags": { "latest": "1.0.0" }
        }"#;
        // Two keys can name one version; neighbours can compare Equal, which
        // the old `debug_assert!(order == Ordering::Greater)` rejected.
        let manifest = parse_packument(packument);
        assert!(!manifest.package_versions.is_empty());
    }

    #[test]
    fn missing_tarball_url_parses_at_manifest_level() {
        // Upstream: `"dist": {}` (and friends) used to abort via
        // `tarball_url is empty for package evil@1.0.0` debug panic in
        // Package::from_npm; the manifest parse itself must accept the
        // shape (the default-URL fallback is from_npm's release path).
        let packument = r#"{
            "name": "evil",
            "versions": {
                "1.0.0": { "name": "evil", "version": "1.0.0", "dist": {} }
            },
            "dist-tags": { "latest": "1.0.0" }
        }"#;
        let manifest = parse_packument(packument);
        assert_eq!(manifest.package_versions.len(), 1);
    }
}
