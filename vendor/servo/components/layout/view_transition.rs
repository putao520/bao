/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS View Transitions — the layout/paint half of the view transition
//! pseudo-element tree.
//!
//! BAO PATCH (REQ-BRW-051 C2): self-built. This module owns the parts of
//! <https://drafts.csswg.org/css-view-transitions-1/> that are pure data and
//! pure string generation, so that the remaining wiring is a *mechanical*
//! hand-off rather than a redesign:
//!
//! * [`ViewTransitionPseudoTree`] — the `::view-transition` → group →
//!   image-pair → old/new structure produced by "setup transition
//!   pseudo-elements".
//! * [`ViewTransitionPseudoTree::dynamic_stylesheet`] — the per-name style
//!   rules and keyframes the spec appends to the *dynamic view transition style
//!   sheet* (single-sided fade rules, the paired group keyframes with
//!   `plus-lighter` blending, and the default `-ua-*` animation names).
//! * [`ViewTransitionPseudoTree::update_stylesheet`] — "update pseudo-element
//!   styles": the per-name rule that sizes and positions a group from the new
//!   element's border box.
//! * [`ViewTransitionPseudoTree::paint_ops`] — the paint hand-off: one image
//!   primitive per old/new capture, with the rect and blend mode the display
//!   list needs.
//! * [`VIEW_TRANSITION_UA_STYLESHEET`] — the global UA stylesheet, verbatim
//!   from the spec.
//!
//! ## Wiring points (registered, not implemented here)
//!
//! 1. **Display list.** `layout_impl.rs::Layout::build_display_list` must, after
//!    the normal display list is built and before it is handed to the painter,
//!    push a new stacking context covering the snapshot containing block and
//!    emit one image primitive per [`ViewTransitionPaintOp`]. Per spec §View
//!    Transition Painting Order the view transition layer paints after all
//!    other content *including the top layer*, and is not affected by filters
//!    applied to that content.
//! 2. **Captured elements skip painting.** While the phase is `animating`,
//!    elements marked *captured in a view transition* (and their contents) are
//!    painted as if `opacity: 0` and do not respond to hit-testing.
//! 3. **Snapshot capture.** `capture the image` (render an element's ink
//!    overflow rectangle into a texture and return a `webrender_api::ImageKey`)
//!    is what produces [`SnapshotImage`]s. It is the interface the script-side
//!    `ViewTransition::capture_old_state` fails closed without.
//!
//! ## Missing on the stylo side (read-only for this wave)
//!
//! The pseudo-element tree cannot be *styled* until stylo knows about it.
//! `vendor/stylo/servo/selector_parser.rs` currently defines only
//! `After`/`Before`/`Selection`/`FirstLetter`/`Cue` (eager) plus the anonymous
//! and form-control lazies. Required additions, in dependency order:
//!
//! 1. `PseudoElement::ViewTransition`, `ViewTransitionGroup`,
//!    `ViewTransitionImagePair`, `ViewTransitionOld`, `ViewTransitionNew`.
//!    None of them generate boxes from the DOM; they are created by the
//!    transition itself (like `Cue`), so they belong in the lazy group with a
//!    cascade type of their own.
//! 2. **Argumented pseudo-elements.** `::view-transition-group(foo)` carries a
//!    `<custom-ident>` argument. `servo/selector_parser.rs::parse_pseudo_element`
//!    has no argumented form at all today, so `PseudoElement` needs to carry the
//!    name (`Option<Atom>`) and `parse_pseudo_element` needs the `(` … `)`
//!    branch. This is the largest single item.
//! 3. **UA stylesheet slot.** [`VIEW_TRANSITION_UA_STYLESHEET`] must be
//!    installed in the user-agent origin (it uses `:root::view-transition*`
//!    with `*` name matching, which only works once (1) and (2) exist).
//! 4. **Dynamic sheet.** A per-document sheet the transition can add rules to
//!    and remove on "clear view transition"; `Stylist` accepts author sheets,
//!    but this one belongs to the UA origin, so it needs either a second UA
//!    sheet slot or a dedicated `Origin::UserAgent` insertion path.
//! 5. **`:active-view-transition` / `:active-view-transition-type()`** and the
//!    `view-transition-name` lookup used by `capture the old state` ("the
//!    element's document-scoped view transition name"). The `view-transition-name`
//!    *property* already exists in stylo (`ViewTransitionName`), including
//!    `TreeScoped` scoping — so only the element→name lookup and the two
//!    pseudo-classes are missing.

