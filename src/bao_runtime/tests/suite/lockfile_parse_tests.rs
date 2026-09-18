//! Text-lockfile parser tests that must run outside `bun_install` because
//! that crate's own test binary cannot link (upward-resolved `__bun_regex_*`
//! symbols live in `bao_runtime`). Entry point is the `#[doc(hidden)]`
//! `bun_install::parse_text_lockfile_for_tests`.

/// Keep the link-time providers in `bao_runtime` on the link line: this test
/// links `bun_install`, whose `NodeLinker` declares those upward-resolved
/// symbols, and without a live reference the linker drops the providers from
/// the runtime rlib.
#[inline(never)]
fn force_link_runtime_providers() {
    bun_runtime::product_native_symbols::force_link_product_native_symbols();
}

/// Upstream 44acc3d61: a `bun.lock` that lists workspaces but has no
/// `"packages"` key must parse exactly like `"packages": {}`. Pre-fix the
/// parser returned early after appending the root + workspace packages,
/// skipping the resolution pass — so `buffers.resolutions` stayed empty while
/// the root package already claimed a resolutions slice for its workspace
/// dependencies, and `Package::Diff::generate` indexed the empty buffer.
mod workspaces_without_packages {
    use super::force_link_runtime_providers;
    use bun_install::parse_text_lockfile_for_tests as parse_lock;

    #[test]
    fn parses_like_empty_packages() {
        force_link_runtime_providers();
        // Reproduction from the upstream report: root "z" + workspace "w",
        // no "packages" key. Pre-fix this parse "succeeded" but left
        // `buffers.resolutions` empty against a non-empty `dependencies`.
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": {
                    "": { "name": "z" },
                    "pkgs/w": { "name": "w", "version": "1.0.0" }
                }
            }"#,
        )
        .unwrap();

        // The resolution pass ran: both flat buffers are sized in lockstep…
        assert!(!lockfile.buffers.dependencies.is_empty());
        assert_eq!(
            lockfile.buffers.resolutions.len(),
            lockfile.buffers.dependencies.len(),
            "missing \"packages\" must not skip the resolution pass"
        );

        // …and the root's workspace dependency is bound to the workspace
        // package (id 1), instead of slicing an empty buffer.
        let root = lockfile.packages.get(0);
        let resolutions = root.resolutions.get(&lockfile.buffers.resolutions);
        assert_eq!(resolutions, &[1]);
    }

    #[test]
    fn empty_packages_object_is_equivalent() {
        // `"packages": {}` and a missing `"packages"` must yield identical
        // buffer shapes.
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": {
                    "": { "name": "z" },
                    "pkgs/w": { "name": "w", "version": "1.0.0" }
                },
                "packages": {}
            }"#,
        )
        .unwrap();
        assert_eq!(
            lockfile.buffers.resolutions.len(),
            lockfile.buffers.dependencies.len()
        );
        let root = lockfile.packages.get(0);
        assert_eq!(root.resolutions.get(&lockfile.buffers.resolutions), &[1]);
    }

    #[test]
    fn root_only_workspace_without_packages_loads_empty() {
        // No workspace members ⇒ the root has no dependencies, and both flat
        // buffers stay empty (Bao's parser requires a `workspaces` object,
        // unlike upstream, so the truly key-less lockfile is out of scope).
        let lockfile = parse_lock(
            r#"{ "lockfileVersion": 1, "workspaces": { "": { "name": "z" } } }"#,
        )
        .unwrap();
        assert!(lockfile.buffers.dependencies.is_empty());
        assert!(lockfile.buffers.resolutions.is_empty());
    }

    #[test]
    fn non_object_packages_still_errors() {
        let err = parse_lock(
            r#"{ "lockfileVersion": 1, "workspaces": { "": { "name": "z" } }, "packages": 3 }"#,
        )
        .err()
        .expect("non-object \"packages\" must be rejected");
        assert!(matches!(
            err,
            bun_install::TextLockfile::ParseError::InvalidPackagesObject
        ));
    }
}

