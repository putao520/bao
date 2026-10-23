/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Web Bundle subresource loading: `<script type=webbundle>` (BAO PATCH,
//! REQ-BRW-051).
//!
//! A `<script type=webbundle>` element is a data block whose text content is a
//! JSON rule:
//!
//! ```json
//! { "source": "bundle.wbn",
//!   "resources": ["script.js", "https://example.test/x.js"],
//!   "scopes": ["assets/"],
//!   "credentials": "same-origin" }
//! ```
//!
//! Processing the element (on insertion) resolves `source` against the
//! document base URL, resolves `resources`/`scopes` against the *bundle* URL,
//! registers the rule with the net-side bundle registry, and fetches the
//! bundle. The bundle body is parsed on the script thread when the fetch
//! completes; the decoded responses and the bundle's resource timing are sent
//! to the net registry, which then serves matching subresource requests (the
//! registry lives in `net::webbundle`).
//!
//! Lifetimes: one *loader* per `(bundle URL, credentials mode)` pair is shared
//! between elements in the same document (only through the orphan hand-off
//! below). Removing an element drops its registration and orphans its loader;
//! an element appended synchronously afterwards adopts the orphaned loader and
//! reuses its in-flight/loaded bundle (WPT reuse-web-bundle-resource). The
//! orphan is destroyed by a microtask, so a separate task re-fetches.
//!
//! Scope note (Chrome oracle, 155.0.8059.39): the link-based form of the
//! explainer (`<link rel=webbundle>`) is **not** implemented on purpose —
//! Chrome reports `link.relList.supports("webbundle") === false` and
//! `"webbundle" in link.relList === false`, so exposing it would create a
//! detectable difference. Only the `<script type=webbundle>` form (which Chrome
//! implements and reports through `HTMLScriptElement.supports`) is provided.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use http::{HeaderMap, HeaderValue, header::ACCEPT};
use js::context::JSContext;
use js::realm::CurrentRealm;
use log::warn;
use net_traits::blob_url_store::UrlWithBlobClaim;
use net_traits::request::{
    CorsSettings, CredentialsMode, Destination, Initiator, ServiceWorkersMode,
};
use net_traits::{
    CoreResourceMsg, ResourceFetchTiming, WebBundleLoaderData, WebBundleResource, WebBundleRule,
};
use serde_json::Value as JsonValue;
use servo_url::ServoUrl;

use crate::dom::bindings::error::{Error, report_pending_exception, throw_dom_exception};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::document::Document;
use crate::dom::event::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::html::scripting::htmlscriptelement::HTMLScriptElement;
use crate::dom::node::NodeTraits;
use crate::fetch::fetch::{RequestWithGlobalScope, create_a_potential_cors_request_with_claim};
use crate::runtime::job_queue::MicrotaskRunnable;

/// Per-element identifiers. Process-unique so a stale message can never match a
/// live element/loader.
static NEXT_ELEMENT_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_LOADER_ID: AtomicU64 = AtomicU64::new(1);

/// A loader is identified by its bundle URL and the element's credentials mode:
/// elements with the same pair share a bundle fetch when one adopts the other's
/// orphaned loader; a different credentials mode must re-fetch (WPT
/// "'remove(), then append()' ... different credentials mode").
pub(crate) type WebBundleLoaderKey = (String, CredentialsMode);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebBundleLoaderState {
    /// The bundle request (or its parse) is in flight.
    Loading,
    /// The bundle was parsed and registered with the net side.
    Loaded,
    /// The bundle request or parse failed; matching subresources fail closed.
    Failed,
}

/// One shared bundle loader. Owned either by the element currently using it or,
/// briefly, by the document's orphan map after that element is removed.
pub(crate) struct WebBundleLoader {
    pub id: u64,
    pub key: WebBundleLoaderKey,
    pub bundle_url: ServoUrl,
    pub credentials: CredentialsMode,
    /// The element that receives the load/error event. `None` while orphaned.
    pub owner: RefCell<Option<Trusted<HTMLScriptElement>>>,
    pub state: RefCell<WebBundleLoaderState>,
}

/// The state a `<script type=webbundle>` element keeps after processing.
pub(crate) struct WebBundleElementState {
    pub element_id: u64,
    pub loader: Rc<WebBundleLoader>,
}

/// The document's web bundle state: every live loader plus the orphan hand-off
/// slot used by the synchronous remove/append reuse contract.
pub(crate) struct WebBundleSession {
    loaders: HashMap<u64, Rc<WebBundleLoader>>,
    /// Key → loader id for loaders whose element was removed and which have not
    /// been adopted yet. Cleaned up by a microtask.
    orphaned: HashMap<WebBundleLoaderKey, u64>,
    /// Cleared by `Document::destroy`; late fetch completions become no-ops.
    active: bool,
}

impl WebBundleSession {
    pub(crate) fn new() -> WebBundleSession {
        WebBundleSession {
            loaders: HashMap::new(),
            orphaned: HashMap::new(),
            active: true,
        }
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.loaders.clear();
        self.orphaned.clear();
    }
}

/// The resolved form of a webbundle rule.
struct ParsedRule {
    bundle_url: ServoUrl,
    credentials: CredentialsMode,
    resources: Vec<ServoUrl>,
    scopes: Vec<ServoUrl>,
}

/// Whether a `<script>` element's `type` attribute selects web bundle
/// processing. The `type` attribute is compared after stripping ASCII
/// whitespace and ASCII case-insensitively (the HTML "data block" rule); this
/// is deliberately separate from `HTMLScriptElement.supports()`, which is an
/// exact match.
pub(crate) fn is_web_bundle_type(type_attribute: &str) -> bool {
    // HTML "ASCII whitespace" (space, tab, LF, FF, CR) around the token.
    type_attribute
        .trim_matches(|character: char| character.is_ascii_whitespace())
        .eq_ignore_ascii_case("webbundle")
}

/// Process a `<script type=webbundle>` element: parse its rule, register it
/// with the net registry, and start (or adopt) the bundle loader. Rule errors
/// are reported as uncaught exceptions on the window (WPT invalid-json).
pub(crate) fn process_element(cx: &mut JSContext, element: &HTMLScriptElement, text: &str) {
    let document = element.owner_document();
    let global = document.global();

    let rule = match parse_rule(&document, text) {
        Ok(Some(rule)) => rule,
        // A JSON array rule is ignored without an error (Chrome oracle: the
        // rule must be a JSON object, but an array produces no exception).
        Ok(None) => return,
        Err(error) => {
            throw_dom_exception(cx, &global, error);
            report_pending_exception(&mut CurrentRealm::assert(cx));
            return;
        },
    };

    let key: WebBundleLoaderKey = (rule.bundle_url.as_str().to_string(), rule.credentials);
    let (loader, adopted) = {
        let mut session = document.web_bundle_session_mut();
        if !session.active {
            return;
        }
        match session.orphaned.remove(&key) {
            Some(loader_id) => match session.loaders.get(&loader_id).cloned() {
                Some(loader) => {
                    *loader.owner.borrow_mut() = Some(Trusted::new(element));
                    (loader, true)
                },
                None => create_loader(&mut session, key, &rule, element),
            },
            None => create_loader(&mut session, key, &rule, element),
        }
    };

    let element_id = NEXT_ELEMENT_ID.fetch_add(1, Ordering::Relaxed);
    let pipeline_id = document.pipeline_id();
    send_message(
        &global,
        CoreResourceMsg::WebBundleRegister(
            pipeline_id,
            WebBundleRule {
                element_id,
                loader_id: loader.id,
                bundle_url: rule.bundle_url.clone(),
                resources: rule.resources.clone(),
                scopes: rule.scopes.clone(),
            },
        ),
    );

    element.set_web_bundle_state(Some(WebBundleElementState {
        element_id,
        loader: loader.clone(),
    }));

    if adopted {
        // The adopted loader may already have settled; the new element still
        // fires the matching event (WPT reuse: "both scripts should fire load
        // events").
        match *loader.state.borrow() {
            WebBundleLoaderState::Loading => {},
            WebBundleLoaderState::Loaded => queue_element_event(element, atom!("load")),
            WebBundleLoaderState::Failed => queue_element_event(element, atom!("error")),
        }
    } else {
        start_fetch(&document, &global, &loader);
    }
}

/// Handle the removal of a `<script type=webbundle>` element: drop its
/// registration (failing in-flight subresource fetches that were waiting on it)
/// and orphan its loader for a synchronous re-append.
pub(crate) fn element_removed(cx: &mut JSContext, element: &HTMLScriptElement) {
    let Some(state) = element.take_web_bundle_state() else {
        return;
    };
    let document = element.owner_document();
    let global = document.global();
    send_message(
        &global,
        CoreResourceMsg::WebBundleUnregister(document.pipeline_id(), state.element_id),
    );

    *state.loader.owner.borrow_mut() = None;
    let key = state.loader.key.clone();
    let loader_id = state.loader.id;
    let orphaned = {
        let mut session = document.web_bundle_session_mut();
        if !session.active || !session.loaders.contains_key(&loader_id) {
            false
        } else {
            session.orphaned.insert(key.clone(), loader_id);
            true
        }
    };
    if orphaned {
        document
            .global()
            .enqueue_microtask(cx, Box::new(WebBundleOrphanCleanup {
                document: Trusted::new(&*document),
                key,
                loader_id,
            }));
    }
}

/// Document teardown: drop the whole session and every net-side registration.
pub(crate) fn destroy_document_session(document: &Document) {
    document.web_bundle_session_mut().deactivate();
    send_message(
        &document.global(),
        CoreResourceMsg::WebBundleClearPipeline(document.pipeline_id()),
    );
}

/// Called from the bundle fetch listener when the request settled. `result` is
/// the parsed response list, or a human-readable parse/fetch failure.
pub(crate) fn fetch_completed(
    cx: &mut JSContext,
    document: &Document,
    loader_id: u64,
    result: Result<Vec<WebBundleResource>, String>,
    timing: ResourceFetchTiming,
) {
    let (loader, owner) = {
        let session = document.web_bundle_session();
        if !session.active {
            return;
        }
        let Some(loader) = session.loaders.get(&loader_id).cloned() else {
            // The loader was dropped (a removed element with no adopter).
            return;
        };
        let owner = loader.owner.borrow().clone();
        (loader, owner)
    };

    let global = document.global();
    let pipeline_id = document.pipeline_id();
    match result {
        Ok(resources) => {
            // Register the data before firing load so a load handler that
            // fetches a bundled URL already sees it.
            send_message(
                &global,
                CoreResourceMsg::WebBundleLoaded(
                    pipeline_id,
                    WebBundleLoaderData {
                        loader_id,
                        resources,
                        timing,
                    },
                ),
            );
            *loader.state.borrow_mut() = WebBundleLoaderState::Loaded;
            if let Some(owner) = owner {
                owner
                    .root()
                    .upcast::<EventTarget>()
                    .fire_event(cx, atom!("load"));
            }
        },
        Err(message) => {
            warn!("Web bundle load failed: {message}");
            send_message(
                &global,
                CoreResourceMsg::WebBundleFailed(pipeline_id, loader_id),
            );
            *loader.state.borrow_mut() = WebBundleLoaderState::Failed;
            if let Some(owner) = owner {
                let element = owner.root();
                // Drop the failed element's subresource registration. Requests
                // that were already waiting on the pending loader fail closed
                // (WPT not-found / csp-blockes-bundle / element-removal), while
                // requests issued after the failure fall through to the
                // network (WPT nested-bundle's `root.js` fetch resolves with
                // 404 rather than rejecting). The element keeps its script-side
                // state so a synchronous re-append still adopts the failed
                // loader and fires `error` (WPT reuse).
                if let Some(element_id) = element.web_bundle_element_id() {
                    send_message(
                        &global,
                        CoreResourceMsg::WebBundleUnregister(pipeline_id, element_id),
                    );
                }
                element
                    .upcast::<EventTarget>()
                    .fire_event(cx, atom!("error"));
            }
        },
    }
}

/// Microtask that destroys an orphaned loader. Running in a microtask (not a
/// task) is what makes `remove(); append(newElement)` in the same task reuse
/// the loader while a `setTimeout`-separated append re-fetches.
#[derive(JSTraceable, MallocSizeOf)]
struct WebBundleOrphanCleanup {
    #[ignore_malloc_size_of = "Because it is non-owning"]
    document: Trusted<Document>,
    #[no_trace]
    #[ignore_malloc_size_of = "loader key is a small URL/credentials pair"]
    key: WebBundleLoaderKey,
    loader_id: u64,
}

impl MicrotaskRunnable for WebBundleOrphanCleanup {
    fn handler(&self, _cx: &mut JSContext) {
        let document = self.document.root();
        let dropped = {
            let mut session = document.web_bundle_session_mut();
            if !session.active || session.orphaned.get(&self.key) != Some(&self.loader_id) {
                false
            } else {
                session.orphaned.remove(&self.key);
                session.loaders.remove(&self.loader_id);
                true
            }
        };
        if dropped {
            send_message(
                &document.global(),
                CoreResourceMsg::WebBundleDropLoader(document.pipeline_id(), self.loader_id),
            );
        }
    }
}

fn create_loader(
    session: &mut WebBundleSession,
    key: WebBundleLoaderKey,
    rule: &ParsedRule,
    element: &HTMLScriptElement,
) -> (Rc<WebBundleLoader>, bool) {
    let loader = Rc::new(WebBundleLoader {
        id: NEXT_LOADER_ID.fetch_add(1, Ordering::Relaxed),
        key,
        bundle_url: rule.bundle_url.clone(),
        credentials: rule.credentials,
        owner: RefCell::new(Some(Trusted::new(element))),
        state: RefCell::new(WebBundleLoaderState::Loading),
    });
    session.loaders.insert(loader.id, loader.clone());
    (loader, false)
}

/// Send a web bundle registration message through the fetch thread's FIFO so
/// it is ordered with the fetches this script thread issues — the net side
/// decides a request's bundle binding at dispatch time, and that decision must
/// observe exactly the registrations the script had issued when the fetch was
/// initiated (not a later removal that happens to arrive first).
fn send_message(global: &GlobalScope, message: CoreResourceMsg) {
    net_traits::send_web_bundle_message(&global.core_resource_thread(), message);
}

fn queue_element_event(element: &HTMLScriptElement, name: stylo_atoms::Atom) {
    element
        .owner_global()
        .task_manager()
        .dom_manipulation_task_source()
        .queue_simple_event(element.upcast(), name);
}

fn type_error(message: &str) -> Error {
    Error::Type(CString::new(message).expect("error message must not contain NUL"))
}

/// Parse and resolve a rule. JSON syntax errors are `SyntaxError`s, every
/// structural/type problem is a `TypeError` (WPT invalid-json). `Ok(None)` means
/// the rule is a JSON array, which is ignored without an exception (Chrome
/// oracle; a `null`/number/string rule is a `TypeError`).
fn parse_rule(document: &Document, text: &str) -> Result<Option<ParsedRule>, Error> {
    let value: JsonValue = serde_json::from_str(text).map_err(|_| {
        Error::Syntax(Some(
            "The web bundle rule is not valid JSON".to_string(),
        ))
    })?;
    let object = match value.as_object() {
        Some(object) => object,
        None if value.is_array() => return Ok(None),
        None => return Err(type_error("The web bundle rule must be a JSON object")),
    };

    let source = object
        .get("source")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| type_error("The web bundle rule requires a string \"source\" member"))?;
    // The source is resolved against the document base URL. Chrome accepts any
    // resolvable URL here (a non-fetchable source fails when its request runs
    // and fires the element's `error` event rather than throwing).
    let bundle_url = document
        .base_url()
        .join(source)
        .map_err(|_| type_error("The web bundle rule \"source\" is not a valid URL"))?;

    // Only a string member selects a credentials mode; any other value
    // (including `null`, numbers and invalid strings) is the default
    // "same-origin" mode and is not an error (Chrome oracle + WPT credentials).
    let credentials = match object.get("credentials") {
        Some(JsonValue::String(value)) => match value.as_str() {
            "omit" => CredentialsMode::Omit,
            "include" => CredentialsMode::Include,
            _ => CredentialsMode::CredentialsSameOrigin,
        },
        _ => CredentialsMode::CredentialsSameOrigin,
    };

    Ok(Some(ParsedRule {
        resources: parse_rule_url_list(object.get("resources"), "resources", &bundle_url)?,
        scopes: parse_rule_url_list(object.get("scopes"), "scopes", &bundle_url)?,
        bundle_url,
        credentials,
    }))
}

