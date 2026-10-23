/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Parsing of speculation rule sets.
//!
//! BAO PATCH (REQ-BRW-051 C2, GOLDEN B): upstream Servo has no speculation
//! rules implementation at all. This module implements the prefetch subset of
//! <https://html.spec.whatwg.org/multipage/speculative-loading.html>:
//!
//! * the JSON grammar of a speculation rule set ("parse a speculation rule
//!   set string"), including top-level and per-rule key validation,
//! * the document rule predicate grammar (`and`, `or`, `not`, `href_matches`,
//!   `selector_matches`),
//! * eagerness (`immediate`, `eager`, `moderate`, `conservative`),
//! * ruleset-level and rule-level tags,
//! * the `No-Vary-Search` hint attached to a rule
//!   (`expects_no_vary_search`).
//!
//! Prerendering is intentionally outside of this module: `prerender` lists are
//! not executed and `document.prerendering` is never exposed (see the boundary
//! note in `speculation/mod.rs`).
//!
//! Spec references (all links are to the multi-page HTML Standard):
//! <https://html.spec.whatwg.org/multipage/speculative-loading.html#parse-a-speculation-rule-set-string>

use std::cmp::Ordering;

use net_traits::ReferrerPolicy;
use net_traits::sfv::{self, SfvDictionaryMember};
use selectors::parser::SelectorList;
use serde_json::Value;
use servo_url::ServoUrl;
use style::selector_parser::{SelectorImpl, SelectorParser};
use style::stylesheets::UrlExtraData;
use urlpattern::UrlPattern;
use urlpattern::quirks;

/// The message used when the top-level value of a rule set is not a JSON
/// object (or is not JSON at all).
const TOP_LEVEL_NOT_OBJECT: &str = "the top-level value needs to be a JSON object";

/// The message used when a ruleset-level `tag` is not a string.
const RULESET_TAG_INVALID: &str = "the speculation rule tag is invalid";

/// A speculation rule tag is valid when every character is printable ASCII
/// (U+0020 to U+007E inclusive).
///
/// <https://wicg.github.io/nav-speculation/speculation-rules-tags.html>
fn is_valid_speculation_rule_tag(tag: &str) -> bool {
    tag.chars().all(|c| (' '..='~').contains(&c))
}

/// <https://html.spec.whatwg.org/multipage/speculative-loading.html#speculation-rule-eagerness>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Eagerness {
    Immediate,
    Eager,
    Moderate,
    Conservative,
}

impl Eagerness {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "immediate" => Some(Eagerness::Immediate),
            "eager" => Some(Eagerness::Eager),
            "moderate" => Some(Eagerness::Moderate),
            "conservative" => Some(Eagerness::Conservative),
            _ => None,
        }
    }

    /// Whether a user interaction of the given kind is enough to start a
    /// speculative load with this eagerness.
    ///
    /// The exact interaction heuristics are user-agent defined. Bao follows
    /// the Chromium shape: hover starts `eager` and `moderate` candidates,
    /// pointerdown starts every candidate (including `conservative`).
    pub(crate) fn triggered_by(self, interaction: LinkInteraction) -> bool {
        match interaction {
            LinkInteraction::Hover => {
                matches!(self, Eagerness::Eager | Eagerness::Moderate)
            },
            LinkInteraction::PointerDown => true,
        }
    }
}

/// The kind of user interaction observed on a link that can trigger
/// non-immediate speculation candidates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkInteraction {
    Hover,
    PointerDown,
}

/// A parsed document rule predicate.
///
/// <https://html.spec.whatwg.org/multipage/speculative-loading.html#document-rule-predicate>
pub(crate) enum DocumentPredicate {
    /// The default predicate: every rendered `a`/`area` link in the document.
    All,
    And(Vec<DocumentPredicate>),
    Or(Vec<DocumentPredicate>),
    Not(Box<DocumentPredicate>),
    HrefMatches(UrlPattern),
    SelectorMatches(Box<SelectorList<SelectorImpl>>),
}

/// The target of a single speculation rule.
pub(crate) enum RuleTarget {
    /// `source: "list"`: a fixed set of candidate URLs.
    List(Vec<ServoUrl>),
    /// `source: "document"`: links in the document matching a predicate.
    Document(DocumentPredicate),
}

/// A single parsed `prefetch` speculation rule.
pub(crate) struct SpeculationRule {
    pub(crate) target: RuleTarget,
    pub(crate) eagerness: Eagerness,
    pub(crate) referrer_policy: Option<ReferrerPolicy>,
    pub(crate) tag: Option<String>,
    /// Whether `requires` contained `anonymous-client-ip-when-cross-origin`.
    pub(crate) anonymous_client_ip: bool,
    pub(crate) no_vary_search_hint: UrlSearchVariance,
}

