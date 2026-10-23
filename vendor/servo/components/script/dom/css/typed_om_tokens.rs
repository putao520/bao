/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO fork self-build (REQ-BRW-051 C2): raw-token normalisation for
//! `CSSUnparsedValue`.
//!
//! Implements "normalize a CSS value's tokens"
//! (<https://drafts.css-houdini.org/css-typed-om-1/#normalize-tokens>): every
//! non-`var()` component value is preserved verbatim (whitespace and comments
//! included, comments dropped), while `var()` references become
//! `CSSVariableReferenceValue` segments with their fallback tokenised
//! recursively. Also owns the token-merge separator rule used when serializing
//! an unparsed value back to text.

use cssparser::{ParseError, Parser as CssParser, Token};
use js::context::JSContext;

use super::cssunparsedvalue::{CSSUnparsedSegment, CSSUnparsedValue};
use super::cssvariablereferencevalue::CSSVariableReferenceValue;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

/// A normalised raw-token segment, before DOM object creation.
#[derive(Clone, Debug)]
pub(crate) enum RawSegment {
    Text(String),
    VariableReference {
        variable: String,
        fallback: Option<Vec<RawSegment>>,
    },
}

/// Whether a character can continue a CSS identifier/number token.
fn is_ident_code_point(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || character == '_'
        || character == '-'
        || !character.is_ascii()
}

/// Whether serializing `previous` immediately followed by `next` would retokenise
/// into different tokens, requiring an inserted comment (`/**/`).
pub(crate) fn needs_separator(previous: &str, next: &str) -> bool {
    let Some(previous) = previous.chars().next_back() else {
        return false;
    };
    let Some(next) = next.chars().next() else {
        return false;
    };
    if is_ident_code_point(previous) && is_ident_code_point(next) {
        return true;
    }
    previous.is_ascii_digit() && next == '.'
}

fn flush(pending: &mut String, segments: &mut Vec<RawSegment>) {
    if !pending.is_empty() {
        segments.push(RawSegment::Text(std::mem::take(pending)));
    }
}

fn parse_variable_reference(
    nested: &mut CssParser,
) -> Result<(String, Option<Vec<RawSegment>>), ()> {
    let variable = nested.expect_ident().map_err(|_| ())?.to_string();
    let has_fallback = nested.try_parse(|input| input.expect_comma()).is_ok();
    if !has_fallback {
        if !nested.is_exhausted() {
            return Err(());
        }
        return Ok((variable, None));
    }
    let mut fallback = Vec::new();
    let mut pending = String::new();
    tokenize_into(nested, &mut pending, &mut fallback)?;
    flush(&mut pending, &mut fallback);
    Ok((variable, Some(fallback)))
}

fn tokenize_into(
    input: &mut CssParser,
    pending: &mut String,
    segments: &mut Vec<RawSegment>,
) -> Result<(), ()> {
    loop {
        let start = input.position();
        let token = match input.next_including_whitespace_and_comments() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        match token {
            // Comments never survive normalisation (they are not tokens of the
            // value); token merging at serialization time is handled by
            // `needs_separator`.
            Token::Comment(_) => {},
            Token::Function(ref name) if name.eq_ignore_ascii_case("var") => {
                let mut parsed: Option<(String, Option<Vec<RawSegment>>)> = None;
                input
                    .parse_nested_block(|nested| {
                        if let Ok(value) = parse_variable_reference(nested) {
                            parsed = Some(value);
                        }
                        Ok(())
                    })
                    .map_err(|_: ParseError<()>| ())?;
                match parsed {
                    Some((variable, fallback)) => {
                        flush(pending, segments);
                        segments.push(RawSegment::VariableReference { variable, fallback });
                    },
                    None => {
                        // Not a well-formed var(): keep the raw text so the
                        // value round-trips instead of being dropped.
                        let end = input.position();
                        pending.push_str(input.slice(start..end));
                    },
                }
            },
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                pending.push_str(input.slice_from(start));
                let close = match token {
                    Token::SquareBracketBlock => ']',
                    Token::CurlyBracketBlock => '}',
                    _ => ')',
                };
                input
                    .parse_nested_block(|nested| {
                        tokenize_into(nested, pending, segments)
                            .map_err(|_| ParseError::<()>::custom(()))
                    })
                    .map_err(|_| ())?;
                pending.push(close);
            },
            _ => {
                pending.push_str(input.slice_from(start));
            },
        }
    }
    Ok(())
}

/// Normalises `input` into raw-token segments.
pub(crate) fn tokenize_unparsed(input: &str) -> Result<Vec<RawSegment>, ()> {
    let mut parser = CssParser::new(input);
    let mut pending = String::new();
    let mut segments = Vec::new();
    tokenize_into(&mut parser, &mut pending, &mut segments)?;
    flush(&mut pending, &mut segments);
    Ok(segments)
}

