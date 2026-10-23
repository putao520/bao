/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / C2): CSS Multi-column layout.
//
// Module layout:
//
//  - [`columns`]: the pure column geometry — the pseudo-algorithm that resolves the
//    used column count / column inline size / column gap, and the column filling and
//    balancing algorithms. Free of layout types so that it is covered by the
//    table-driven tests in `flow/multicol/tests/multicol_columns.rs`.
//  - `fragment`: the fragment-level fragmentation of a laid-out column flow —
//    collecting break opportunities from a fragment tree and cutting one column's
//    worth of fragments out of it.
//  - `container`: [`MultiColContainer`], the formatting context itself — column rows
//    (including `column-span: all`), column placement and the block stacking of the
//    rows.
//
// Specification: CSS Multi-column Layout Level 1
// <https://drafts.csswg.org/css-multicol-1/> and, for the spanning element and the
// fragmentation rules, §8/§9 of the same document together with CSS Fragmentation
// <https://drafts.csswg.org/css-break-3/>.
//
// Registered scope boundaries (all of them are honest, deterministic and documented
// rather than silent fallbacks):
//
//  - Vertical writing modes are not fragmented; a `columns` container in a vertical
//    writing mode keeps the pre-existing unfragmented layout. See
//    [`MultiColContainer::is_multicol_style`].
//  - `column-fill` and the `column-rule-*` properties are Gecko-only in stylo, so
//    they cannot be read from a servo `ComputedValues`. The initial `column-fill`
//    value (`balance`) is therefore approximated: the columns are balanced, and with
//    a definite block size they are filled to that block size only when the balanced
//    columns would overflow it. Column rules are not painted. Registering these
//    properties for servo is a stylo-side change.
//  - `break-before` / `break-after` / `break-inside` are Gecko-only in stylo as well,
//    so `break-inside: avoid` cannot be honoured: every collected break opportunity
//    is taken. The `avoid` face of CSS Fragmentation is a stylo-side prerequisite.
//  - Balancing uses a binary search over the column block size with a greedy fill
//    simulation (`columns::plan_balanced`); it is exact for the break opportunities
//    that come from block-level boundaries and line boxes.
//  - Box decorations of a box that is sliced across a column boundary are not
//    sliced per fragment (`box-decoration-break: slice` visual semantics): the box's
//    borders and padding are repeated in every fragment it produces.
//  - `column-span: all` is honoured for in-flow block-level *children* of the
//    container. A spanner nested deeper does not split the column rows.
//  - The baselines propagated out of a fragmented column row come from the row's
//    own layout (i.e. the last inflow baseline of the unfragmented flow), not from
//    the last column box. Only observable for an inline-block multi-column
//    container's baseline alignment.
//  - Absolutely positioned descendants of a fragmented row keep the static position
//    they were given in the unfragmented flow; their hoisted boxes are not offset by
//    the column translation.

pub mod columns;

pub(crate) mod container;
pub(crate) mod fragment;

pub(crate) use container::MultiColContainer;
