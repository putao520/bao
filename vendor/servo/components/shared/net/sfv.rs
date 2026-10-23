/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Minimal RFC 9651 (Structured Field Values) parser — the shared subset the
//! Bao fork needs.
//!
//! BAO PATCH (REQ-BRW-051): this module is the single Structured Fields slice
//! for the whole engine, shared by two consumers that previously hand-rolled
//! their own syntax faces (same-wave audit finding — divergent strictness
//! would have let RFC compliance fixes stop propagating between them):
//!
//! - `net::dbsc` (Device Bound Session Credentials headers
//!   `Secure-Session-Registration` / `Secure-Session-Challenge` /
//!   `Clear-Site-Data`) via [`parse_list`],
//! - `script::dom::speculation` (the `No-Vary-Search` hint of speculation
//!   rules) via [`parse_dictionary`] and the `Sec-Speculation-Tags` string
//!   serialization via [`serialize_string`].
//!
//! The upstream dependency tree has no `sfv` crate, so this module implements
//! the smallest faithful slice of RFC 9651:
//!
//! - **List face** ([`parse_list`]): List of Items and Inner Lists (the only
//!   top-level shape DBSC uses), with bare Items of type Token and String and
//!   Parameters with Token or String values. Booleans, Integers, Decimals,
//!   Byte Sequences, Dates and Display Strings fail the parse — the caller
//!   treats a failed parse as "header absent", which is the spec-mandated
//!   behavior for unparseable fields (fail-closed: an unknown shape never
//!   establishes or mutates a session).
//! - **Dictionary face** ([`parse_dictionary`], RFC 9651 §4.2.2): members
//!   keyed by RFC-key-validated keys (lcalpha / `*` start), with Boolean
//!   (`?0`/`?1`, and the
//!   implicit-true bare-key form), String/Token Items, and Inner Lists of the
//!   same. Everything else fails. Duplicate keys are preserved in insertion
//!   order; RFC 9651 semantics is last-wins overwrite, and consumers apply
//!   their own duplicate policy on top.
//! - **Serialization** ([`serialize_string`], RFC 9651 §4.1.6): a single
//!   String bare item with DQUOTE/backslash escaping. The RFC's serialization
//!   *failure* preconditions (non-ASCII or control characters) are caller
//!   obligations here — the only consumer feeds it tags already validated as
//!   printable ASCII.
//!
//! Not implemented (registered boundary): Integer/Decimal bare items, Byte
//! Sequences, Dates, Display Strings, parameters with Boolean values, and
//! parameters attached to bare-key Dictionary members — no consumer needs
//! them, and every such shape fails the parse fail-closed.

/// A bare item value — the shared subset (token or string).
#[derive(Clone, Debug, PartialEq)]
pub enum SfvBareItem {
    Token(String),
    Str(String),
}

impl SfvBareItem {
    /// The token payload, if this bare item is a token.
    pub fn as_token(&self) -> Option<&str> {
        match self {
            SfvBareItem::Token(token) => Some(token),
            SfvBareItem::Str(_) => None,
        }
    }

    /// The string payload, if this bare item is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SfvBareItem::Str(value) => Some(value),
            SfvBareItem::Token(_) => None,
        }
    }
}

/// One list entry of a parameter dictionary: `key=bare-item`.
pub type SfvParameter = (String, SfvBareItem);

/// An Item: bare item + parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SfvItem {
    pub bare: SfvBareItem,
    pub params: Vec<SfvParameter>,
}

/// The value of a string-valued parameter in a parameter list (shared by the
/// item-parameter and inner-list-parameter readers: RFC 9651 attaches
/// parameters to both shapes, and DBSC's registration header puts its
/// `path`/`challenge`/`authorization` members on the *inner list*, not on the
/// algorithm item).
/// BAO PATCH (REQ-BRW-051): the string_param_in inner-list read is the fix
/// for the registration-chain defect — reading these members from the item's
/// parameters yields `None` for every legal header.
pub fn string_param_in<'a>(params: &'a [SfvParameter], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(param_key, _)| param_key == key)
        .and_then(|(_, value)| value.as_str())
}

impl SfvItem {
    /// The value of a string-valued parameter, if present.
    pub fn string_param(&self, key: &str) -> Option<&str> {
        string_param_in(&self.params, key)
    }

    /// The bare item as a token, if it is one.
    pub fn token(&self) -> Option<&str> {
        self.bare.as_token()
    }
}

/// One member of a List: a bare Item or an Inner List (with parameters).
#[derive(Clone, Debug, PartialEq)]
pub enum SfvListMember {
    Item(SfvItem),
    InnerList {
        items: Vec<SfvItem>,
        params: Vec<SfvParameter>,
    },
}