/// A whole parsed speculation rule set (`<script type="speculationrules">`).
pub(crate) struct SpeculationRuleSet {
    /// The ruleset-level `tag`, if any.
    pub(crate) tag: Option<String>,
    /// `prefetch` rules. `prerender` rules are deliberately not represented:
    /// prerendering is not implemented in Bao.
    pub(crate) prefetch: Vec<SpeculationRule>,
}

impl SpeculationRuleSet {
    /// The ordered, deduplicated tag list for a candidate produced by `rule`.
    ///
    /// <https://html.spec.whatwg.org/multipage/speculative-loading.html#speculation-rule-tag>
    /// Ruleset-level tags come first, then the rule-level tag; duplicates
    /// collapse. When there is no tag at all, the predefined `null` tag is
    /// used.
    pub(crate) fn candidate_tags(&self, rule: &SpeculationRule) -> Vec<Option<String>> {
        let mut tags: Vec<Option<String>> = Vec::new();
        if let Some(tag) = self.tag.as_ref() {
            tags.push(Some(tag.clone()));
        }
        if let Some(tag) = rule.tag.as_ref() {
            tags.push(Some(tag.clone()));
        }
        if tags.is_empty() {
            tags.push(None);
        }
        sort_and_dedupe_tags(&mut tags);
        tags
    }
}

/// Sort a tag list into wire order (`null` first, then UTF-16 code unit order)
/// and drop duplicates. Also used when merging candidate groups.
pub(crate) fn sort_and_dedupe_tags(tags: &mut Vec<Option<String>>) {
    tags.sort_by(compare_tags);
    tags.dedup();
}

/// Compare two tags with `null` first, then by UTF-16 code unit order.
fn compare_tags(a: &Option<String>, b: &Option<String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => a.encode_utf16().cmp(b.encode_utf16()),
    }
}

/// Serialize a tag list as the `Sec-Speculation-Tags` structured field value,
/// e.g. `"a", "b"` or `null`.
///
/// <https://wicg.github.io/nav-speculation/prefetch.html#sec-speculation-tags-header>
pub(crate) fn serialize_sec_speculation_tags(tags: &[Option<String>]) -> String {
    let serialized = tags
        .iter()
        .map(|tag| match tag {
            None => "null".to_string(),
            Some(tag) => sfv::serialize_string(tag),
        })
        .collect::<Vec<_>>();
    serialized.join(", ")
}

/// A `No-Vary-Search` value, used both for the per-rule
/// `expects_no_vary_search` hint and for grouping candidates.
///
/// <https://drafts.csswg.org/css-navigation/#no-vary-search>
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UrlSearchVariance {
    mode: SearchVarianceMode,
    /// Whether the order of query keys matters (the `key-order` member,
    /// defaulting to `true`).
    key_order_matters: bool,
}

