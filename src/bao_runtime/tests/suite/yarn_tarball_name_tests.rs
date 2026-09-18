// @trace TEST-CLI-001-YARN-TARBALL-NAME [req:REQ-CLI-001] [level:unit]
// Upstream bun 643b957b42 "install: fix yarn.lock migration panic on tarball
// URLs with '/-/' right after the host (#43189)" — the yarn v1 migrator's
// `name_to_use` block searched the resolved URL for "/-/" and for
// "registry." independently, then sliced between the two indexes; a "/-/"
// directly after the host panicked with `slice index starts at 42 but ends
// at 20`. The fix parses left to right (scheme, default-registry host, one
// name segment — two for @scope/name — then "/-/") and requires the name to
// be a safe install folder name.
//
// This is the upstream's full 15-row verdict table (each URL was run through
// `bun pm migrate` in the commit notes; "spec name" rows return None so the
// migrator keeps the spec's own name). Hosted in the bun_runtime suite
// (bun_install's own test binary cannot link).

use bun_install::yarn::Entry;

#[test]
fn default_registry_tarball_url_name_extraction_table() {
    let cases: &[(&str, Option<&[u8]>)] = &[
        // "/-/" right after an evil host — used to panic `42..20`
        (
            "https://evil.example/-/registry.npmjs.org/x.tgz",
            None,
        ),
        // "/-/" right after the real host — used to panic `27..26`
        ("https://registry.npmjs.org/-/y.tgz", None),
        // the real package `-` has that shape
        (
            "https://registry.npmjs.org/-/-/--0.0.1.tgz",
            Some(b"-"),
        ),
        // empty name segment
        ("https://registry.npmjs.org//-/z.tgz", None),
        // registry host in the path, not the authority
        (
            "https://registry.mirror.example/registry.npmjs.org/other/-/other-1.0.0.tgz",
            None,
        ),
        // the first "/-/" of that URL is inside the name `@scope/-`
        (
            "https://registry.npmjs.org/@scope/-/-/--0.0.1.tgz",
            Some(b"@scope/-"),
        ),
        ("https://registry.npmjs.org/@scope/-/y.tgz", None),
        ("https://registry.npmjs.org/@scope//-/z.tgz", None),
        // two name segments without a scope
        ("https://registry.npmjs.org/a/b/-/b-1.0.0.tgz", None),
        // `..` is not a safe install folder name
        ("https://registry.npmjs.org/../-/x-1.0.0.tgz", None),
        // no "/-/" at all
        ("https://registry.npmjs.org/w.tgz", None),
        // the shapes that must keep working
        (
            "https://registry.npmjs.org/@scope/real/-/real-1.0.0.tgz",
            Some(b"@scope/real"),
        ),
        (
            "http://registry.npmjs.org/real-a/-/real-a-1.0.0.tgz",
            Some(b"real-a"),
        ),
        (
            "https://registry.yarnpkg.com/real-b/-/real-b-1.0.0.tgz",
            Some(b"real-b"),
        ),
        (
            "http://registry.yarnpkg.com/real-c/-/real-c-1.0.0.tgz",
            Some(b"real-c"),
        ),
    ];
    for (url, expected) in cases {
        assert_eq!(
            Entry::get_package_name_from_default_registry_url(url.as_bytes()),
            *expected,
            "url: {url}"
        );
    }
}
