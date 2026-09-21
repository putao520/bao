# Bao fork of libffi-sys 2.3.0 (upstream: tov/libffi-rs)

Patched for linux-host → windows-msvc cross builds (bao #18). Upstream
2.3.0's build selects its strategy with `#[cfg(target_env = "msvc")]`, which
inside a build script keys on the **host**, so every cross build took the
autoconf/configure arm and died with "OS msvc not recognized"; its msvc arm
additionally required a real MSVC install (`cc::windows_registry::find("cl.exe")`)
for the asm pre-processing step.

Deltas (all marked `BAO FORK` in place):

1. `build/build.rs` — target dispatch via `CARGO_CFG_TARGET_ENV`; both arms
   always compile.
2. `build/msvc.rs` — the `/EP` asm pre-processing runs through the
   env-contract compiler (`CC_<triple>` / `TARGET_CC` / `CC`, default
   `clang-cl`); INCLUDE comes from the environment (xwin-style sysroot
   contract) with the libffi faces appended.
3. `build/msvc.rs` — the pre-processed `.asm` lands in `OUT_DIR` instead of
   the package source tree (the relative path dirtied/raced the crate
   directory on every build).
4. `build/msvc.rs` — for x86_64 the assembly is the GAS-variant
   `libffi/src/x86/win64.S`, assembled by cc's clang-cl (integrated
   assembler, GNU `.seh_*`). The MASM route (`win64_intel.S` → `/EP` →
   ml64.exe) needs a real MSVC ml64; `llvm-ml` cannot parse `extern sym:near`
   or `.seh_*` even with `-m64`, so it is a hard cross blocker. Same exported
   symbols (`ffi_call_win64`/`ffi_closure_win64`), same upstream tree — this
   is the mingw-proven variant. The MASM pre-processing path stays for
   x86/aarch64.

Re-sync recipe: copy the new upstream release over this tree and re-apply the
three `BAO FORK`-marked deltas. Wired via `[patch.crates-io]` in the workspace
root (same-name fork — no consumer manifest changes).