impl Default for UrlSearchVariance {
    fn default() -> Self {
        Self {
            mode: SearchVarianceMode::Exact,
            key_order_matters: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum SearchVarianceMode {
    /// The default URL search variance: the query string must match exactly.
    #[default]
    Exact,
    /// `params` is `true`: every query parameter may vary.
    AllParamsVary,
    /// `params` is a list: only the listed parameters may vary.
    ParamsVary(Vec<String>),
    /// `except` is a list: every parameter except the listed ones may vary.
    ExceptParamsVary(Vec<String>),
}

impl UrlSearchVariance {
    fn new(mode: SearchVarianceMode, key_order_matters: bool) -> Self {
        Self {
            mode,
            key_order_matters,
        }
    }

    /// Parse a `No-Vary-Search` header value (also used for the rule's
    /// `expects_no_vary_search` member).
    ///
    /// This understands the structured-field dictionary members `params`,
    /// `except` and `key-order`. An invalid value returns `None`.
    ///
    /// BAO PATCH (REQ-BRW-051 SFV convergence): this parse used to run over a
    /// hand-rolled top-level comma splitter that silently tolerated malformed
    /// input — unclosed quoted strings, empty/trailing members (`,`` key` and
    /// `a,,b`), whitespace around `=`, and non-lowercase member keys. It now
    /// parses through the shared RFC 9651 Dictionary face
    /// (`net_traits::sfv::parse_dictionary`), which strictens the accepted
    /// language: every one of those forms — plus `?`-booleans other than
    /// `?0`/`?1` and non-string inner-list members — now fails to `None`
    /// instead of being silently tolerated or failing further downstream.
    /// Every legal shape (`params`/`except` as `?1`/`?0`/bare or a closed
    /// inner list of strings, `key-order` as `?1`/`?0`/bare, OWS around the
    /// member commas) parses exactly as before.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let mut params: Option<SearchVarianceMode> = None;
        let mut except: Option<SearchVarianceMode> = None;
        let mut key_order_matters = true;
        let mut seen_key_order = false;

        let members = sfv::parse_dictionary(value);
        let Some(members) = members else {
            return None;
        };
        if members.is_empty() {
            return None;
        }
        for (name, member) in members {
            // Member keys are validated lowercase by the SFV key grammar
            // (`lcalpha` / `*` start); no ASCII folding on top.
            match name.as_str() {
                "params" => {
                    if params.is_some() {
                        return None;
                    }
                    params = Some(variance_from_member(&member, true)?);
                },
                "except" => {
                    if except.is_some() {
                        return None;
                    }
                    except = Some(variance_from_member(&member, false)?);
                },
                "key-order" => {
                    if seen_key_order {
                        return None;
                    }
                    seen_key_order = true;
                    // A bare `key-order` member is the implicit boolean
                    // `true`; anything but a boolean fails.
                    let SfvDictionaryMember::Boolean(value) = member else {
                        return None;
                    };
                    key_order_matters = value;
                },
                _ => return None,
            }
        }

        if params.is_some() && except.is_some() {
            return None;
        }
        let mode = params.or(except).unwrap_or(SearchVarianceMode::Exact);
        Some(UrlSearchVariance::new(mode, key_order_matters))
    }

    /// Whether the two URLs are equivalent modulo this search variance.
    ///
    /// Two URLs are equivalent when everything except the query (and the
    /// fragment, which is always ignored for speculative loads) matches, and
    /// the query parameters match according to the variance.
    pub(crate) fn urls_equivalent(&self, a: &ServoUrl, b: &ServoUrl) -> bool {
        if a.scheme() != b.scheme() ||
            a.host_str() != b.host_str() ||
            a.port_or_known_default() != b.port_or_known_default() ||
            a.username() != b.username() ||
            a.password() != b.password() ||
            a.path() != b.path()
        {
            return false;
        }

        let query_a = query_pairs(a.query());
        let query_b = query_pairs(b.query());
        let normalize = |pairs: Vec<(String, String)>| {
            if self.key_order_matters {
                pairs
            } else {
                let mut sorted = pairs;
                sorted.sort();
                sorted
            }
        };
        match &self.mode {
            SearchVarianceMode::Exact => normalize(query_a) == normalize(query_b),
            SearchVarianceMode::AllParamsVary => true,
            SearchVarianceMode::ParamsVary(names) => {
                let filter = |pairs: Vec<(String, String)>| {
                    normalize(
                        pairs
                            .into_iter()
                            .filter(|(name, _)| !names.iter().any(|varies| varies == name))
                            .collect::<Vec<_>>(),
                    )
                };
                filter(query_a) == filter(query_b)
            },
            SearchVarianceMode::ExceptParamsVary(names) => {
                let filter = |pairs: Vec<(String, String)>| {
                    normalize(
                        pairs
                            .into_iter()
                            .filter(|(name, _)| names.iter().any(|fixed| fixed == name))
                            .collect::<Vec<_>>(),
                    )
                };
                filter(query_a) == filter(query_b)
            },
        }
    }
}

fn query_pairs(query: Option<&str>) -> Vec<(String, String)> {
    let Some(query) = query else {
        return Vec::new();
    };
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

/// Interpret one No-Vary-Search dictionary member value: a Boolean or an
/// inner list of strings. Token/other item shapes fail (the hint's grammar
/// allows only booleans and string inner lists).
///
/// The two members have opposite polarity: `params` lists the parameters that
/// *may* vary, while `except` lists the parameters that *must not* vary.
fn variance_from_member(
    member: &SfvDictionaryMember,
    is_params: bool,
) -> Option<SearchVarianceMode> {
    match member {
        SfvDictionaryMember::Boolean(value) => Some(match (is_params, *value) {
            (true, true) | (false, false) => SearchVarianceMode::AllParamsVary,
            (true, false) | (false, true) => SearchVarianceMode::Exact,
        }),
        SfvDictionaryMember::InnerList { items, .. } => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                names.push(item.bare.as_str()?.to_string());
            }
            Some(match (is_params, names.is_empty()) {
                (true, true) => SearchVarianceMode::Exact,
                (true, false) => SearchVarianceMode::ParamsVary(names),
                (false, true) => SearchVarianceMode::AllParamsVary,
                (false, false) => SearchVarianceMode::ExceptParamsVary(names),
            })
        },
        // `params=<item>` (a bare token or string) is not a No-Vary-Search
        // value shape.
        SfvDictionaryMember::Item(_) => None,
    }
}

/// Parse a referrer policy token, returning `None` for unknown tokens.
fn parse_referrer_policy(value: &str) -> Option<ReferrerPolicy> {
    const POLICIES: [(&str, ReferrerPolicy); 8] = [
        ("no-referrer", ReferrerPolicy::NoReferrer),
        (
            "no-referrer-when-downgrade",
            ReferrerPolicy::NoReferrerWhenDowngrade,
        ),
        ("origin", ReferrerPolicy::Origin),
        ("same-origin", ReferrerPolicy::SameOrigin),
        ("origin-when-cross-origin", ReferrerPolicy::OriginWhenCrossOrigin),
        ("unsafe-url", ReferrerPolicy::UnsafeUrl),
        ("strict-origin", ReferrerPolicy::StrictOrigin),
        (
            "strict-origin-when-cross-origin",
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
        ),
    ];
    POLICIES
        .iter()
        .find(|(name, _)| value.eq_ignore_ascii_case(name))
        .map(|(_, policy)| *policy)
}

/// The key names allowed in a speculation rule. `target_hint` is parsed and
/// ignored, as required by the specification.
const RULE_KEYS: [&str; 10] = [
    "source",
    "urls",
    "where",
    "relative_to",
    "eagerness",
    "referrer_policy",
    "tag",
    "requires",
    "expects_no_vary_search",
    "target_hint",
];

/// The key names allowed in a document rule predicate.
const PREDICATE_KEYS: [&str; 6] = [
    "and",
    "or",
    "not",
    "href_matches",
    "selector_matches",
    "relative_to",
];

#[derive(Clone, Copy, Eq, PartialEq)]
enum RuleSource {
    List,
    Document,
}

/// Parse a speculation rule set string.
///
/// Returns `Err(message)` only for top-level misconfiguration (invalid JSON, a
/// non-object top level, or an invalid ruleset-level tag); in that case the
/// caller reports a `TypeError`. Malformed individual rules are skipped with a
/// console warning, per
/// <https://html.spec.whatwg.org/multipage/speculative-loading.html#parse-a-speculation-rule-set-string>.
pub(crate) fn parse_speculation_rule_set(
    text: &str,
    base_url: &ServoUrl,
    document_url: &ServoUrl,
    warn: &mut dyn FnMut(&str),
) -> Result<SpeculationRuleSet, String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Err(TOP_LEVEL_NOT_OBJECT.to_string());
    };
    let Some(top) = value.as_object() else {
        return Err(TOP_LEVEL_NOT_OBJECT.to_string());
    };

    // Step: parse the ruleset-level tag.
    let mut tag = None;
    if let Some(raw_tag) = top.get("tag") {
        match raw_tag.as_str() {
            Some(raw_tag) if is_valid_speculation_rule_tag(raw_tag) => {
                tag = Some(raw_tag.to_string());
            },
            // A non-string or non-printable-ASCII ruleset tag invalidates the
            // whole rule set.
            _ => return Err(RULESET_TAG_INVALID.to_string()),
        }
    }

    // Unknown top-level keys are ignored with a warning; they do not discard
    // the rule set (only top-level misconfiguration does that).
    for key in top.keys() {
        if !matches!(key.as_str(), "tag" | "prefetch" | "prerender") {
            warn(&format!("the speculation rule set has an unrecognized key: {key}"));
        }
    }

    let mut prefetch = Vec::new();
    match top.get("prefetch") {
        None => {},
        Some(Value::Array(rules)) => {
            for rule in rules {
                if let Some(rule) =
                    parse_speculation_rule(rule, base_url, document_url, warn)
                {
                    prefetch.push(rule);
                }
            }
        },
        Some(_) => warn("the rules list for type needs to be a JSON array"),
    }

    // `prerender` is deliberately not executed by Bao (prerendering is outside
    // this implementation); it is still validated as an array so that
    // authoring mistakes are reported the same way as for `prefetch`.
    if let Some(prerender) = top.get("prerender") &&
        !prerender.is_array()
    {
        warn("the rules list for type needs to be a JSON array");
    }

    Ok(SpeculationRuleSet { tag, prefetch })
}

