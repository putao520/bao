/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Speculation rules: registration, document rule matching and candidate
//! execution.
//!
//! BAO PATCH (REQ-BRW-051 C2, GOLDEN B): upstream Servo has no speculation
//! rules implementation. This module is the execution half of Bao's prefetch
//! support; `rules` holds the JSON grammar and `prefetch` owns the wire-level
//! request.
//!
//! ## Scope boundary: prerendering is not implemented
//!
//! `prerender` rule lists are parsed for well-formedness and then ignored.
//! `document.prerendering` and the rest of the prerendering API are never
//! exposed. This is a hard constraint: exposing the prerendering feature
//! detection surface without a prerenderer would turn the speculation-rules
//! WPT prerender family from fast errors into long timeouts, and would lie to
//! content about a capability Bao does not have.
//!
//! ## What is implemented
//!
//! * `<script type="speculationrules">` inline rule sets are parsed and
//!   registered per document (`HTMLScriptElement` drives registration).
//! * `source: "list"` prefetch rules start a navigational prefetch when their
//!   eagerness allows it.
//! * `source: "document"` prefetch rules are evaluated against the document's
//!   rendered `a`/`area` links on registration, on DOM mutations and on link
//!   interactions; `and`/`or`/`not`/`href_matches`/`selector_matches`
//!   predicates are supported.
//! * Eagerness: `immediate` starts at registration; `eager`/`moderate` start
//!   when a matching link is hovered; `conservative` starts on pointerdown.
//! * Tags (ruleset-level and rule-level) are collected, deduplicated and sent
//!   as `Sec-Speculation-Tags`.
//!
//! ## Known gaps (declared, not stubbed)
//!
//! * No prefetch cache: a navigation does not consume a prefetched response,
//!   so `deliveryType`, the navigation-side `Sec-Purpose` assertions and
//!   `No-Vary-Search` navigation matching are not observable.
//! * The rendered-link check uses computed style (`display: none`) when style
//!   data is available; `content-visibility: hidden` subtrees and links inside
//!   `<select>`/`<option>` are not filtered.
//! * The `Speculation-Rules` HTTP response header (out-of-document rule sets)
//!   is not implemented.

pub(crate) mod prefetch;
pub(crate) mod rules;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use embedder_traits::{ConsoleLogLevel, EmbedderMsg};
use html5ever::local_name;
use js::context::JSContext;
use net_traits::request::RequestId;
use net_traits::ReferrerPolicy;
use selectors::parser::SelectorList;
use servo_url::ServoUrl;
use style::dom_apis;
use style::selector_parser::SelectorImpl;

pub(crate) use self::rules::LinkInteraction;
use self::rules::{
    DocumentPredicate, Eagerness, RuleTarget, SpeculationRule, SpeculationRuleSet,
    UrlSearchVariance, url_pattern_matches,
};
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot, LayoutDom, ToLayout};
use crate::dom::document::Document;
use crate::dom::element::{Element, referrer_policy_for_element};
use crate::dom::node::Node;
use crate::dom::node::iterators::ShadowIncluding;
use crate::layout_dom::ServoDangerousStyleElement;

/// Monotonic identifier handed to each registered rule set so that the script
/// element that owns it can unregister it later.
static NEXT_RULE_SET_KEY: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh rule set key.
pub(crate) fn next_rule_set_key() -> u64 {
    NEXT_RULE_SET_KEY.fetch_add(1, Ordering::Relaxed)
}

/// The per-document speculation rules registry.
pub(crate) struct SpeculationRulesState {
    /// Rule sets created by `<script type="speculationrules">` elements.
    rule_sets: Vec<RegisteredRuleSet>,
    /// Prefetch requests already started, so that equivalent candidates are
    /// not fetched repeatedly.
    started: Vec<StartedPrefetch>,
    /// Whether a "consider speculative loads" update is already scheduled.
    update_scheduled: Cell<bool>,
}

struct RegisteredRuleSet {
    element_key: u64,
    rules: Rc<SpeculationRuleSet>,
}

