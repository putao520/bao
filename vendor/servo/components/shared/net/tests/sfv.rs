/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Tests for the shared RFC 9651 (Structured Field Values) subset in
//! `net_traits::sfv`.
//!
//! BAO PATCH (REQ-BRW-051): these live as an integration test target because
//! the `net_traits` lib target is built with `test = false` (inline
//! `#[cfg(test)]` code is never compiled there). The DBSC list-face tests
//! moved here verbatim from their previous home in
//! `components/net/dbsc/sfv.rs` when the module was hoisted into
//! `net_traits`; the dictionary/serialization faces are new.

use net_traits::sfv::{
    SfvBareItem, SfvDictionaryMember, SfvItem, SfvListMember, parse_dictionary, parse_list,
    serialize_string, string_param_in,
};

// ------------------------------------------------------------------
// List face (migrated from net/dbsc/sfv.rs — assertions unchanged).
// ------------------------------------------------------------------

#[test]
fn parses_registration_header_shape() {
    // BAO PATCH (REQ-BRW-051): RFC 9651 attaches the parameters that
    // follow an inner list's `)` to the inner list itself — DBSC's
    // `path`/`challenge`/`authorization` members are inner-list
    // parameters, and the assertions here pin that reading (the previous
    // form of this test asserted them on the algorithm item, which no
    // legal serialization can satisfy).
    let members = parse_list(
        r#"(RS256);path="/device-bound-session-credentials/start_session.py";challenge="login_challenge_value""#,
    )
    .expect("must parse");
    assert_eq!(members.len(), 1);
    let SfvListMember::InnerList { items, params } = &members[0] else {
        panic!("must be an inner list");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].token(), Some("RS256"));
    assert_eq!(
        string_param_in(params, "path"),
        Some("/device-bound-session-credentials/start_session.py")
    );
    assert_eq!(
        string_param_in(params, "challenge"),
        Some("login_challenge_value")
    );
    // Item-level parameters are a distinct shape and absent here.
    assert_eq!(items[0].string_param("path"), None);
    assert_eq!(items[0].string_param("challenge"), None);
    // A bare item at the top level is a different member shape.
    assert!(parse_list(r#""challenge_value""#).is_some());
    // The minimal registration shape — inner list plus inner-list
    // parameters — is legal (the audit found this very input
    // mis-asserted as `None` in the previous form of the rejects test).
    assert!(parse_list(r#"(RS256);path="/a""#).is_some());
}

#[test]
fn parses_multi_item_inner_list() {
    // `inner-list = "(" *SP [ item *( 1*SP item ) *SP ] ")" parameters`:
    // more than one algorithm item is legal (the DBSC header carries one
    // today, but the parser must not reject the grammar).
    let members = parse_list(r#"(RS256 ES256);path="/a""#).expect("must parse");
    let SfvListMember::InnerList { items, params } = &members[0] else {
        panic!("must be an inner list");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].token(), Some("RS256"));
    assert_eq!(items[1].token(), Some("ES256"));
    assert_eq!(string_param_in(params, "path"), Some("/a"));
}

#[test]
fn parses_comma_joined_inner_lists() {
    let members = parse_list(r#"(RS256);path="/a", (RS256);path="/b""#).expect("must parse");
    assert_eq!(members.len(), 2);
    for member in &members {
        let SfvListMember::InnerList { items, .. } = member else {
            panic!("must be an inner list");
        };
        assert_eq!(items[0].token(), Some("RS256"));
    }
    assert_eq!(
        members[1].clone(),
        parse_list(r#"(RS256); path="/b""#).expect("must parse")[0]
    );
}

#[test]
fn parses_challenge_with_id_parameter() {
    let members = parse_list(r#""refresh_challenge_value";id="0""#).expect("must parse");
    let SfvListMember::Item(item) = &members[0] else {
        panic!("must be an item");
    };
    assert_eq!(item.bare.as_str(), Some("refresh_challenge_value"));
    assert_eq!(item.string_param("id"), Some("0"));
}

#[test]
fn rejects_truncated_and_foreign_shapes() {
    // Unterminated inner list (no closing parenthesis).
    assert!(parse_list(r#"(RS256"#).is_none());
    // Inner list closed but the parameter list truncated (`;` with no
    // `key=` after it).
    assert!(parse_list(r#"(RS256);"#).is_none());
    // A comma where the inner list's closing parenthesis belongs.
    assert!(parse_list(r#"(RS256,"#).is_none());
    // Boolean-ish parameter value (implicit-true parameter, no "=").
    assert!(parse_list(r#"(RS256);secure"#).is_none());
    // Integer bare item is outside the DBSC subset.
    assert!(parse_list("123").is_none());
    // Trailing garbage.
    assert!(parse_list(r#""challenge" garbage"#).is_none());
    // Empty input is not a valid list.
    assert!(parse_list("").is_none());
}

#[test]
fn rejects_parameter_without_value_and_string_escapes() {
    assert!(parse_list(r#"(RS256);path"#).is_none());
    let members = parse_list(r#""a\"b\\c""#).expect("escapes must parse");
    let SfvListMember::Item(item) = &members[0] else {
        panic!("must be an item");
    };
    assert_eq!(item.bare.as_str(), Some("a\"b\\c"));
}

// ------------------------------------------------------------------
// Dictionary face (No-Vary-Search shapes + fail-closed boundaries).
// ------------------------------------------------------------------

#[test]
fn parses_no_vary_search_dictionary_shapes() {
    // Explicit boolean members.
    assert_eq!(
        parse_dictionary("params=?0"),
        Some(vec![(
            "params".to_string(),
            SfvDictionaryMember::Boolean(false)
        )])
    );
    // A bare key is the implicit boolean true (RFC 9651 §4.2.2).
    assert_eq!(
        parse_dictionary("params"),
        Some(vec![(
            "params".to_string(),
            SfvDictionaryMember::Boolean(true)
        )])
    );
    // Inner list of strings.
    assert_eq!(
        parse_dictionary(r#"params=("a" "b")"#),
        Some(vec![(
            "params".to_string(),
            SfvDictionaryMember::InnerList {
                items: vec![str_item("a"), str_item("b"),],
                params: Vec::new(),
            }
        )])
    );
    // OWS is allowed around the member separator.
    assert_eq!(
        parse_dictionary(r#"params=?0 , except=("a")"#),
        parse_dictionary(r#"params=?0, except=("a")"#)
    );
    // Item and inner-list members can carry token/string parameters.
    assert_eq!(
        parse_dictionary(r#"a="v";p="w""#),
        Some(vec![(
            "a".to_string(),
            SfvDictionaryMember::Item(SfvItem {
                bare: str_bare("v"),
                params: vec![("p".to_string(), str_bare("w"))],
            })
        )])
    );
    // Duplicate keys are preserved in input order (RFC 9651 resolves them
    // last-wins; the consumer owns the policy).
    assert_eq!(
        parse_dictionary("params=?0, params=?1"),
        Some(vec![
            ("params".to_string(), SfvDictionaryMember::Boolean(false)),
            ("params".to_string(), SfvDictionaryMember::Boolean(true)),
        ])
    );
}

#[test]
fn dictionary_rejects_foreign_and_malformed_shapes() {
    // RFC 9651 §4.2.2: "there is a trailing comma; fail parsing."
    assert!(parse_dictionary("params=?0,").is_none());
    // An empty member (",,") fails the same way.
    assert!(parse_dictionary("params=?0,, key-order=?0").is_none());
    // RFC 9651 §3.2: keys and values are separated by "=" without
    // whitespace.
    assert!(parse_dictionary("params = ?1").is_none());
    // Keys are lowercase-only (no ASCII folding of member names).
    assert!(parse_dictionary("Params=?1").is_none());
    // Unterminated string.
    assert!(parse_dictionary(r#"params=("a"#).is_none());
    // Unterminated inner list.
    assert!(parse_dictionary("params=(").is_none());
    // `?2` is not a boolean.
    assert!(parse_dictionary("params=?2").is_none());
    // Booleans inside an inner list are outside the subset.
    assert!(parse_dictionary("params=(?1)").is_none());
    // Integer item values are outside the subset.
    assert!(parse_dictionary("a=5").is_none());
    // Parameters on a bare-key (implicit-boolean) member are outside the
    // subset.
    assert!(parse_dictionary(r#"a;foo="bar""#).is_none());
    // Trailing garbage after a member.
    assert!(parse_dictionary("params=?0 garbage").is_none());
    // Empty input is not a valid dictionary.
    assert!(parse_dictionary("").is_none());
}

// ------------------------------------------------------------------
// Serialization face (RFC 9651 §4.1.6).
// ------------------------------------------------------------------

#[test]
fn serializes_strings_with_rfc_escaping() {
    assert_eq!(serialize_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    assert_eq!(serialize_string("conservative"), "\"conservative\"");
    assert_eq!(serialize_string(""), "\"\"");
}

// ------------------------------------------------------------------
// Local constructors to keep the assertions above readable.
// ------------------------------------------------------------------

fn str_bare(value: &str) -> SfvBareItem {
    SfvBareItem::Str(value.to_string())
}

fn str_item(value: &str) -> SfvItem {
    SfvItem {
        bare: str_bare(value),
        params: Vec::new(),
    }
}