fn parse_speculation_rule(
    value: &Value,
    base_url: &ServoUrl,
    document_url: &ServoUrl,
    warn: &mut dyn FnMut(&str),
) -> Option<SpeculationRule> {
    let Some(rule) = value.as_object() else {
        warn("the rule needs to be a JSON object");
        return None;
    };

    for key in rule.keys() {
        if !RULE_KEYS.contains(&key.as_str()) {
            warn(&format!("the rule has unrecognized keys: {key}"));
            return None;
        }
    }

    let has_urls = rule.contains_key("urls");
    let has_where = rule.contains_key("where");

    // Step: determine the source.
    let source = match rule.get("source") {
        Some(Value::String(source)) => match source.as_str() {
            "list" => Some(RuleSource::List),
            "document" => Some(RuleSource::Document),
            _ => {
                warn("a source could not be inferred or an invalid source was specified");
                None
            },
        },
        Some(_) => {
            warn("a source could not be inferred or an invalid source was specified");
            None
        },
        None => match (has_urls, has_where) {
            (true, false) => Some(RuleSource::List),
            (false, true) => Some(RuleSource::Document),
            (false, false) => {
                warn("a source could not be inferred or an invalid source was specified");
                None
            },
            (true, true) => {
                warn("there were conflicting sources for this rule");
                None
            },
        },
    }?;

    if source == RuleSource::List && !has_urls {
        warn("the supplied URL list was invalid");
        return None;
    }
    if source == RuleSource::Document && has_urls && has_where {
        warn("there were conflicting sources for this rule");
        return None;
    }
    if source == RuleSource::Document && has_urls {
        warn("there were conflicting sources for this rule");
        return None;
    }

    // Step: relative_to is only meaningful for list sources without a
    // document predicate.
    let relative_to = match rule.get("relative_to") {
        None => None,
        Some(Value::String(relative_to)) => match relative_to.as_str() {
            "ruleset" => Some("ruleset"),
            "document" => Some("document"),
            _ => {
                warn("the supplied relative-to value was invalid");
                return None;
            },
        },
        Some(_) => {
            warn("the supplied relative-to value was invalid");
            return None;
        },
    };
    if relative_to.is_some() && (source == RuleSource::Document || has_where) {
        warn("the supplied relative-to value was invalid");
        return None;
    }
    // For an inline `<script type="speculationrules">` the rule set URL *is*
    // the document URL, so `relative_to: "ruleset"` and
    // `relative_to: "document"` resolve against the same base URL and the
    // value needs no further handling here. (Out-of-document rule sets, which
    // would differ, are not implemented.)
    let _ = relative_to;

    let target = match source {
        RuleSource::List => {
            let urls = parse_url_list(rule.get("urls")?, base_url, warn)?;
            RuleTarget::List(urls)
        },
        RuleSource::Document => {
            let predicate = match rule.get("where") {
                Some(where_value) => {
                    match parse_document_predicate(where_value, base_url, document_url, warn) {
                        Some(predicate) => predicate,
                        None => return None,
                    }
                },
                // No predicate matches every link in the document.
                None => DocumentPredicate::All,
            };
            RuleTarget::Document(predicate)
        },
    };

    // Step: eagerness. The default depends on the source.
    let default_eagerness = match source {
        RuleSource::List => Eagerness::Immediate,
        RuleSource::Document => Eagerness::Conservative,
    };
    let eagerness = match rule.get("eagerness") {
        None => default_eagerness,
        Some(Value::String(eagerness)) => match Eagerness::from_str(eagerness) {
            Some(eagerness) => eagerness,
            None => {
                warn("the eagerness was invalid");
                return None;
            },
        },
        Some(_) => {
            warn("the eagerness was invalid");
            return None;
        },
    };

    // Step: referrer policy.
    let referrer_policy = match rule.get("referrer_policy") {
        None => None,
        Some(Value::String(policy)) => match parse_referrer_policy(policy) {
            Some(policy) => Some(policy),
            None => {
                warn("the referrer policy was invalid");
                return None;
            },
        },
        Some(_) => {
            warn("the referrer policy was invalid");
            return None;
        },
    };

    // Step: requirements.
    let mut anonymous_client_ip = false;
    if let Some(requires) = rule.get("requires") {
        let Some(requires) = requires.as_array() else {
            warn("the requirements were not understood");
            return None;
        };
        for requirement in requires {
            match requirement.as_str() {
                Some("anonymous-client-ip-when-cross-origin") => anonymous_client_ip = true,
                _ => {
                    warn("the requirements were not understood");
                    return None;
                },
            }
        }
    }

    // Step: the No-Vary-Search hint.
    let no_vary_search_hint = match rule.get("expects_no_vary_search") {
        None => UrlSearchVariance::default(),
        Some(Value::String(hint)) => match UrlSearchVariance::parse(hint) {
            Some(hint) => hint,
            None => {
                warn("the No-Vary-Search hint was invalid");
                return None;
            },
        },
        Some(_) => {
            warn("the No-Vary-Search hint was invalid");
            return None;
        },
    };

    // Step: the rule-level tag. A missing tag means "no tag"; a present but
    // invalid tag (non-string, or not printable ASCII) drops just this rule.
    let tag = match rule.get("tag") {
        None => None,
        Some(Value::String(tag)) if is_valid_speculation_rule_tag(tag) => Some(tag.clone()),
        Some(_) => {
            warn("the speculation rule tag is invalid");
            return None;
        },
    };

    // `target_hint` is parsed but otherwise ignored at this level of support.
    let _ = rule.get("target_hint");

    Some(SpeculationRule {
        target,
        eagerness,
        referrer_policy,
        tag,
        anonymous_client_ip,
        no_vary_search_hint,
    })
}