struct StartedPrefetch {
    element_key: u64,
    url: ServoUrl,
    request_ids: Vec<RequestId>,
}

impl Default for SpeculationRulesState {
    fn default() -> Self {
        Self {
            rule_sets: Vec::new(),
            started: Vec::new(),
            update_scheduled: Cell::new(false),
        }
    }
}

impl SpeculationRulesState {
    fn register(&mut self, element_key: u64, rules: Rc<SpeculationRuleSet>) {
        self.rule_sets.push(RegisteredRuleSet { element_key, rules });
    }

    /// Remove a rule set and return the ids of its in-flight prefetches.
    fn unregister(&mut self, element_key: u64) -> Vec<RequestId> {
        self.rule_sets
            .retain(|registered| registered.element_key != element_key);
        let mut cancelled = Vec::new();
        self.started.retain(|started| {
            if started.element_key == element_key {
                cancelled.extend(started.request_ids.iter().copied());
                false
            } else {
                true
            }
        });
        cancelled
    }

    fn begin_update(&mut self) -> bool {
        let already_scheduled = self.update_scheduled.get();
        self.update_scheduled.set(true);
        !already_scheduled
    }

    fn finish_update(&mut self) {
        self.update_scheduled.set(false);
    }

    fn has_document_rules(&self) -> bool {
        self.rule_sets.iter().any(|registered| {
            registered
                .rules
                .prefetch
                .iter()
                .any(|rule| matches!(rule.target, RuleTarget::Document(_)))
        })
    }

    fn snapshot(&self) -> Vec<(u64, Rc<SpeculationRuleSet>)> {
        self.rule_sets
            .iter()
            .map(|registered| (registered.element_key, registered.rules.clone()))
            .collect()
    }

    fn has_started(&self, url: &ServoUrl) -> bool {
        self.started.iter().any(|started| &started.url == url)
    }

    fn mark_started(&mut self, element_key: u64, url: ServoUrl, request_id: RequestId) {
        if let Some(started) = self.started.iter_mut().find(|started| started.url == url) {
            started.request_ids.push(request_id);
            return;
        }
        self.started.push(StartedPrefetch {
            element_key,
            url,
            request_ids: vec![request_id],
        });
    }
}

/// Parse the text of an inline `<script type="speculationrules">` element.
///
/// A top-level failure returns the message that must be turned into a
/// `TypeError` for the `window` `error` event.
pub(crate) fn parse_inline_rule_set(
    document: &Document,
    text: &str,
) -> Result<Rc<SpeculationRuleSet>, String> {
    let base_url = document.base_url();
    let document_url = document.url();
    let mut warn = |message: &str| console_warn(document, message);
    rules::parse_speculation_rule_set(text, &base_url, &document_url, &mut warn).map(Rc::new)
}

fn console_warn(document: &Document, message: &str) {
    let global = document.global();
    global.send_chrome_notification_message_to_embedder(EmbedderMsg::ShowConsoleApiMessage(
        global.webview_id(),
        ConsoleLogLevel::Warn,
        format!("Speculation rules: {message}"),
    ));
}

/// Register a parsed rule set for `document` and schedule candidate
/// consideration.
pub(crate) fn register_rule_set(
    document: &Document,
    element_key: u64,
    rule_set: Rc<SpeculationRuleSet>,
) {
    document
        .speculation_rules_mut()
        .register(element_key, rule_set);
    schedule_update(document);
}

/// Unregister a rule set, cancelling any prefetch it started.
pub(crate) fn unregister_rule_set(document: &Document, element_key: u64) {
    let cancelled = document.speculation_rules_mut().unregister(element_key);
    prefetch::cancel_prefetches(document, &cancelled);
}

/// Note that the document's links or rules may have changed, scheduling a
/// coalesced "consider speculative loads" pass.
pub(crate) fn document_maybe_changed(document: &Document) {
    let should_schedule = {
        let mut state = document.speculation_rules_mut();
        state.has_document_rules() && state.begin_update()
    };
    if should_schedule {
        schedule_consider_task(document);
    }
}

