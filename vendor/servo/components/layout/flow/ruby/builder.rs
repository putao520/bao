/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / Ruby W1a): DOM-side ruby classification
// glue.
//
// These functions are the only DOM-aware piece of the ruby plan surface:
// they classify the flat-tree children of a ruby container into
// [`RubyChildKind`]s for [`build_plan`]. Classification is tag-first (HTML
// Ruby Extensions §2.1.1 pairs by tags) and display-second (CSS Ruby §2.2),
// exactly the two axes the planner consumes; everything else in the ruby
// module is pure and payload-generic.

use style::values::specified::box_ as stylo;
use web_atoms::{LocalName, local_name};

use crate::flow::ruby::plan::RubyChildKind;

/// Classify one element child of a ruby container (or of an internal ruby
/// container) for box generation.
///
/// The tag wins over the computed `display` (HTML Ruby Extensions §2.1.1,
/// and the UA sheet gives `rb`/`rbc` no display of their own), but only for
/// HTML elements inside HTML documents; every other element is classified by
/// its `display` value alone (CSS Ruby §2.2).
///
/// Returns [`None`] for the children the ruby container ignores: `rp`
/// elements (§2.1.1 step 1) and `display: none` subtrees.
pub fn ruby_child_kind_for_element(
    local_name: Option<&LocalName>,
    display: stylo::Display,
    is_html_element_in_html_document: bool,
) -> Option<RubyChildKind> {
    if is_html_element_in_html_document && let Some(local_name) = local_name {
        if *local_name == local_name!("rp") {
            return None;
        }
        if *local_name == local_name!("rb") {
            return Some(RubyChildKind::Base);
        }
        // `rbc` is not in web_atoms 0.3.0's local_names table; use the dynamic
        // Atom construction (the `dequeue` atom precedent from the SW wave).
        if *local_name == LocalName::from("rbc") {
            return Some(RubyChildKind::BaseContainer);
        }
        if *local_name == local_name!("rt") {
            return Some(RubyChildKind::Annotation);
        }
        if *local_name == local_name!("rtc") {
            return Some(RubyChildKind::AnnotationContainer);
        }
    }

    if display == stylo::Display::None {
        return None;
    }
    Some(if display == stylo::Display::RubyBase {
        RubyChildKind::Base
    } else if display == stylo::Display::RubyBaseContainer {
        RubyChildKind::BaseContainer
    } else if display == stylo::Display::RubyText {
        RubyChildKind::Annotation
    } else if display == stylo::Display::RubyTextContainer {
        RubyChildKind::AnnotationContainer
    } else {
        // Everything else is inline-level content of the surrounding
        // container: nested `display: ruby` containers (§2.2 rule 2 wraps
        // misplaced ruby boxes in an anonymous container in a later wave),
        // and generic phrasing content (§2.2 rule 3). Block-level children
        // are inlinified before they reach this classification (§2.2
        // rule 1).
        RubyChildKind::Inline
    })
}

/// Classify one text child of a ruby container: collapsible white space
/// directly inside the container is [`RubyChildKind::Whitespace`] (its
/// interpretation is §2.2 rules 4-6, done by the planner); any other text is
/// inline content (§2.2 rule 3 anonymous base).
pub fn ruby_child_kind_for_text(text: &str) -> RubyChildKind {
    if text.chars().all(char::is_whitespace) {
        RubyChildKind::Whitespace
    } else {
        RubyChildKind::Inline
    }
}