fn parse_url_list(
    value: &Value,
    base_url: &ServoUrl,
    warn: &mut dyn FnMut(&str),
) -> Option<Vec<ServoUrl>> {
    let Some(urls) = value.as_array() else {
        warn("the supplied URL list was invalid");
        return None;
    };
    let mut parsed: Vec<ServoUrl> = Vec::new();
    for url in urls {
        let Some(url) = url.as_str() else {
            warn("the supplied URL must be a string");
            continue;
        };
        let Ok(mut parsed_url) = ServoUrl::parse_with_base(Some(base_url), url) else {
            warn("the supplied URL string was unparseable");
            continue;
        };
        if !matches!(parsed_url.scheme(), "http" | "https") {
            warn("the supplied URL string was unparseable");
            continue;
        }
        // Speculative loads never carry a fragment.
        parsed_url.set_fragment(None);
        if !parsed.contains(&parsed_url) {
            parsed.push(parsed_url);
        }
    }
    if parsed.is_empty() {
        warn("the supplied URL list was invalid");
        return None;
    }
    Some(parsed)
}

fn parse_document_predicate(
    value: &Value,
    base_url: &ServoUrl,
    document_url: &ServoUrl,
    warn: &mut dyn FnMut(&str),
) -> Option<DocumentPredicate> {
    let Some(predicate) = value.as_object() else {
        warn("the document rule predicate was invalid");
        return None;
    };

    for key in predicate.keys() {
        if !PREDICATE_KEYS.contains(&key.as_str()) {
            warn(&format!(
                "the document rule predicate had unexpected extra options: {key}"
            ));
            return None;
        }
    }

    let clause_keys: Vec<&String> = predicate
        .keys()
        .filter(|key| key.as_str() != "relative_to")
        .collect();
    if clause_keys.len() != 1 {
        warn("the document rule predicate was empty or ambiguous");
        return None;
    }
    let clause = clause_keys[0].as_str();

    // `relative_to` is only valid alongside `href_matches`.
    let relative_to = match predicate.get("relative_to") {
        None => "document",
        Some(Value::String(relative_to)) => match relative_to.as_str() {
            "ruleset" => "ruleset",
            "document" => "document",
            _ => {
                warn("the document rule predicate had an invalid value for relative_to");
                return None;
            },
        },
        Some(_) => {
            warn("the document rule predicate had an invalid value for relative_to");
            return None;
        },
    };
    if predicate.contains_key("relative_to") && clause != "href_matches" {
        warn("the document rule predicate had unexpected extra options");
        return None;
    }

    match clause {
        "and" | "or" => {
            let Some(clauses) = predicate.get(clause).and_then(Value::as_array) else {
                warn("the document rule predicate had an invalid clause list");
                return None;
            };
            if clauses.is_empty() {
                warn("the document rule predicate had an invalid clause list");
                return None;
            }
            let mut parsed = Vec::with_capacity(clauses.len());
            for clause_value in clauses {
                parsed.push(parse_document_predicate(
                    clause_value,
                    base_url,
                    document_url,
                    warn,
                )?);
            }
            Some(if clause == "and" {
                DocumentPredicate::And(parsed)
            } else {
                DocumentPredicate::Or(parsed)
            })
        },
        "not" => {
            let clause_value = predicate.get("not")?;
            let inner = parse_document_predicate(clause_value, base_url, document_url, warn)?;
            Some(DocumentPredicate::Not(Box::new(inner)))
        },
        "href_matches" => {
            let Some(pattern) = predicate.get("href_matches").and_then(Value::as_str) else {
                warn("the document rule predicate was invalid");
                return None;
            };
            let pattern_base = if relative_to == "ruleset" {
                base_url
            } else {
                document_url
            };
            match parse_url_pattern(pattern, pattern_base) {
                Some(pattern) => Some(DocumentPredicate::HrefMatches(pattern)),
                None => {
                    warn("the supplied URL pattern was invalid");
                    None
                }
            }
        },
        "selector_matches" => {
            let Some(selector) = predicate.get("selector_matches").and_then(Value::as_str) else {
                warn("the document rule predicate was invalid");
                return None;
            };
            match SelectorParser::parse_author_origin_no_namespace(
                selector,
                &UrlExtraData(document_url.get_arc()),
            ) {
                Ok(selectors) => Some(DocumentPredicate::SelectorMatches(Box::new(selectors))),
                Err(_) => {
                    warn("the supplied selector list was invalid");
                    None
                }
            }
        },
        _ => {
            warn("the document rule predicate was invalid");
            None
        },
    }
}

