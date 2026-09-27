/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use servo_webvtt::cue::text::{
    WebVTTNodeObjectIterable, WebVTTNodeObjectKind, webvtt_cue_text_parsing_rules,
};

#[test]
fn test_parses_simple_cue_text() {
    let root = webvtt_cue_text_parsing_rules("Some text", None);
    assert_eq!(root.kind, WebVTTNodeObjectKind::List);
    assert_eq!(root.children().len(), 1);
    assert_eq!(
        root.children()[0].kind,
        WebVTTNodeObjectKind::Text("Some text".to_owned())
    );
}

#[test]
fn test_parses_italic_object() {
    let root = webvtt_cue_text_parsing_rules("<i>Some italic text</i>", None);
    assert_eq!(root.kind, WebVTTNodeObjectKind::List);
    assert_eq!(root.children().len(), 1);
    let first_child = &root.children()[0];
    assert_eq!(first_child.kind, WebVTTNodeObjectKind::Italic);
    assert_eq!(first_child.children().len(), 1);
    assert_eq!(
        first_child.children()[0].kind,
        WebVTTNodeObjectKind::Text("Some italic text".to_owned())
    );
}

#[test]
fn test_parses_object_with_classes() {
    let root = webvtt_cue_text_parsing_rules("<i.loud>Some italic text</i>", None);
    assert_eq!(root.kind, WebVTTNodeObjectKind::List);
    assert_eq!(root.children().len(), 1);
    let first_child = &root.children()[0];
    assert_eq!(first_child.kind, WebVTTNodeObjectKind::Italic);
    assert_eq!(first_child.applicable_classes, vec!["loud".to_owned()]);
    assert_eq!(first_child.children().len(), 1);
    assert_eq!(
        first_child.children()[0].kind,
        WebVTTNodeObjectKind::Text("Some italic text".to_owned())
    );
}

#[test]
fn test_parses_object_with_multiple_classes() {
    let root = webvtt_cue_text_parsing_rules("<i.loud.big>Some italic text</i>", None);
    assert_eq!(root.kind, WebVTTNodeObjectKind::List);
    assert_eq!(root.children().len(), 1);
    let first_child = &root.children()[0];
    assert_eq!(first_child.kind, WebVTTNodeObjectKind::Italic);
    assert_eq!(
        first_child.applicable_classes,
        vec!["loud".to_owned(), "big".to_owned()]
    );
    assert_eq!(first_child.children().len(), 1);
    assert_eq!(
        first_child.children()[0].kind,
        WebVTTNodeObjectKind::Text("Some italic text".to_owned())
    );
}

#[test]
fn test_parses_nested_objects() {
    let root = webvtt_cue_text_parsing_rules("<v Neil deGrasse Tyson><i>Laughs</i>", None);
    assert_eq!(root.kind, WebVTTNodeObjectKind::List);
    assert_eq!(root.children().len(), 1);
    let first_child = &root.children()[0];
    assert_eq!(
        first_child.kind,
        WebVTTNodeObjectKind::Voice("Neil deGrasse Tyson".to_owned())
    );
    let nested_children = first_child.children();
    assert_eq!(nested_children[0].kind, WebVTTNodeObjectKind::Italic);
    assert_eq!(nested_children[0].children().len(), 1);
    assert_eq!(
        nested_children[0].children()[0].kind,
        WebVTTNodeObjectKind::Text("Laughs".to_owned())
    );
}

#[test]
fn test_can_iterate_through_nodes() {
    let root = webvtt_cue_text_parsing_rules("<v Neil deGrasse Tyson><i>Laughs</i>", None);
    let mut iter = root.into_iter();
    let node = iter.assert_and_return_next_child();
    assert_eq!(
        node.kind,
        WebVTTNodeObjectKind::Voice("Neil deGrasse Tyson".to_owned())
    );
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Italic);
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Text("Laughs".to_owned()));
    assert!(iter.next_is_parent());
    assert_eq!(iter.next(), None);
}