fn parse_rule_url_list(
    value: Option<&JsonValue>,
    member: &str,
    bundle_url: &ServoUrl,
) -> Result<Vec<ServoUrl>, Error> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        type_error(&format!("The web bundle rule \"{member}\" must be an array"))
    })?;
    let mut urls = Vec::with_capacity(array.len());
    for item in array {
        let text = item.as_str().ok_or_else(|| {
            type_error(&format!(
                "The web bundle rule \"{member}\" must contain only strings"
            ))
        })?;
        let url = resolve_rule_url(bundle_url, text).ok_or_else(|| {
            type_error(&format!(
                "The web bundle rule \"{member}\" contains an invalid URL"
            ))
        })?;
        // Same-origin-with-bundle and path-restricted URLs only. A disallowed
        // URL is not registered, so it falls back to the network
        // (WPT path-restriction / "Subresource URL must be same-origin with
        // bundle URL").
        if rule_url_allowed(bundle_url, &url) {
            urls.push(url);
        }
    }
    Ok(urls)
}

/// Resolve a rule URL: absolute URLs are kept as-is; everything else is
/// resolved against the bundle URL (WPT relative-url-resources/scopes).
fn resolve_rule_url(bundle_url: &ServoUrl, text: &str) -> Option<ServoUrl> {
    match ServoUrl::parse(text) {
        Ok(url) => Some(url),
        Err(_) => bundle_url.join(text).ok(),
    }
}