fn parse_url_pattern(pattern: &str, base_url: &ServoUrl) -> Option<UrlPattern> {
    let init = quirks::process_construct_pattern_input(
        quirks::StringOrInit::String(pattern.to_string()),
        Some(base_url.as_str()),
    )
    .ok()?;
    UrlPattern::parse(init, urlpattern::UrlPatternOptions::default()).ok()
}

/// Test a URL pattern against an absolute URL.
pub(crate) fn url_pattern_matches(pattern: &UrlPattern, url: &ServoUrl) -> bool {
    let Ok(Some((input, _))) = quirks::process_match_input(
        quirks::StringOrInit::String(url.as_str().to_string()),
        None,
    ) else {
        return false;
    };
    pattern.test(input).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<SpeculationRuleSet, String> {
        let base = ServoUrl::parse("https://example.com/dir/page.html").unwrap();
        let mut warnings: Vec<String> = Vec::new();
        let mut warn = |warning: &str| warnings.push(warning.to_string());
        parse_speculation_rule_set(text, &base, &base, &mut warn)
    }

    #[test]
    fn parses_a_minimal_prefetch_list_rule() {
        let rules = parse(r#"{"prefetch": [{"source": "list", "urls": ["/next"]}]}"#).unwrap();
        assert_eq!(rules.prefetch.len(), 1);
        let rule = &rules.prefetch[0];
        assert_eq!(rule.eagerness, Eagerness::Immediate);
        match &rule.target {
            RuleTarget::List(urls) => {
                assert_eq!(urls.len(), 1);
                assert_eq!(urls[0].as_str(), "https://example.com/next");
            },
            RuleTarget::Document(_) => panic!("expected a list rule"),
        }
    }

    #[test]
    fn infers_the_source_from_urls_and_where() {
        let list = parse(r#"{"prefetch": [{"urls": ["/a"]}]}"#).unwrap();
        assert!(matches!(&list.prefetch[0].target, RuleTarget::List(_)));

        let document = parse(r#"{"prefetch": [{"where": {"selector_matches": "a"}}]}"#).unwrap();
        assert!(matches!(
            &document.prefetch[0].target,
            RuleTarget::Document(_)
        ));
        assert_eq!(document.prefetch[0].eagerness, Eagerness::Conservative);
    }

    #[test]
    fn rejects_top_level_misconfiguration() {
        assert_eq!(
            parse("not json").err().as_deref(),
            Some(TOP_LEVEL_NOT_OBJECT)
        );
        assert_eq!(parse("[]").err().as_deref(), Some(TOP_LEVEL_NOT_OBJECT));
        assert_eq!(
            parse(r#"{"tag": 1}"#).err().as_deref(),
            Some(RULESET_TAG_INVALID)
        );
    }

    #[test]
    fn drops_rules_with_unrecognized_keys() {
        let rules = parse(
            r#"{"prefetch": [{"source": "list", "urls": ["/a"], "bogus": 1}, {"source": "list", "urls": ["/b"]}]}"#,
        )
        .unwrap();
        assert_eq!(rules.prefetch.len(), 1);
        match &rules.prefetch[0].target {
            RuleTarget::List(urls) => assert_eq!(urls[0].as_str(), "https://example.com/b"),
            RuleTarget::Document(_) => panic!("expected a list rule"),
        }
    }

    #[test]
    fn strips_fragments_and_dedupes_urls() {
        let rules =
            parse(r#"{"prefetch": [{"urls": ["/a#one", "/a#two"]}]}"#).unwrap();
        match &rules.prefetch[0].target {
            RuleTarget::List(urls) => assert_eq!(urls.len(), 1),
            RuleTarget::Document(_) => panic!("expected a list rule"),
        }
    }

    #[test]
    fn collects_and_sorts_tags() {
        let rules = parse(
            r#"{"tag": "ruleset", "prefetch": [{"urls": ["/a"], "tag": "rule"}, {"urls": ["/b"]}]}"#,
        )
        .unwrap();
        assert_eq!(
            rules.candidate_tags(&rules.prefetch[0]),
            vec![Some("rule".to_string()), Some("ruleset".to_string())]
        );
        assert_eq!(
            rules.candidate_tags(&rules.prefetch[1]),
            vec![Some("ruleset".to_string())]
        );

        let untagged = parse(r#"{"prefetch": [{"urls": ["/a"]}]}"#).unwrap();
        assert_eq!(untagged.candidate_tags(&untagged.prefetch[0]), vec![None]);
        assert_eq!(
            serialize_sec_speculation_tags(&untagged.candidate_tags(&untagged.prefetch[0])),
            "null"
        );
    }

    #[test]
    fn serializes_structured_field_strings() {
        assert_eq!(sfv::serialize_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(
            serialize_sec_speculation_tags(&[
                Some("conservative".to_string()),
                Some("moderate".to_string())
            ]),
            "\"conservative\", \"moderate\""
        );
    }

    #[test]
    fn validates_tag_printable_ascii() {
        assert!(is_valid_speculation_rule_tag(""));
        assert!(is_valid_speculation_rule_tag(" "));
        assert!(is_valid_speculation_rule_tag("~"));
        assert!(is_valid_speculation_rule_tag("\""));
        assert!(!is_valid_speculation_rule_tag("\u{19}"));
        assert!(!is_valid_speculation_rule_tag("\u{7f}"));
        assert!(!is_valid_speculation_rule_tag("my\nrules"));
        assert!(!is_valid_speculation_rule_tag("マイルール"));

        // A bad ruleset-level tag discards the whole rule set.
        assert_eq!(
            parse("{\"tag\": \"\\u0019\", \"prefetch\": [{\"urls\": [\"/a\"]}]}")
                .err()
                .as_deref(),
            Some(RULESET_TAG_INVALID)
        );
        assert_eq!(
            parse(r#"{"tag": null}"#).err().as_deref(),
            Some(RULESET_TAG_INVALID)
        );

        // A bad rule-level tag (including JSON null) drops only that rule.
        let rules = parse(
            r#"{"prefetch": [{"urls": ["/a"], "tag": null}, {"urls": ["/b"], "tag": "\u0019"}, {"urls": ["/c"]}]}"#,
        )
        .unwrap();
        assert_eq!(rules.prefetch.len(), 1);
        match &rules.prefetch[0].target {
            RuleTarget::List(urls) => assert_eq!(urls[0].as_str(), "https://example.com/c"),
            RuleTarget::Document(_) => panic!("expected a list rule"),
        }
    }

    #[test]
    fn merges_tags_across_rules_and_rulesets() {
        let rules = parse(
            r#"{"tag": "def", "prefetch": [{"urls": ["/a"], "tag": "def"}, {"urls": ["/a"]}, {"urls": ["/a"], "tag": "abc"}]}"#,
        )
        .unwrap();
        // "def" is deduplicated; a rule without a tag inherits the ruleset tag
        // and never contributes the null tag when a ruleset tag exists.
        assert_eq!(
            rules.candidate_tags(&rules.prefetch[1]),
            vec![Some("def".to_string())]
        );
        assert_eq!(
            rules.candidate_tags(&rules.prefetch[2]),
            vec![Some("abc".to_string()), Some("def".to_string())]
        );
    }

    #[test]
    fn parses_search_variance() {
        let exact = UrlSearchVariance::parse("params=?0").unwrap();
        assert_eq!(exact.mode, SearchVarianceMode::Exact);

        let all = UrlSearchVariance::parse("params").unwrap();
        assert_eq!(all.mode, SearchVarianceMode::AllParamsVary);

        let listed = UrlSearchVariance::parse(r#"params=("a" "b")"#).unwrap();
        assert_eq!(
            listed.mode,
            SearchVarianceMode::ParamsVary(vec!["a".to_string(), "b".to_string()])
        );

        let except = UrlSearchVariance::parse(r#"except=("a")"#).unwrap();
        assert_eq!(
            except.mode,
            SearchVarianceMode::ExceptParamsVary(vec!["a".to_string()])
        );

        assert!(UrlSearchVariance::parse(r#"params, except=("a")"#).is_none());
        assert!(UrlSearchVariance::parse("params=(").is_none());

        // Strictness registration (REQ-BRW-051 SFV convergence): the forms
        // below were silently tolerated (or tolerated until a later parse
        // stage) by the previous hand-rolled comma splitter and now fail.
        assert!(UrlSearchVariance::parse("params=?0,").is_none()); // trailing comma
        assert!(UrlSearchVariance::parse("params=?0,, key-order=?0").is_none()); // empty member
        assert!(UrlSearchVariance::parse("params = ?1").is_none()); // OWS around "="
        assert!(UrlSearchVariance::parse("Params=?1").is_none()); // non-lowercase key
        assert!(UrlSearchVariance::parse(r#"params=("a"#).is_none()); // unclosed quote
        // Key-order keeps accepting only booleans (explicit or bare).
        assert!(UrlSearchVariance::parse("key-order").is_some());
        assert!(UrlSearchVariance::parse(r#"key-order="1""#).is_none());
    }

    #[test]
    fn compares_urls_modulo_search_variance() {
        let a = ServoUrl::parse("https://example.com/path?a=1&b=2").unwrap();
        let b = ServoUrl::parse("https://example.com/path?b=2&a=1").unwrap();
        let c = ServoUrl::parse("https://example.com/path?a=1").unwrap();

        let exact = UrlSearchVariance::default();
        assert!(!exact.urls_equivalent(&a, &b));

        let unordered = UrlSearchVariance::parse("key-order=?0").unwrap();
        assert!(unordered.urls_equivalent(&a, &b));

        let all_vary = UrlSearchVariance::parse("params").unwrap();
        assert!(all_vary.urls_equivalent(&a, &c));

        // Only `a` may vary, so the differing `b` parameter still matters.
        let only_a_varies = UrlSearchVariance::parse(r#"params=("a")"#).unwrap();
        assert!(!only_a_varies.urls_equivalent(&a, &c));

        // Every parameter except `a` may vary, so only `a` is compared.
        let except_a = UrlSearchVariance::parse(r#"except=("a")"#).unwrap();
        assert!(except_a.urls_equivalent(&a, &c));
    }

    #[test]
    fn parses_composite_predicates() {
        let rules = parse(
            r#"{"prefetch": [{"source": "document", "where": {"and": [{"href_matches": "/*"}, {"or": [{"selector_matches": "a"}, {"not": {"selector_matches": "a.nope"}}]}]}}]}"#,
        )
        .unwrap();
        assert_eq!(rules.prefetch.len(), 1);
        assert!(matches!(
            &rules.prefetch[0].target,
            RuleTarget::Document(DocumentPredicate::And(_))
        ));
    }

    #[test]
    fn rejects_relative_to_for_document_sources() {
        let rules = parse(
            r#"{"prefetch": [{"source": "document", "relative_to": "ruleset", "where": {"selector_matches": "a"}}]}"#,
        )
        .unwrap();
        assert!(rules.prefetch.is_empty());
    }
}
