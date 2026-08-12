# Spec-driven development

Bao is developed with a strict **requirement-first** methodology. Every feature
is a contract before it is code, and every code change traces back to that
contract. This page explains why and how — it is the single best signal that
Bao is a serious, long-maintained project rather than a solo experiment.

## The derivation chain (single direction, never reversed)

```
PRD  ── what to build & for whom (goals, scenarios, what-NOT-to-build)
 │
 ▼
SPEC ── how to know it's right (REQ / Entity / API / DEC / state machines)
 │
 ▼
Code ── the runnable artifact that fulfills the SPEC
```

Each layer delegates downward and never rewrites the layer above:

| Layer | Decides | Source of truth | Must not |
|-------|---------|-----------------|----------|
| **PRD** (`.spec/01-BUSINESS.html` …) | user goals, scenarios, explicit non-goals | product intent | be rewritten from current code or "industry惯例" |
| **SPEC** (`.spec/10-REQUIREMENTS.html` …) | REQ / Entity / API / Decision contracts | implementation contract | be reverse-written from code现状 |
| **Code** | the runnable deliverable | no legislation of its own | invent domain models the SPEC doesn't define |

`SPEC.req.groundedIn → PRD-REQ`. `Code @trace → SPEC REQ`. Code never becomes
a source of truth.

## Why this matters

Most "the implementation drifted from the intent" bugs are impossible when the
chain is intact:

1. **You read the contract before writing code.** The bug class "guessed an API
   name / signature / serialization shape from training data" — which accounts
   for nearly all integration failures — is eliminated because you `spec_read`
   the REQ first.
2. **Audit measures code against SPEC, not against "what I think it should do".**
   A `residual` or `gap` is a real diff against a written contract, not an
   opinion.
3. **WIP is not the product终点.** Half-built code is recorded as a gap, never
   passed off as the finished product.

## How it's enforced

- **SPEC is SSOT**: `.spec/` is the single source of truth. If the SPEC doesn't
  define it, work stops and the SPEC is amended first — never silently coded
  around.
- **`@trace REQ-XXX` annotations** in code link every structural change to a
  REQ. A CI gate (`.github/workflows/bce-gc-unsafe.yml`, SPEC-ID scanner)
  checks traceability.
- **PRD↔SPEC governance** runs `prd_coverage` (every PRD-REQ derives ≥1 live
  SPEC) and `spec_grounding` (every SPEC REQ grounds in a live PRD-REQ).
  Orphans and broken links are zero-tolerance.

## BCE — Bug-Class Eradication

When a bug is fixed, it is never a one-off patch. Each fix follows the full
loop and the pattern is recorded in `src/BUG-KNOWLEDGE.md`:

```
root-cause → generalize the bug class → sweep the whole project
→ eradicate every instance → confirm residual = 0 → record the pattern
```

A regression test that fails on the bug's *signature* is always added, so the
whole class cannot return. `BUG-KNOWLEDGE.md` carries the accumulated patterns
across sessions — it is the project's memory of "bugs we've already understood."

## Why this is a good fit for AI-assisted engineering

The spec-driven chain makes Bao unusually suitable for AI-assisted
contribution:

- The **contract is explicit**, so an agent can verify its work against a
  written REQ rather than guessing.
- **Traceability is machine-checked**, so drift is caught immediately.
- **BUG-KNOWLEDGE** gives an agent the accumulated context of past failures,
  avoiding re-derivation.

Web/Node/CDP compatibility is an open-ended, continuously-maintained
infrastructure effort — exactly the kind of long tail where disciplined,
traceable, AI-assisted maintenance adds the most value.

## Further reading

- [CONTRIBUTING.md](../CONTRIBUTING.md) — how to participate
- [architecture.md](./architecture.md) — what the layers are
- `.spec/` — the contracts themselves (start with `00-INDEX.html`)