#[test]
fn test_can_iterate_back_to_parent() {
    let root = webvtt_cue_text_parsing_rules(
        "<v Neil deGrasse Tyson><i>Laughs</i><b>Laughs again</b>",
        None,
    );
    let mut iter = root.into_iter();
    let node = iter.assert_and_return_next_child();
    assert_eq!(
        node.kind,
        WebVTTNodeObjectKind::Voice("Neil deGrasse Tyson".to_owned())
    );
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Italic);
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Text("Laughs".to_owned()));
    assert!(iter.next_is_parent());
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Bold);
    let node = iter.assert_and_return_next_child();
    assert_eq!(
        node.kind,
        WebVTTNodeObjectKind::Text("Laughs again".to_owned())
    );
    assert!(iter.next_is_parent());
    assert_eq!(iter.next(), None);
}

#[test]
fn test_can_iterate_with_multiple_classes() {
    let root =
        webvtt_cue_text_parsing_rules("I said <c.red.uppercase>Bear is coming now</c>!!!!", None);
    let mut iter = root.into_iter();
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Text("I said ".to_owned()));
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Class);
    assert_eq!(
        node.applicable_classes,
        vec!["red".to_owned(), "uppercase".to_owned()]
    );
    let node = iter.assert_and_return_next_child();
    assert_eq!(
        node.kind,
        WebVTTNodeObjectKind::Text("Bear is coming now".to_owned())
    );
    assert!(iter.next_is_parent());
    let node = iter.assert_and_return_next_child();
    assert_eq!(node.kind, WebVTTNodeObjectKind::Text("!!!!".to_owned()));
    assert_eq!(iter.next(), None);
}

// ─── task#8: HTML character reference consumption (named + numeric) ────────

/// Collect the concatenated text of a cue parse result.
fn collected_text(input: &str) -> String {
    let root = webvtt_cue_text_parsing_rules(input, None);
    eprintln!("DBG collected_text({:?}) -> {:?}", input, {
        let mut out = String::new();
        for child in root.children().iter() {
            if let WebVTTNodeObjectKind::Text(text) = &child.kind {
                out.push_str(text);
            }
        }
        out
    });
    root.children()
        .iter()
        .filter_map(|child| match &child.kind {
            WebVTTNodeObjectKind::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn test_parses_named_character_references() {
    assert_eq!(collected_text("&amp;"), "&");
    assert_eq!(collected_text("&lt;"), "<");
    assert_eq!(collected_text("&gt;"), ">");
    assert_eq!(collected_text("&quot;"), "\"");
    assert_eq!(collected_text("&apos;"), "'");
    assert_eq!(collected_text("&nbsp;"), "\u{00A0}");
}

#[test]
fn test_parses_numeric_character_references() {
    // Decimal.
    assert_eq!(collected_text("&#38;"), "&");
    assert_eq!(collected_text("&#60;"), "<");
    // Hexadecimal (both x casings).
    assert_eq!(collected_text("&#x26;"), "&");
    assert_eq!(collected_text("&#X3C;"), "<");
    // C1 window maps through the spec replacement table (128 → €).
    assert_eq!(collected_text("&#128;"), "\u{20AC}");
    // Out-of-range / control edge cases fall back to U+FFFD.
    assert_eq!(collected_text("&#0;"), "\u{FFFD}");
    assert_eq!(collected_text("&#x110000;"), "\u{FFFD}");
}

#[test]
fn test_character_reference_failures_flush_literally() {
    // Legacy semicolon-less names DO match in data state ("&amp" → "&"),
    // and chars consumed past the best match flush literally (browser-
    // consistent: "&amplifier" → "&" + "lifier").
    assert_eq!(collected_text("&amplifier"), "&lifier");
    // Legacy "&not" (no semicolon) matches in data state — browser-consistent:
    // "&notanentity;" → "¬" + literal "anentity;".
    assert_eq!(collected_text("&notanentity;"), "¬anentity;");
    assert_eq!(collected_text("&"), "&");
    assert_eq!(collected_text("&#x"), "&#x");
    assert_eq!(collected_text("&#;"), "&#;");
}

#[test]
fn test_parses_entities_inside_markup() {
    let root = webvtt_cue_text_parsing_rules("<i>a&amp;b</i>", None);
    assert_eq!(root.children().len(), 1);
    let first_child = &root.children()[0];
    assert_eq!(first_child.kind, WebVTTNodeObjectKind::Italic);
    assert_eq!(
        first_child.children()[0].kind,
        WebVTTNodeObjectKind::Text("a&b".to_owned())
    );
    assert_eq!(collected_text("a&lt;b&gt;c&amp;d"), "a<b>c&d");
}
