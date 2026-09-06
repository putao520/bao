//! Binary-lockfile (`bun.lockb`) load tests that must run outside
//! `bun_install` because that crate's own test binary cannot link
//! (upward-resolved `__bun_regex_*` symbols live in `bao_runtime`, same
//! reason as `lockfile_parse_tests.rs`).

/// Keep the link-time providers in `bao_runtime` on the link line: this test
/// links `bun_install`, whose `NodeLinker` declares those upward-resolved
/// symbols, and without a live reference the linker drops the providers from
/// the runtime rlib.
#[inline(never)]
fn force_link_runtime_providers() {
    bun_runtime::product_native_symbols::force_link_product_native_symbols();
}

/// Upstream 86b2e060cf (oven-sh/bun#41366): a `bun.lockb` whose stored
/// `name_hash` disagrees with the package name must load with the hash of the
/// name. The stored hash used to be trusted verbatim, and a later
/// `Package::clone` appending bytes the size pass never reserved (the string
/// pool was keyed by the stored hash) panicked with "range end index N out of
/// range for slice of length M". The debug `verify_data` inside
/// `load_from_bytes` asserts `string_hash(name) == name_hash`, so this test
/// only passes with the load-time recompute in place.
#[test]
fn recomputes_a_package_name_hash_that_disagrees_with_the_name() {
    force_link_runtime_providers();

    use bun_install::lockfile::package::{Package, PackageColumns as _};
    use bun_install::lockfile::{LoadResult, Lockfile, bun_lockb};
    use bun_install::package_manager_real::Options as PackageManagerOptions;
    use bun_install::resolution::{Resolution, TaggedValue};

    // Longer than 8 bytes so the name lives in the string buffer (shorter
    // names inline and never touch the pool).
    let name_bytes: &[u8] = b"dep-with-a-long-pooled-name";
    let good_hash = bun_semver::semver_string::Builder::string_hash(name_bytes);

    let mut lockfile = Lockfile::default();
    {
        let mut builder = lockfile.string_builder();
        builder.count(name_bytes);
        builder.allocate().expect("allocate string buffer");
        let name: bun_semver::String = builder.append(name_bytes);
        drop(builder);
        // A concrete resolution tag — `eql` in `get_package_id` rejects the
        // default (uninitialized) tag.
        let pkg = Package {
            name,
            name_hash: good_hash,
            resolution: Resolution::init(TaggedValue::Root),
            ..Default::default()
        };
        lockfile.append_package(&pkg).expect("append_package");
    }

    let options = PackageManagerOptions::default();
    let mut bytes: Vec<u8> = Vec::new();
    let mut total_size: usize = 0;
    let mut end_pos: usize = 0;
    bun_lockb::save(&mut lockfile, &options, &mut bytes, &mut total_size, &mut end_pos)
        .expect("save lockfile");
    // `save_to_disk` writes the total size into the `end_pos` slot; `load`
    // debug-asserts `stream.pos == total_buffer_size` against it.
    bytes[end_pos..end_pos + core::mem::size_of::<usize>()]
        .copy_from_slice(&total_size.to_ne_bytes());

    // Corrupt the stored name_hash column entry. Columns serialize as raw
    // native bytes and the string pool is not serialized, so the hash of a
    // crafted name appears exactly once in the file.
    let bad_hash = good_hash ^ 0xffff;
    let needle = good_hash.to_ne_bytes();
    let hit = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("stored name_hash not found in serialized lockfile");
    bytes[hit..hit + needle.len()].copy_from_slice(&bad_hash.to_ne_bytes());

    let mut loaded = Lockfile::default();
    let mut log = bun_ast::Log::init();
    let result = loaded.load_from_bytes(None, core::mem::take(&mut bytes), &mut log);
    assert!(
        matches!(result, LoadResult::Ok(_)),
        "corrupted name hash must not fail the load"
    );
    assert_eq!(
        loaded.packages.items_name_hash()[0],
        good_hash,
        "name_hash must be recomputed from the name"
    );
    assert_ne!(good_hash, bad_hash);
}