/// Whether the segments contain any `var()` reference.
pub(crate) fn segments_have_variable_reference(segments: &[RawSegment]) -> bool {
    segments.iter().any(|segment| match segment {
        RawSegment::Text(_) => false,
        RawSegment::VariableReference { .. } => true,
    })
}

/// Builds the `CSSUnparsedValue` object tree for the given raw segments.
pub(crate) fn build_unparsed_value(
    cx: &mut JSContext,
    global: &GlobalScope,
    segments: &[RawSegment],
) -> DomRoot<CSSUnparsedValue> {
    let mut result = Vec::with_capacity(segments.len());
    for segment in segments {
        match segment {
            RawSegment::Text(text) => result.push(CSSUnparsedSegment::Text(text.clone())),
            RawSegment::VariableReference { variable, fallback } => {
                let fallback = fallback
                    .as_ref()
                    .map(|fallback| build_unparsed_value(cx, global, fallback));
                let reference =
                    CSSVariableReferenceValue::new(cx, global, variable.clone(), fallback);
                result.push(CSSUnparsedSegment::VariableReference(Dom::from_ref(
                    &*reference,
                )));
            },
        }
    }
    CSSUnparsedValue::new(cx, global, result)
}

/// Normalises `text` and builds a `CSSUnparsedValue`. Malformed `var()`
/// references degrade to a single verbatim text segment (never panics and never
/// loses the value).
pub(crate) fn build_unparsed_from_text(
    cx: &mut JSContext,
    global: &GlobalScope,
    text: &str,
) -> DomRoot<CSSUnparsedValue> {
    match tokenize_unparsed(text) {
        Ok(segments) => build_unparsed_value(cx, global, &segments),
        Err(()) => build_unparsed_value(cx, global, &[RawSegment::Text(text.to_string())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_single_segment() {
        let segments = tokenize_unparsed("10px solid red").expect("tokenize");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], RawSegment::Text("10px solid red".to_string()));
        assert!(!segments_have_variable_reference(&segments));
    }

    #[test]
    fn var_reference_is_split_out() {
        let segments = tokenize_unparsed("var(--x)").expect("tokenize");
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            RawSegment::VariableReference { variable, fallback } => {
                assert_eq!(variable, "--x");
                assert!(fallback.is_none());
            },
            other => panic!("expected a variable reference, got {other:?}"),
        }
        assert!(segments_have_variable_reference(&segments));
    }

    #[test]
    fn var_with_fallback_keeps_nested_segments() {
        let segments = tokenize_unparsed("1px var(--x, 2px) solid").expect("tokenize");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], RawSegment::Text("1px ".to_string()));
        match &segments[1] {
            RawSegment::VariableReference { variable, fallback } => {
                assert_eq!(variable, "--x");
                let fallback = fallback.as_ref().expect("fallback present");
                assert_eq!(
                    fallback.as_slice(),
                    [RawSegment::Text(" 2px".to_string())].as_slice()
                );
            },
            other => panic!("expected a variable reference, got {other:?}"),
        }
        assert_eq!(segments[2], RawSegment::Text(" solid".to_string()));
    }

    #[test]
    fn malformed_var_degrades_to_text() {
        // `var()` without a custom-ident argument is not a valid reference; the
        // raw text is kept verbatim (never panics, never loses the value).
        // (An unterminated `var(--x` recovers per the CSS tokenizer's EOF
        // handling and *is* a reference.)
        let segments = tokenize_unparsed("var()").expect("degrades, never errors");
        assert_eq!(
            segments.as_slice(),
            [RawSegment::Text("var()".to_string())].as_slice()
        );
        assert!(!segments_have_variable_reference(&segments));
    }

    #[test]
    fn unterminated_var_recovers_per_css_tokenizer() {
        // CSS syntax recovers EOF inside an unclosed function as if the block
        // were closed, so `var(--x` still yields a reference.
        let segments = tokenize_unparsed("var(--x").expect("tokenize");
        assert!(matches!(
            segments.as_slice(),
            [RawSegment::VariableReference { variable, fallback: None }]
                if variable == "--x"
        ));
    }

    #[test]
    fn nested_var_inside_function_is_extracted() {
        // var() references are extracted at any nesting depth; the surrounding
        // function text stays verbatim.
        let segments = tokenize_unparsed("rgb(1, 2, var(--accent))").expect("tokenize");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], RawSegment::Text("rgb(1, 2, ".to_string()));
        assert!(matches!(
            &segments[1],
            RawSegment::VariableReference { variable, fallback: None }
                if variable == "--accent"
        ));
        assert_eq!(segments[2], RawSegment::Text(")".to_string()));
        assert!(segments_have_variable_reference(&segments));
    }

    #[test]
    fn non_var_functions_stay_verbatim() {
        let segments = tokenize_unparsed("calc(1px + 2px)").expect("tokenize");
        assert_eq!(
            segments.as_slice(),
            [RawSegment::Text("calc(1px + 2px)".to_string())].as_slice()
        );
        assert!(!segments_have_variable_reference(&segments));
    }
}
