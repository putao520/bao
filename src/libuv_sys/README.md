# bun_libuv_sys

Raw libuv FFI (Windows only). Re-exports the `libuv` module's contents at (Bao runtime component crate).

On windows targets the build script additionally compiles the vendored
oven-sh/libuv fork (`vendor/libuv`, uv 1.51.1-dev) into a static `uv`
library — the uv_* symbol supply (issue #34). Other targets: declaration-only.


Part of the [Bao project](https://github.com/putao520/bao) — see the [workspace README](https://github.com/putao520/bao#readme) for the architecture overview.

License: MIT OR MPL-2.0.