fn schedule_update(document: &Document) {
    let should_schedule = document.speculation_rules_mut().begin_update();
    if should_schedule {
        schedule_consider_task(document);
    }
}

fn schedule_consider_task(document: &Document) {
    // A real task on the DOM manipulation task source (rather than a delayed
    // task, which only drains when a script/layout blocker is removed) keeps
    // this independent of unrelated blocking activity. The document is held
    // through a `Trusted` handle because queued tasks must be `Send`.
    let global = document.global();
    let document = Trusted::new(document);
    global
        .task_manager()
        .dom_manipulation_task_source()
        .queue(task!(ConsiderSpeculativeLoads: move |cx| {
            let document = document.root();
            consider_speculative_loads(cx, &document);
        }));
}

/// <https://html.spec.whatwg.org/multipage/speculative-loading.html#consider-speculative-loads>
///
/// This pass turns the registered rules into prefetch candidates and starts
/// every candidate whose eagerness allows an immediate load.
pub(crate) fn consider_speculative_loads(cx: &mut JSContext, document: &Document) {
    document.speculation_rules_mut().finish_update();

    let rule_sets = document.speculation_rules().snapshot();
    if rule_sets.is_empty() || !document.is_fully_active() {
        return;
    }

    // Duplicate ids may need to be resolved before selector matching.
    document
        .id_map()
        .resolve_all(cx.no_gc(), document.upcast::<Node>());

    let links = collect_document_links(document);
    let mut candidates = Vec::new();
    for (element_key, rule_set) in &rule_sets {
        for rule in &rule_set.prefetch {
            match &rule.target {
                RuleTarget::List(urls) => {
                    let referrer_policy = rule
                        .referrer_policy
                        .unwrap_or_else(|| document.get_referrer_policy());
                    for url in urls {
                        candidates.push(candidate(
                            *element_key,
                            rule_set,
                            rule,
                            url.clone(),
                            referrer_policy,
                        ));
                    }
                },
                RuleTarget::Document(predicate) => {
                    for link in &links {
                        if !predicate_matches(document, &link.element, &link.url, predicate) {
                            continue;
                        }
                        let referrer_policy =
                            rule.referrer_policy.unwrap_or(link.referrer_policy);
                        candidates.push(candidate(
                            *element_key,
                            rule_set,
                            rule,
                            link.url.clone(),
                            referrer_policy,
                        ));
                    }
                },
            }
        }
    }

    start_candidates(document, candidates, None);
}

/// Handle a link interaction that can start non-immediate candidates.
pub(crate) fn handle_link_interaction(
    _cx: &mut JSContext,
    document: &Document,
    target: &Element,
    interaction: LinkInteraction,
) {
    let rule_sets = document.speculation_rules().snapshot();
    if rule_sets.is_empty() {
        return;
    }

    let Some(link) = find_link_element(target) else {
        return;
    };
    let Some(url) = link_url(document, &link) else {
        return;
    };

    let link_referrer_policy = referrer_policy_for_element(&link);
    let mut candidates = Vec::new();
    for (element_key, rule_set) in &rule_sets {
        for rule in &rule_set.prefetch {
            if !rule.eagerness.triggered_by(interaction) {
                continue;
            }
            match &rule.target {
                RuleTarget::List(urls) => {
                    if !urls
                        .iter()
                        .any(|candidate_url| {
                            rule.no_vary_search_hint
                                .urls_equivalent(candidate_url, &url)
                        })
                    {
                        continue;
                    }
                    let referrer_policy =
                        rule.referrer_policy.unwrap_or(link_referrer_policy);
                    candidates.push(candidate(
                        *element_key,
                        rule_set,
                        rule,
                        url.clone(),
                        referrer_policy,
                    ));
                },
                RuleTarget::Document(predicate) => {
                    if !predicate_matches(document, &link, &url, predicate) {
                        continue;
                    }
                    let referrer_policy =
                        rule.referrer_policy.unwrap_or(link_referrer_policy);
                    candidates.push(candidate(
                        *element_key,
                        rule_set,
                        rule,
                        url.clone(),
                        referrer_policy,
                    ));
                },
            }
        }
    }

    start_candidates(document, candidates, Some(interaction));
}