#![allow(dead_code)]

use webrender_api::ImageKey;

/// The global view transition UA stylesheet.
///
/// Verbatim from <https://drafts.csswg.org/css-view-transitions-1/#ua-styles>.
pub(crate) const VIEW_TRANSITION_UA_STYLESHEET: &str = r#"
:root {
    view-transition-name: root;
}

:root::view-transition {
    position: absolute;
    inset: 0;
}

:root::view-transition-group(*) {
    position: absolute;
    top: 0;
    left: 0;

    animation-duration: 0.25s;
    animation-fill-mode: both;
}

:root::view-transition-image-pair(*) {
    position: absolute;
    inset: 0;
}

:root::view-transition-old(*),
:root::view-transition-new(*) {
    position: absolute;
    inset-block-start: 0;
    inline-size: 100%;
    block-size: auto;
}

:root::view-transition-image-pair(*),
:root::view-transition-old(*),
:root::view-transition-new(*) {
    animation-duration: inherit;
    animation-fill-mode: inherit;
    animation-delay: inherit;
    animation-timing-function: inherit;
    animation-iteration-count: inherit;
    animation-direction: inherit;
    animation-play-state: inherit;
}

/* Default cross-fade transition */
@keyframes -ua-view-transition-fade-out {
    to { opacity: 0; }
}
@keyframes -ua-view-transition-fade-in {
    from { opacity: 0; }
}

/* Keyframes for blending when there are 2 images */
@keyframes -ua-mix-blend-mode-plus-lighter {
    from { mix-blend-mode: plus-lighter }
    to { mix-blend-mode: plus-lighter }
}
"#;

/// A rectangle in CSS pixels, relative to the snapshot containing block origin.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CssRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CssRect {
    pub(crate) fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        CssRect {
            x,
            y,
            width,
            height,
        }
    }
}

/// A 2D affine transform in the CSS `matrix(a, b, c, d, e, f)` form.
///
/// The spec's captured transforms and group transforms are all
/// `<transform-function>` values of this shape; keeping the six numbers rather
/// than a `euclid` type makes the CSS serialization exact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Affine2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine2D {
    pub(crate) const IDENTITY: Affine2D = Affine2D {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// The transform that maps a border box from the snapshot containing block
    /// origin to its current visual position. For an untransformed element that
    /// is a pure translation by the border box origin.
    pub(crate) fn translation(x: f32, y: f32) -> Self {
        Affine2D {
            e: x,
            f: y,
            ..Affine2D::IDENTITY
        }
    }

    pub(crate) fn to_css(self) -> String {
        format!(
            "matrix({}, {}, {}, {}, {}, {})",
            self.a, self.b, self.c, self.d, self.e, self.f
        )
    }
}

/// One captured snapshot image.
///
/// `image_key` is the WebRender image holding the element's ink overflow
/// rectangle; `width`/`height` are the *natural* size of the image, which the
/// spec pins to the principal border box ("the natural dimensions of this image
/// must be those of its principal border box").
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SnapshotImage {
    pub image_key: ImageKey,
    pub width: f32,
    pub height: f32,
}

/// The pseudo-element kinds of the view transition tree, in tree order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewTransitionPseudo {
    /// `::view-transition`
    Root,
    /// `::view-transition-group(<name>)`
    Group,
    /// `::view-transition-image-pair(<name>)`
    ImagePair,
    /// `::view-transition-old(<name>)`
    Old,
    /// `::view-transition-new(<name>)`
    New,
}

/// Geometry of one `::view-transition-group(<name>)`, i.e. the inputs of
/// "update pseudo-element styles".
///
/// The `Option<String>` fields are the computed-value copies the spec requires
/// (writing-mode, direction, text-orientation, mix-blend-mode, backdrop-filter,
/// color-scheme); they are `None` when the caller has no computed value to
/// offer, in which case the rule simply omits the property.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupGeometry {
    pub width: f32,
    pub height: f32,
    pub transform: Affine2D,
    pub writing_mode: Option<String>,
    pub direction: Option<String>,
    pub text_orientation: Option<String>,
    pub mix_blend_mode: Option<String>,
    pub backdrop_filter: Option<String>,
    pub color_scheme: Option<String>,
}

impl Default for Affine2D {
    fn default() -> Self {
        Affine2D::IDENTITY
    }
}

