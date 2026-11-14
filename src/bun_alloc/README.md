# bun_alloc

Port of `src/bun_alloc/bun_alloc.zig`.

Part of the [Bao project](https://github.com/putao520/bao) — see the [workspace README](https://github.com/putao520/bao#readme) for the architecture overview.

## Build-time feature gate (nightly only)

`ast_alloc::AstAlloc::vec` is a `const fn` over `Vec::new_in`, so the crate
needs whichever `#![feature]` makes that construction const on the running
compiler. The gate name moved with rustc, and `build.rs` proves the selected
arm against the live compiler instead of assuming one:

| compiler | gate (declared in `lib.rs`) |
|---|---|
| stable | none — `allocator_api2` mirror |
| nightly < 1.100 | `feature(allocator_api)` |
| nightly >= 1.100 (e.g. 1.101.0-nightly) | `feature(allocator_ext)` |

`build.rs` probes `allocator_ext` first, then `allocator_api`, and fails the
build with the compiler version if neither permits a const `Vec::new_in`.
The probe runs with `-D stable_features`, matching the crate's
`warnings = deny` lint posture, so a gate that is only accepted *with* a
stability warning is rejected rather than selected and later failed.

Do not re-add a silent fallback to `allocator_api`: on a >= 1.100 nightly
that shape compiles the attribute but not `AstAlloc::vec`, which is the
0.1.4/0.2.0 crates.io failure of issue #57.

License: MIT OR MPL-2.0.