/// Upstream bun 64669ab07c "install: keep a bundled file: dependency bundled
/// when installing from bun.lock (#42884)" — the bundle-root pre-scan used to
/// read the info object at index 2 of every package entry, which is right
/// only for an npm resolution (`[res, registry, {info}, integrity]`); a
/// `file:`, git, tarball or workspace resolution has no registry string, so
/// its info object is at index 1 and the `"bundled": true` marker was never
/// seen. The installer then installed the bundled dependency a second time
/// (landing as self-referencing `package.json -> package.json` symlinks).
/// Regression assertions re-expressed from
/// test/cli/install/bun-install-registry.test.ts `bundledDependencies` block
/// (registry fixtures are not absorbed).
mod bundled_file_prescan {
    use bun_install::parse_text_lockfile_for_tests as parse_lock;

    /// `file:` resolution entries carry the info object at index 1; their
    /// `"bundled": true` marker must mark the parent's edge BUNDLED.
    #[test]
    fn file_resolution_bundled_marker_is_seen_at_index_1() {
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": { "": { "name": "z" } },
                "packages": {
                    "bundled-file": ["bundled-file@1.0.0", "", { "dependencies": { "bundled-file-dep": "file:vendor/bundled-file-dep" } }, ""],
                    "bundled-file/bundled-file-dep": ["bundled-file-dep@file:vendor/bundled-file-dep", { "bundled": true }]
                }
            }"#,
        )
        .expect("lockfile parses");

        // "bundled-file" is package id 1 (root=0, insertion order of "packages").
        let parent = lockfile.packages.get(1);
        let name = parent.name.slice(lockfile.buffers.string_bytes.as_slice());
        assert_eq!(name, b"bundled-file");
        let deps = parent.dependencies.get(&lockfile.buffers.dependencies);
        assert_eq!(deps.len(), 1);
        assert!(
            deps[0].behavior.is_bundled(),
            "file: entry's bundled marker at index 1 must mark the parent edge BUNDLED"
        );
    }

    /// npm resolutions keep the info object at index 2; their bundled marker
    /// must keep working (the old hardcoded index was right for these).
    #[test]
    fn npm_resolution_bundled_marker_still_seen_at_index_2() {
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": { "": { "name": "z" } },
                "packages": {
                    "ships-bundled": ["ships-bundled@1.0.0", "", { "dependencies": { "inner": "1.0.0" } }, ""],
                    "ships-bundled/inner": ["inner@1.0.0", "", { "bundled": true }, ""]
                }
            }"#,
        )
        .expect("lockfile parses");

        let parent = lockfile.packages.get(1);
        let name = parent.name.slice(lockfile.buffers.string_bytes.as_slice());
        assert_eq!(name, b"ships-bundled");
        let deps = parent.dependencies.get(&lockfile.buffers.dependencies);
        assert_eq!(deps.len(), 1);
        assert!(
            deps[0].behavior.is_bundled(),
            "npm entry's bundled marker at index 2 must keep marking the edge BUNDLED"
        );
    }

    /// Without the marker the edge must NOT be bundled (a `file:` dependency
    /// that is not bundled is installed/linked normally).
    #[test]
    fn file_resolution_without_marker_is_not_bundled() {
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": { "": { "name": "z" } },
                "packages": {
                    "bundled-file": ["bundled-file@1.0.0", "", { "dependencies": { "bundled-file-dep": "file:vendor/bundled-file-dep" } }, ""],
                    "bundled-file/bundled-file-dep": ["bundled-file-dep@file:vendor/bundled-file-dep", {}]
                }
            }"#,
        )
        .expect("lockfile parses");

        let parent = lockfile.packages.get(1);
        let deps = parent.dependencies.get(&lockfile.buffers.dependencies);
        assert_eq!(deps.len(), 1);
        assert!(
            !deps[0].behavior.is_bundled(),
            "no bundled marker => edge stays installable"
        );
    }
}
