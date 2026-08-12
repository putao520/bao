# Good first issues

This file tracks approachable entry points for new contributors. Bao's core
(SpiderMonkey GC rooting, servo integration, the dual-realm model) is deep —
but there is plenty of valuable work that does **not** require understanding the
whole stack. The issues below are real work, deliberately scoped to be
self-contained.

> Maintainers: when creating the corresponding GitHub issues, tag them
> `good first issue` / `help wanted` and link back here.

## Compatibility tests (no engine internals needed)

These add a measurable, public-facing test. Each is independently valuable.

- **Add a Node API compatibility test** — pick a `node:*` module in
  `src/bao_runtime/`, add conformance cases to `compat/node/`, wire it into the
  compat runner. Good first module: `node:url` query-string edge cases.
- **Add a Bun API compatibility test** — e.g. `Bun.file` / `Bun.write`
  round-trip; record the pass rate in `compat/bun/`.
- **Add a CDP method schema test** — pick one of the 12 CDP domains, add a
  request/response shape test in `compat/cdp/`.
- **Add a Web Platform Tests subset** — pull a small WPT slice (DOM events,
  HTML parsing) and report the pass rate under `compat/web/`.

## Examples & docs

- **Improve an example** — the four examples in `examples/` can always be
  clearer, smaller, or cover another use case.
- **Add a benchmark case** — `bench/` lists the dimensions to measure; pick one
  (e.g. `evaluate/s` round-trip latency) and add a runnable harness.
- **Improve Windows / macOS build docs** — the primary path is Linux x86_64;
  documenting what it takes to build elsewhere is genuinely useful.

## Tooling

- **Tighten `bao doctor`** — add a check (e.g. detect missing `libssl-dev`,
  or check `RUST_LOG` suggestions). Self-contained in `src/bao_cli/src/doctor.rs`.
- **Add a CHANGELOG entry helper** — a tiny script to validate
  Keep-a-Changelog format.

## Before you start

1. Read [CONTRIBUTING.md](../CONTRIBUTING.md) — especially the **SPEC-driven**
   and **"don't modify bun_*/vendor/"** rules.
2. Comment on the issue so others know you're picking it up.
3. Open a draft PR early if you want design feedback.

## What good-first-issues are NOT

- Fixing SpiderMonkey GC rooting — not beginner-friendly.
- Modifying vendored servo/mozjs code — forbidden by default (see
  CONTRIBUTING; only authorized BCE patches touch upstream).
- Large architectural changes — discuss in a Discussion/issue first.
