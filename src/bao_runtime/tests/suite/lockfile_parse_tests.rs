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

/// Upstream bun b64b63069c "install: only constrain transitive file:
/// targets of remote packages (#33106)" — the escape check used to reject
/// ANY transitive folder path leaving the project root, whoever declared it,
/// breaking root `file:` packages with relative `file:` dependencies of
/// their own (`error: Could not find package.json for "file:../packages/..."
/// dependency`). The fix gates both sites (resolve-time
/// PackageManagerEnqueue, install-time PackageInstaller) on
/// `Lockfile::is_trusted_folder_dependency`: the declaring package is the
/// root, a workspace, or a `file:` package the root or a workspace depends
/// on directly, or a plain override names the dependency (#32452 carve-out,
/// carried by OverrideMap::contains_name). Assertions mirror the upstream
/// minimal repro and the trust-anchor negative from
/// test/cli/install/bun-install.test.ts / migration tests (e2e registry
/// fixtures not absorbed).
mod trusted_folder_dependency {
    use bun_install::parse_text_lockfile_for_tests as parse_lock;

    /// root -> plugin (file:../packages/plugin, Folder)
    ///      plugin -> shared-lib (file:../shared-lib, Folder, escapes root)
    /// plus a registry package with an escaping file: dependency of its own.
    fn fixture() -> bun_install::Lockfile {
        parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": {
                    "": { "name": "z", "dependencies": { "plugin": "file:../packages/plugin", "remote": "1.0.0" } }
                },
                "packages": {
                    "plugin": ["plugin@file:../packages/plugin", { "dependencies": { "shared-lib": "file:../shared-lib" } }],
                    "plugin/shared-lib": ["shared-lib@file:../shared-lib", {}],
                    "remote": ["remote@1.0.0", "", { "dependencies": { "evil": "file:../../outside" } }, ""],
                    "remote/evil": ["evil@file:../../outside", {}]
                }
            }"#,
        )
        .expect("lockfile parses")
    }

    /// Flat dependency id of the `i`th dependency of package `pkg_id`.
    fn dep_id(
        lockfile: &bun_install::Lockfile,
        pkg_id: bun_install::PackageID,
        i: usize,
    ) -> bun_install::DependencyID {
        let slice = lockfile.packages.get(pkg_id as usize).dependencies;
        slice.off + i as u32
    }

    #[test]
    fn file_dependency_of_root_declared_folder_package_is_trusted() {
        let lockfile = fixture();
        // package ids: root=0, plugin=1, shared-lib=2, remote=3
        let plugin = lockfile.packages.get(1);
        assert_eq!(
            plugin.name.slice(lockfile.buffers.string_bytes.as_slice()),
            b"plugin"
        );
        // root's own file: dependency
        assert!(lockfile.is_trusted_folder_dependency(dep_id(&lockfile, 0, 0)));
        // plugin's transitive file: dependency that escapes the project root —
        // pre-b64b63069c both sites rejected this path unconditionally.
        assert!(
            lockfile.is_trusted_folder_dependency(dep_id(&lockfile, 1, 0)),
            "a file: path declared in a local file: package's package.json is user authored"
        );
    }

    #[test]
    fn file_dependency_of_registry_package_is_not_trusted() {
        let lockfile = fixture();
        assert!(
            !lockfile.is_trusted_folder_dependency(dep_id(&lockfile, 3, 0)),
            "registry, git and tarball packages stay constrained"
        );
        assert!(!lockfile.is_dependency_of_local_package(dep_id(&lockfile, 3, 0)));
    }

    #[test]
    fn parent_pkg_lookup_and_workspace_declaration() {
        let lockfile = fixture();
        // get_parent_pkg_of_dependency (#38867 shape): the root's first
        // dependency belongs to package 0; shared-lib's to plugin (1).
        assert_eq!(
            lockfile.get_parent_pkg_of_dependency(dep_id(&lockfile, 1, 0)),
            Some(1)
        );
        // plugin is a Folder the root depends on directly
        assert!(lockfile.is_workspace_declared_package(1));
        // shared-lib (Folder) is declared by plugin, not by root/workspace
        assert!(!lockfile.is_workspace_declared_package(2));
        // a dependency of a folder package IS a dependency of a local package
        assert!(lockfile.is_dependency_of_local_package(dep_id(&lockfile, 1, 0)));
    }

    /// The trust anchor is checked, not assumed: a Folder shipped inside a
    /// registry package (hand-written or migrated lockfile) does not make
    /// its dependencies trusted (upstream:
    /// `refuses to install an escaping file: dependency that a registry
    /// package's own folder declares in the lockfile`).
    #[test]
    fn folder_inside_registry_package_is_not_a_trust_anchor() {
        let lockfile = parse_lock(
            r#"{
                "lockfileVersion": 1,
                "workspaces": { "": { "name": "z", "dependencies": { "evil": "1.0.0" } } },
                "packages": {
                    "evil": ["evil@1.0.0", "", { "dependencies": { "inner": "file:node_modules/evil/inner" } }, ""],
                    "evil/inner": ["inner@file:node_modules/evil/inner", { "dependencies": { "loot": "file:/outside" } }],
                    "evil/inner/loot": ["loot@file:/outside", {}]
                }
            }"#,
        )
        .expect("lockfile parses");
        // evil/inner is a Folder whose parent is a registry package
        assert!(!lockfile.is_workspace_declared_package(2));
        assert!(!lockfile.is_dependency_of_local_package(dep_id(&lockfile, 2, 0)));
        assert!(!lockfile.is_trusted_folder_dependency(dep_id(&lockfile, 2, 0)));
    }

    #[test]
    fn is_local_package_tag_set() {
        use bun_install::resolution::Tag;
        assert!(Tag::Root.is_local_package());
        assert!(Tag::Workspace.is_local_package());
        assert!(Tag::Folder.is_local_package());
        assert!(!Tag::Npm.is_local_package());
        assert!(!Tag::Git.is_local_package());
        assert!(!Tag::Github.is_local_package());
        assert!(!Tag::LocalTarball.is_local_package());
        assert!(!Tag::RemoteTarball.is_local_package());
        assert!(!Tag::Symlink.is_local_package());
    }
}