/// One member value of a Dictionary ([`parse_dictionary`]): an explicit or
/// implicit Boolean, an Item, or an Inner List.
#[derive(Clone, Debug, PartialEq)]
pub enum SfvDictionaryMember {
    /// `?0` / `?1`, or the implicit `?1` of a bare `key` with no `=`.
    Boolean(bool),
    Item(SfvItem),
    InnerList {
        items: Vec<SfvItem>,
        params: Vec<SfvParameter>,
    },
}

/// Parse a complete field value as an RFC 9651 List. `None` on any parse
/// failure (caller must treat the header as absent — never partially apply).
pub fn parse_list(input: &str) -> Option<Vec<SfvListMember>> {
    let mut parser = SfvParser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let members = parser.parse_list_members()?;
    // RFC 9651: trailing SP is allowed after the list; nothing else is.
    parser.skip_sp();
    if parser.pos != parser.bytes.len() {
        return None;
    }
    Some(members)
}

/// Parse a complete field value as an RFC 9651 Dictionary (§4.2.2). `None` on
/// any parse failure (caller must treat the header as absent — never
/// partially apply).
///
/// Members are returned in input order, duplicates included: RFC 9651
/// resolves duplicate keys last-wins (overwrite), and the policy for a
/// repeated key is left to the consumer (the No-Vary-Search reader rejects
/// its own keys twice-seen).
pub fn parse_dictionary(input: &str) -> Option<Vec<(String, SfvDictionaryMember)>> {
    let mut parser = SfvParser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let members = parser.parse_dictionary_members()?;
    // RFC 9651: trailing SP is allowed after the dictionary; nothing else is.
    parser.skip_sp();
    if parser.pos != parser.bytes.len() {
        return None;
    }
    Some(members)
}

/// Serialize a single RFC 9651 String bare item (§4.1.6): wrap in DQUOTEs,
/// escaping DQUOTE and backslash with a backslash.
///
/// The RFC's serialization *failure* preconditions — the value must be ASCII
/// with only VCHAR and SP (no control characters, no non-ASCII) — are caller
/// obligations; this function never fails and passes such characters through
/// (the sole consumer, `Sec-Speculation-Tags`, pre-validates its tags as
/// printable ASCII).
pub fn serialize_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