struct Candidate {
    element_key: u64,
    url: ServoUrl,
    eagerness: Eagerness,
    referrer_policy: ReferrerPolicy,
    tags: Vec<Option<String>>,
    anonymous_client_ip: bool,
    no_vary_search_hint: UrlSearchVariance,
}

fn candidate(
    element_key: u64,
    rule_set: &SpeculationRuleSet,
    rule: &SpeculationRule,
    url: ServoUrl,
    referrer_policy: ReferrerPolicy,
) -> Candidate {
    Candidate {
        element_key,
        url,
        eagerness: rule.eagerness,
        referrer_policy,
        tags: rule_set.candidate_tags(rule),
        anonymous_client_ip: rule.anonymous_client_ip,
        no_vary_search_hint: rule.no_vary_search_hint.clone(),
    }
}

/// Start prefetch requests for candidates allowed by `interaction`
/// (`None` meaning "at registration time", which only permits `immediate`).
fn start_candidates(
    document: &Document,
    candidates: Vec<Candidate>,
    interaction: Option<LinkInteraction>,
) {
    // Group redundant candidates: same URL (exact match) and same
    // No-Vary-Search hint. Tags from every candidate in a group are merged.
    let mut groups: Vec<CandidateGroup> = Vec::new();
    for candidate in candidates {
        let allowed = match interaction {
            None => candidate.eagerness == Eagerness::Immediate,
            Some(interaction) => candidate.eagerness.triggered_by(interaction),
        };
        if !allowed {
            continue;
        }
        if !candidate.url.is_potentially_trustworthy() {
            continue;
        }
        // Same-document URLs (identical modulo the fragment) only trigger a
        // fragment navigation and must never be prefetched.
        if candidate
            .url
            .is_equal_excluding_fragments(&document.url())
        {
            continue;
        }
        let existing = groups.iter().position(|group| {
            group.url == candidate.url &&
                group.no_vary_search_hint == candidate.no_vary_search_hint
        });
        match existing {
            Some(index) => {
                let group = &mut groups[index];
                group.tags.extend(candidate.tags);
                group.element_key = candidate.element_key;
                group.referrer_policy = candidate.referrer_policy;
                group.anonymous_client_ip |= candidate.anonymous_client_ip;
            },
            None => groups.push(CandidateGroup {
                element_key: candidate.element_key,
                url: candidate.url,
                tags: candidate.tags,
                referrer_policy: candidate.referrer_policy,
                anonymous_client_ip: candidate.anonymous_client_ip,
                no_vary_search_hint: candidate.no_vary_search_hint,
            }),
        }
    }

    for mut group in groups {
        if document.speculation_rules().has_started(&group.url) {
            continue;
        }
        rules::sort_and_dedupe_tags(&mut group.tags);
        let request_id = prefetch::start_prefetch(
            document,
            group.url.clone(),
            group.referrer_policy,
            group.tags,
            group.anonymous_client_ip,
        );
        document
            .speculation_rules_mut()
            .mark_started(group.element_key, group.url, request_id);
    }
}

struct CandidateGroup {
    element_key: u64,
    url: ServoUrl,
    tags: Vec<Option<String>>,
    referrer_policy: ReferrerPolicy,
    anonymous_client_ip: bool,
    no_vary_search_hint: UrlSearchVariance,
}

struct DocumentLink {
    element: DomRoot<Element>,
    url: ServoUrl,
    referrer_policy: ReferrerPolicy,
}