/// Whether `url` may be served from a bundle at `bundle_url`: it must either be
/// a `uuid-in-package:` URL, or be same-origin with the bundle and live under
/// the bundle's directory (the path up to the last `/`).
fn rule_url_allowed(bundle_url: &ServoUrl, url: &ServoUrl) -> bool {
    match url.scheme() {
        webbundle::UUID_IN_PACKAGE_SCHEME => true,
        "http" | "https" => {
            url.origin() == bundle_url.origin() && {
                let path = bundle_url.path();
                let directory_end = path.rfind('/').map(|index| index + 1).unwrap_or(1);
                url.path().starts_with(&path[..directory_end])
            }
        },
        _ => false,
    }
}

/// Start the bundle request for a freshly created loader.
fn start_fetch(document: &Document, global: &GlobalScope, loader: &Rc<WebBundleLoader>) {
    let bundle_url = loader.bundle_url.clone();
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static(webbundle::ACCEPT_HEADER_VALUE),
    );
    let cors_setting = Some(match loader.credentials {
        CredentialsMode::Include => CorsSettings::UseCredentials,
        _ => CorsSettings::Anonymous,
    });
    let request = create_a_potential_cors_request_with_claim(
        Some(document.webview_id()),
        UrlWithBlobClaim::from_url_without_having_claimed_blob(bundle_url.clone()),
        // The CSP crate has no `webbundle` destination; the request is marked
        // by `Initiator::WebBundle`, which drives both the `Sec-Fetch-Dest:
        // webbundle` header and the service-worker bypass.
        Destination::None,
        cors_setting,
        None,
        global.get_referrer(),
    )
    .credentials_mode(loader.credentials)
    .service_workers_mode(ServiceWorkersMode::None)
    .initiator(Initiator::WebBundle)
    .headers(headers)
    .with_global_scope(global);

    let listener = crate::dom::webbundle_fetch::WebBundleFetchListener::new(
        Trusted::new(document),
        loader.id,
        bundle_url,
    );
    document.fetch_background(request, listener);
}