struct SfvParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> SfvParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_sp(&mut self) {
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
    }

    fn skip_ows(&mut self) {
        self.skip_sp();
    }

    /// list = list-member *( OWS "," OWS list-member )
    fn parse_list_members(&mut self) -> Option<Vec<SfvListMember>> {
        let mut members = Vec::new();
        loop {
            self.skip_ows();
            members.push(self.parse_list_member()?);
            self.skip_ows();
            if self.peek() == Some(b',') {
                self.pos += 1;
                self.skip_ows();
                // A comma must be followed by another member; an empty list
                // is not a valid Structured Field.
                if self.peek().is_none() {
                    return None;
                }
                continue;
            }
            return Some(members);
        }
    }

    /// list-member = inner-list / item
    fn parse_list_member(&mut self) -> Option<SfvListMember> {
        if self.peek() == Some(b'(') {
            let (items, params) = self.parse_inner_list()?;
            Some(SfvListMember::InnerList { items, params })
        } else {
            Some(SfvListMember::Item(self.parse_item()?))
        }
    }

    /// dictionary = dict-member *( OWS "," OWS dict-member )
    fn parse_dictionary_members(&mut self) -> Option<Vec<(String, SfvDictionaryMember)>> {
        let mut members = Vec::new();
        loop {
            self.skip_ows();
            members.push(self.parse_dictionary_member()?);
            self.skip_ows();
            if self.peek() == Some(b',') {
                self.pos += 1;
                self.skip_ows();
                // RFC 9651 §4.2.2: "there is a trailing comma; fail parsing."
                if self.peek().is_none() {
                    return None;
                }
                continue;
            }
            return Some(members);
        }
    }

    /// dict-member = key [ "=" ( boolean / inner-list / item ) ] — the bare
    /// `key` form is the implicit boolean `true` (§4.2.2). Parameters after a
    /// bare key are legal RFC-wise but outside this subset (fail-closed; the
    /// [`SfvDictionaryMember::Boolean`] carrier has no parameter list).
    fn parse_dictionary_member(&mut self) -> Option<(String, SfvDictionaryMember)> {
        let key = self.parse_key()?;
        if self.peek() == Some(b'=') {
            self.pos += 1;
            let member = self.parse_dictionary_value()?;
            Some((key, member))
        } else {
            Some((key, SfvDictionaryMember::Boolean(true)))
        }
    }

    /// The value side of a `key=` Dictionary member. Note there is no OWS
    /// skip here on purpose: RFC 9651 §3.2 keys and values are separated by
    /// "=" *without whitespace*, so `key = value` fails at the value parser.
    fn parse_dictionary_value(&mut self) -> Option<SfvDictionaryMember> {
        match self.peek() {
            Some(b'(') => {
                let (items, params) = self.parse_inner_list()?;
                Some(SfvDictionaryMember::InnerList { items, params })
            },
            Some(b'?') => self.parse_boolean().map(SfvDictionaryMember::Boolean),
            _ => self.parse_item().map(SfvDictionaryMember::Item),
        }
    }

    /// boolean = "?" ( "0" / "1" )
    fn parse_boolean(&mut self) -> Option<bool> {
        self.expect(b'?')?;
        self.pos += 1;
        match self.peek()? {
            b'0' => {
                self.pos += 1;
                Some(false)
            },
            b'1' => {
                self.pos += 1;
                Some(true)
            },
            _ => None,
        }
    }

    /// inner-list = "(" *SP [ item *( 1*SP item ) *SP ] ")" parameters
    fn parse_inner_list(&mut self) -> Option<(Vec<SfvItem>, Vec<SfvParameter>)> {
        self.expect(b'(')?;
        self.pos += 1;
        self.skip_sp();
        let mut items = Vec::new();
        // Zero-item inner list (DBSC's `(RS256)` has exactly one item, but the
        // grammar allows zero).
        if self.peek() == Some(b')') {
            self.pos += 1;
            let params = self.parse_parameters()?;
            return Some((items, params));
        }
        loop {
            items.push(self.parse_item()?);
            self.skip_sp();
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
                    break;
                },
                // `item *( 1*SP item )`: after the SP run, anything but the
                // closing parenthesis must start another item (so `(A B)` is
                // a two-member inner list). A byte that cannot start a bare
                // item — an early `,`, a stray `;`, end of input (`None`) —
                // fails the parse via `parse_item`/the `None` arm (fail-closed,
                // never silently truncated).
                Some(_) => {},
                None => return None,
            }
        }
        let params = self.parse_parameters()?;
        Some((items, params))
    }

    /// item = bare-item parameters
    fn parse_item(&mut self) -> Option<SfvItem> {
        let bare = self.parse_bare_item()?;
        let params = self.parse_parameters()?;
        Some(SfvItem { bare, params })
    }

    /// bare-item = string / token (shared subset — everything else fails)
    fn parse_bare_item(&mut self) -> Option<SfvBareItem> {
        match self.peek() {
            Some(b'"') => self.parse_string().map(SfvBareItem::Str),
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'*' => {
                self.parse_token().map(SfvBareItem::Token)
            },
            _ => None,
        }
    }

    /// token = ( ALPHA / "*" ) *( tchar / ":" / "/" )
    fn parse_token(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            let is_tchar = byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                        | b':'
                        | b'/'
                );
            if !is_tchar {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        Some(String::from_utf8(self.bytes[start..self.pos].to_vec()).ok()?)
    }

    /// string = DQUOTE *( unescaped / escaped ) DQUOTE
    /// escaped = "\" ( DQUOTE / "\" )
    fn parse_string(&mut self) -> Option<String> {
        self.expect(b'"')?;
        self.pos += 1;
        let mut value = Vec::new();
        loop {
            let byte = self.peek()?;
            self.pos += 1;
            match byte {
                b'"' => {
                    return String::from_utf8(value).ok();
                },
                b'\\' => {
                    let escaped = self.peek()?;
                    self.pos += 1;
                    if escaped != b'"' && escaped != b'\\' {
                        return None;
                    }
                    value.push(escaped);
                },
                byte if (0x20..=0x7e).contains(&byte) || byte == 0x80 => {
                    value.push(byte);
                },
                _ => return None,
            }
        }
    }

    /// parameters = *( ";" *SP parameter )
    fn parse_parameters(&mut self) -> Option<Vec<SfvParameter>> {
        let mut params = Vec::new();
        while self.peek() == Some(b';') {
            self.pos += 1;
            self.skip_sp();
            let key = self.parse_key()?;
            // A parameter with no "=" has an implicit boolean true value in
            // RFC 9651; no consumer uses that form, and a boolean is outside
            // our subset, so treat it as a parse failure (fail-closed).
            self.expect(b'=')?;
            self.pos += 1;
            let value = self.parse_bare_item()?;
            params.push((key, value));
        }
        Some(params)
    }

    /// key = ( lcalpha / "*" ) *( lcalpha / DIGIT / "_" / "-" / "." / "*" )
    fn parse_key(&mut self) -> Option<String> {
        let start = self.pos;
        let first = self.peek()?;
        if !(first.is_ascii_lowercase() || first == b'*') {
            return None;
        }
        self.pos += 1;
        while let Some(byte) = self.peek() {
            let is_key_char = byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'*');
            if !is_key_char {
                break;
            }
            self.pos += 1;
        }
        Some(String::from_utf8(self.bytes[start..self.pos].to_vec()).ok()?)
    }

    fn expect(&self, byte: u8) -> Option<()> {
        if self.peek() == Some(byte) {
            Some(())
        } else {
            None
        }
    }
}