/// Collect the links that document rules can match: rendered `a`/`area`
/// elements with an HTTP(S) `href`, excluding links that only differ from the
/// current document by fragment.
fn collect_document_links(document: &Document) -> Vec<DocumentLink> {
    let mut links = Vec::new();
    let document_url = document.url();
    let root = document.upcast::<Node>();
    for node in root.traverse_preorder(ShadowIncluding::Yes) {
        let Some(element) = DomRoot::downcast::<Element>(node) else {
            continue;
        };
        if !is_link_element(&element) || !element.has_attribute(&local_name!("href")) {
            continue;
        }
        let Some(url) = link_url(document, &element) else {
            continue;
        };
        // A link that only differs from the current document by its fragment
        // is a same-document navigation and cannot be prefetched.
        if url.is_equal_excluding_fragments(&document_url) {
            continue;
        }
        // Skip links that are known not to be rendered. When style data has
        // not been computed yet this conservatively assumes the link is
        // rendered; `content-visibility: hidden` subtrees are a known gap.
        if element.is_styled() && element.is_display_none() {
            continue;
        }
        // Unslotted light-DOM children of a shadow host are not rendered.
        if let Some(parent) = element.upcast::<Node>().GetParentNode() &&
            let Some(parent_element) = parent.downcast::<Element>() &&
            parent_element.shadow_root().is_some() &&
            element.upcast::<Node>().assigned_slot().is_none()
        {
            continue;
        }
        let referrer_policy = referrer_policy_for_element(&element);
        links.push(DocumentLink {
            element,
            url,
            referrer_policy,
        });
    }
    links
}

/// Resolve the URL of an `a`/`area` element, or `None` if it has no usable
/// HTTP(S) `href`.
fn link_url(document: &Document, element: &Element) -> Option<ServoUrl> {
    let href = element.get_attribute_string_value(&local_name!("href"))?;
    if href.is_empty() {
        return None;
    }
    let Ok(mut url) = document.base_url().join(&href) else {
        return None;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// Whether `element` is an `a` or `area` element.
fn is_link_element(element: &Element) -> bool {
    let name = element.local_name();
    name == &local_name!("a") || name == &local_name!("area")
}

/// Find the nearest `a`/`area` element with an `href`, starting from `target`.
fn find_link_element(target: &Element) -> Option<DomRoot<Element>> {
    for node in target
        .upcast::<Node>()
        .inclusive_ancestors(ShadowIncluding::Yes)
    {
        let Some(element) = DomRoot::downcast::<Element>(node) else {
            continue;
        };
        if is_link_element(&element) && element.has_attribute(&local_name!("href")) {
            return Some(element);
        }
    }
    None
}

fn predicate_matches(
    document: &Document,
    element: &Element,
    url: &ServoUrl,
    predicate: &DocumentPredicate,
) -> bool {
    match predicate {
        DocumentPredicate::All => true,
        DocumentPredicate::And(predicates) => predicates
            .iter()
            .all(|predicate| predicate_matches(document, element, url, predicate)),
        DocumentPredicate::Or(predicates) => predicates
            .iter()
            .any(|predicate| predicate_matches(document, element, url, predicate)),
        DocumentPredicate::Not(predicate) => !predicate_matches(document, element, url, predicate),
        DocumentPredicate::HrefMatches(pattern) => url_pattern_matches(pattern, url),
        DocumentPredicate::SelectorMatches(selectors) => {
            selector_matches(document, element, selectors)
        },
    }
}

/// <https://dom.spec.whatwg.org/#concept-element-selectors-matching>
///
/// This mirrors `Element::Matches`: selector matching requires the layout
/// thread state (the `selectors` implementation works on `LayoutDom`), even
/// though nothing is mutated and no GC can happen while matching.
#[allow(unsafe_code)]
fn selector_matches(
    document: &Document,
    element: &Element,
    selectors: &SelectorList<SelectorImpl>,
) -> bool {
    let quirks_mode = document.quirks_mode();
    // SAFETY: `element` outlives this call, matching does not GC and does not
    // mutate the DOM; this is the same pattern used by `Element::Matches`.
    let traced_element = Dom::from_ref(element);
    layout_api::with_layout_state(|| {
        let layout_element: LayoutDom<'_, _> = unsafe { traced_element.to_layout() };
        dom_apis::element_matches(
            &ServoDangerousStyleElement::from(layout_element.upcast()),
            selectors,
            quirks_mode,
        )
    })
}