/// One transition name's captured old/new state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ViewTransitionGroup {
    /// The `view-transition-name` (a `<custom-ident>`; `root` for the document
    /// element, from the UA stylesheet).
    pub name: String,
    /// The "old" capture, absent for entry animations.
    pub old: Option<SnapshotImage>,
    /// The "new" capture, absent for exit animations.
    pub new: Option<SnapshotImage>,
    /// Present once the new element's border box is known.
    pub geometry: GroupGeometry,
}

impl ViewTransitionGroup {
    /// Whether this group has both sides (the paired cross-fade case).
    fn is_paired(&self) -> bool {
        self.old.is_some() && self.new.is_some()
    }
}

/// The pseudo tree of one view transition, in paint order.
///
/// Ordering is the order of the `named elements` map: elements captured in the
/// old state come first (in paint order), then names that only exist in the new
/// state (entry animations) on top.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewTransitionPseudoTree {
    pub groups: Vec<ViewTransitionGroup>,
}

/// One image primitive of the view transition layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewTransitionPaintOp {
    pub pseudo: ViewTransitionPseudo,
    pub image_key: ImageKey,
    pub rect: CssRect,
    /// `mix-blend-mode: plus-lighter` while both images are shown, so equal
    /// pixels blend to themselves.
    pub mix_blend_plus_lighter: bool,
}

impl ViewTransitionPseudoTree {
    /// "Setup transition pseudo-elements": one group per name, each with an
    /// image pair, holding the old image and/or the new capture.
    pub(crate) fn new(groups: Vec<ViewTransitionGroup>) -> Self {
        ViewTransitionPseudoTree { groups }
    }

    /// The rules "setup transition pseudo-elements" appends to the dynamic view
    /// transition style sheet.
    pub(crate) fn dynamic_stylesheet(&self) -> String {
        let mut css = String::new();
        for group in &self.groups {
            let name = &group.name;
            match (&group.old, &group.new) {
                // Entry animation: only the new image exists.
                (None, Some(_)) => {
                    css.push_str(&format!(
                        ":root::view-transition-new({name}) {{\n    animation-name: -ua-view-transition-fade-in;\n}}\n"
                    ));
                },
                // Exit animation: only the old image exists.
                (Some(_), None) => {
                    css.push_str(&format!(
                        ":root::view-transition-old({name}) {{\n    animation-name: -ua-view-transition-fade-out;\n}}\n"
                    ));
                },
                // Paired cross-fade: group keyframes from the old state plus the
                // plus-lighter blend on both images.
                (Some(old), Some(_)) => {
                    css.push_str(&format!(
                        "@keyframes -ua-view-transition-group-anim-{name} {{\n    from {{\n        transform: {};\n        width: {}px;\n        height: {}px;\n    }}\n}}\n",
                        group.geometry.transform.to_css(),
                        old.width,
                        old.height,
                    ));
                    css.push_str(&format!(
                        ":root::view-transition-old({name}) {{\n    animation-name: -ua-view-transition-fade-out, -ua-mix-blend-mode-plus-lighter;\n}}\n"
                    ));
                    css.push_str(&format!(
                        ":root::view-transition-new({name}) {{\n    animation-name: -ua-view-transition-fade-in, -ua-mix-blend-mode-plus-lighter;\n}}\n"
                    ));
                },
                (None, None) => {},
            }
        }
        css
    }

    /// "Update pseudo-element styles": per-name rules that size and position
    /// the group from the new element's border box.
    pub(crate) fn update_stylesheet(&self) -> String {
        let mut css = String::new();
        for group in &self.groups {
            let geometry = &group.geometry;
            let mut rule = format!(
                ":root::view-transition-group({}) {{\n    width: {}px;\n    height: {}px;\n    transform: {};\n",
                group.name,
                geometry.width,
                geometry.height,
                geometry.transform.to_css(),
            );
            let optional = [
                ("writing-mode", &geometry.writing_mode),
                ("direction", &geometry.direction),
                ("text-orientation", &geometry.text_orientation),
                ("mix-blend-mode", &geometry.mix_blend_mode),
                ("backdrop-filter", &geometry.backdrop_filter),
                ("color-scheme", &geometry.color_scheme),
            ];
            for (property, value) in optional {
                if let Some(value) = value {
                    rule.push_str(&format!("    {property}: {value};\n"));
                }
            }
            rule.push_str("}\n");
            css.push_str(&rule);
        }
        css
    }

