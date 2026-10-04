// Copyright 2018 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// BAO PATCH (bao, clip-path-recursion-001): regression tests for the
// `clipPath` reference-cycle guard in `src/parser/clippath.rs`.
//
// Upstream 0.48.1 defends against `clipPath` cycles in two layers:
//
// 1. `fix_recursive_links` (`src/parser/svgtree/parse.rs`) strips the cycle
//    attribute at parse time, but only detects self-references and 2-edge
//    cycles. Longer cycles survive with their attributes intact.
// 2. Conversion (`convert` -> `convert_clip_path_elements` -> `convert_group`
//    -> `convert`) re-enters per referenced element. The converted-clip cache
//    is only populated *after* a subtree is converted, so every element on a
//    surviving cycle is always a cache miss and conversion recurses until the
//    stack overflows.
//
// Servo's image cache parses SVG data URLs via `usvg::Tree::from_data` on a
// rayon worker (`GlobalPool`), so a hostile SVG with a >=3-edge cycle aborts
// the whole renderer (WPT css-masking clip-path-recursion-001.svg). The BAO
// guard closes layer 2: a re-entry into a `clipPath` currently being converted
// is an invalid reference and invalidates that clip path, which cascades up
// and empties the cycle's content.

use usvg::{Options, Tree};

fn parse(svg: &str) -> Tree {
    // A stack overflow aborts the process, so simply returning here is
    // already the core regression assertion. `Ok` is guaranteed for
    // well-formed documents: a rejected clip reference only drops the clip,
    // it never fails the parse.
    Tree::from_str(svg, &Options::default()).expect("well-formed SVG must parse")
}

#[test]
fn self_referencing_clip_path_neutralized_at_parse_time() {
    // 1-edge cycle: `fix_recursive_links` strips the self-referencing
    // attribute, so the clip stays valid and its content is kept. This pins
    // the parse-time layer so a future change cannot silently regress it.
    let tree = parse(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>
            <defs>
                <clipPath id='c0'>
                    <rect x='10' width='50' height='50' clip-path='url(#c0)'/>
                </clipPath>
            </defs>
            <rect x='10' width='100' height='100' clip-path='url(#c0)'/>
        </svg>",
    );

    assert!(
        tree.root().has_children(),
        "a self-referencing clipPath is neutralized at parse time; the clipped element must still render"
    );
}

#[test]
fn mutually_referencing_clip_paths_neutralized_at_parse_time() {
    // 2-edge cycle: `fix_recursive_links` strips the second edge, so both
    // clips stay valid and their content is kept.
    let tree = parse(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>
            <defs>
                <clipPath id='a'>
                    <rect width='10' height='10' clip-path='url(#b)'/>
                </clipPath>
                <clipPath id='b'>
                    <rect width='10' height='10' clip-path='url(#a)'/>
                </clipPath>
            </defs>
            <rect width='100' height='100' clip-path='url(#a)'/>
        </svg>",
    );

    assert!(
        tree.root().has_children(),
        "a 2-edge clipPath cycle is neutralized at parse time; the clipped element must still render"
    );
}

#[test]
fn three_edge_clip_cycle_through_mask_does_not_overflow() {
    // >=3-edge cycle (the WPT clip-path-recursion-001.svg shape): the
    // parse-time fixer misses it, the cycle is reached from mask content, and
    // before the BAO guard this recursed until the stack overflowed. Now the
    // re-entry is treated as an invalid reference: the clips cascade to
    // invalid, the mask ends up empty (invalid), and the masked element is
    // dropped - i.e. nothing is rendered, which is also what the WPT test
    // expects ("A clipPath recursion counts as invalid clipping path and
    // makes the element disappear").
    let tree = parse(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>
            <defs>
                <clipPath id='clip0'>
                    <rect width='1' height='1' clip-path='url(#clip)'/>
                </clipPath>
                <clipPath id='clip2'>
                    <rect width='100' height='100' clip-path='url(#clip0)'/>
                </clipPath>
                <clipPath id='clip'>
                    <rect width='1' height='1' clip-path='url(#clip2)'/>
                </clipPath>
                <mask id='mask1' x='0' y='0' width='1' height='1' maskContentUnits='objectBoundingBox'>
                    <rect width='1' height='1' clip-path='url(#clip)'/>
                </mask>
            </defs>
            <circle r='500' mask='url(#mask1)'/>
        </svg>",
    );

    assert!(
        !tree.root().has_children(),
        "a >=3-edge clip cycle must not render any element (invalid reference cascade)"
    );
}

#[test]
fn non_cyclic_clip_chain_is_still_resolved() {
    // Zero-behavior-change control: a valid clip chain must keep working
    // (the referencing element is rendered, i.e. its clip resolved).
    let tree = parse(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>
            <defs>
                <clipPath id='inner'>
                    <rect width='50' height='50'/>
                </clipPath>
                <clipPath id='outer'>
                    <rect width='80' height='80' clip-path='url(#inner)'/>
                </clipPath>
            </defs>
            <rect width='100' height='100' clip-path='url(#outer)'/>
        </svg>",
    );

    assert!(
        tree.root().has_children(),
        "a valid (non-cyclic) clip chain must keep rendering the element"
    );
}