    /// The paint hand-off for the display-list builder.
    ///
    /// Each group contributes its old image then its new image, both filling the
    /// group's box: the UA stylesheet lays the image pair over the group and
    /// gives the images `inline-size: 100%`, so the group box *is* the image
    /// rect.
    pub(crate) fn paint_ops(&self) -> Vec<ViewTransitionPaintOp> {
        let mut ops = Vec::new();
        for group in &self.groups {
            let rect = CssRect::new(
                group.geometry.transform.e,
                group.geometry.transform.f,
                group.geometry.width,
                group.geometry.height,
            );
            let mix_blend_plus_lighter = group.is_paired();
            if let Some(old) = group.old {
                ops.push(ViewTransitionPaintOp {
                    pseudo: ViewTransitionPseudo::Old,
                    image_key: old.image_key,
                    rect,
                    mix_blend_plus_lighter,
                });
            }
            if let Some(new) = group.new {
                ops.push(ViewTransitionPaintOp {
                    pseudo: ViewTransitionPseudo::New,
                    image_key: new.image_key,
                    rect,
                    mix_blend_plus_lighter,
                });
            }
        }
        ops
    }
}

#[cfg(test)]
mod tests {
    use webrender_api::IdNamespace;

    use super::*;

    fn image(namespace: u32, width: f32, height: f32) -> SnapshotImage {
        SnapshotImage {
            image_key: ImageKey::new(IdNamespace(namespace), 0),
            width,
            height,
        }
    }

    fn group(name: &str, old: bool, new: bool) -> ViewTransitionGroup {
        ViewTransitionGroup {
            name: name.to_owned(),
            old: old.then(|| image(1, 100.0, 50.0)),
            new: new.then(|| image(2, 120.0, 60.0)),
            geometry: GroupGeometry {
                width: 120.0,
                height: 60.0,
                transform: Affine2D::translation(10.0, 20.0),
                ..GroupGeometry::default()
            },
        }
    }

    #[test]
    fn single_sided_groups_use_the_fade_rules() {
        let tree = ViewTransitionPseudoTree::new(vec![
            group("root", true, false),
            group("entering", false, true),
        ]);
        let css = tree.dynamic_stylesheet();
        assert!(css.contains(
            ":root::view-transition-old(root) {\n    animation-name: -ua-view-transition-fade-out;\n}"
        ));
        assert!(css.contains(
            ":root::view-transition-new(entering) {\n    animation-name: -ua-view-transition-fade-in;\n}"
        ));
        assert!(!css.contains("-ua-mix-blend-mode-plus-lighter"));
    }

    #[test]
    fn paired_groups_emit_group_keyframes_and_plus_lighter() {
        let tree = ViewTransitionPseudoTree::new(vec![group("target", true, true)]);
        let css = tree.dynamic_stylesheet();
        assert!(css.contains("@keyframes -ua-view-transition-group-anim-target"));
        assert!(css.contains("transform: matrix(1, 0, 0, 1, 10, 20);"));
        assert!(css.contains("width: 100px;"));
        assert!(css.contains(
            "animation-name: -ua-view-transition-fade-in, -ua-mix-blend-mode-plus-lighter;"
        ));
    }

    #[test]
    fn update_stylesheet_serializes_the_group_box() {
        let tree = ViewTransitionPseudoTree::new(vec![group("target", true, true)]);
        let css = tree.update_stylesheet();
        assert!(css.contains(":root::view-transition-group(target)"));
        assert!(css.contains("width: 120px;"));
        assert!(css.contains("height: 60px;"));
        assert!(css.contains("transform: matrix(1, 0, 0, 1, 10, 20);"));
        assert!(!css.contains("writing-mode"));
    }

    #[test]
    fn paint_ops_follow_the_group_box_and_blend_rule() {
        let tree = ViewTransitionPseudoTree::new(vec![
            group("paired", true, true),
            group("exit", true, false),
        ]);
        let ops = tree.paint_ops();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].pseudo, ViewTransitionPseudo::Old);
        assert!(ops[0].mix_blend_plus_lighter);
        assert_eq!(ops[1].pseudo, ViewTransitionPseudo::New);
        assert_eq!(ops[2].pseudo, ViewTransitionPseudo::Old);
        assert!(!ops[2].mix_blend_plus_lighter);
        assert_eq!(ops[0].rect, CssRect::new(10.0, 20.0, 120.0, 60.0));
    }
}
