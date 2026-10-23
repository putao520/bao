/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The DBSC HTTP exchange layer — the single registration/refresh POST
//! driver, the cookie-jar seams the drivers share with the page network, and
//! the session-level orchestration the fetch hooks call.
//!
//! BAO PATCH (REQ-BRW-051): this module is the *merged* exchange layer — the
//! former `refresh.rs` duplicated `DbscExchange`/`DbscPostError`/`dbsc_post`
//! with diverged behavior (non-fail-closed header insertion, no scratch
//! timing, no explicit Content-Length) and has been deleted; the surviving
//! forms are the ones here. Both DBSC POSTs and the
//! `.well-known/device-bound-sessions` GETs ride the page network's own
//! bridge (`crate::fetch::bun_bridge::obtain_response_bun`) so the wire leg
//! is indistinguishable from the page's other traffic (stealth TLS profile
//! resolved per webview, same connection pool). They bypass the fetch
//! pipeline deliberately: the draft defines them as UA-driven requests with
//! no CORS participation and no recursion into DBSC processing (the refresh
//! URL is excluded from its own session's scope — `DbscSession::url_in_scope`),
//! and `bun_bridge` does not run the fetch algorithm. The POSTs carry the
//! jar's cookies under the third-party attachment rule; the well-known GETs
//! carry none (credentials omitted).
//!
//! The exchange carries a scratch resource-timing container so the internal
//! POST never mutates the triggering request's observable
//! `PerformanceResourceTiming` record; state (cookie jar, cert overrides,
//! stealth registries) is shared with the triggering fetch. Store locks are
//! never held across the `.await`s below: signing happens inside the store
//! visitor (keys never leave the store — the `DbscStore::with_session`
//! contract) and every network leg runs lock-free.

use futures_util::TryStreamExt as _;
use http::{HeaderMap, HeaderValue, Method, header};
use net_traits::request::Destination;
use net_traits::{ResourceFetchTiming, ResourceTimingType};
// BAO PATCH (REQ-BRW-051): `WebViewId` lives in `servo_base::id` (the
// request struct only carries it as a field type) — the pre-fix orphan
// files referenced a nonexistent `net_traits::request::WebViewId` path,
// invisible until E1 declared the module and compilation reached it.
use servo_base::id::{PipelineId, WebViewId};
use servo_url::{ImmutableOrigin, ServoUrl};

use crate::dbsc::keys::DbscKeyPair;
use crate::dbsc::model::{
    DbscAlgorithm, DbscSession, RefreshInstructions, RequestOrigin, SessionInstructions,
    SessionRegistration, parse_challenge_header, parse_refresh_instructions,
    parse_registration_header, parse_session_instructions, same_registrable_domain,
    well_known_url,
};
use crate::dbsc::store::urls_share_site;
use crate::fetch::methods::FetchContext;
use crate::http_loader::HttpState;

/// One completed DBSC exchange: status line, response headers, full body.
pub(crate) struct DbscExchange {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

/// The outcome of a DBSC POST.
#[derive(Debug)]
pub(crate) enum DbscPostError {
    /// The network leg failed entirely (DNS / connection / TLS), or a
    /// header value the driver itself must emit was not a valid header
    /// value (a malformed proof JWT is a driver bug — fail closed as a
    /// network-class failure, never send a broken exchange).
    Network,
    /// The response body stream failed while draining.
    Body,
}

/// The DBSC request headers the drivers themselves manage; a caller-visible
/// response must never leak them.
const DBSC_REQUEST_HEADERS: [&str; 2] = ["Secure-Session-Response", "Sec-Secure-Session-Id"];

/// Perform one DBSC POST against `url`.
///
/// - `jwt`: the `Secure-Session-Response` proof JWT value, or `None` for a
///   challenge-less, authorization-less exchange (the header is omitted).
/// - `session_id`: the `Sec-Secure-Session-Id` value (refresh only; absent
///   on registration).
/// - `authorization`: the `Authorization` header value when the registration
///   header carried an `authorization` parameter (registration only).
/// - `webview_id` / `pipeline_id`: attribution for wire-profile resolution
///   and per-page connection identity, inherited from the triggering
///   request.
///
/// BAO PATCH (REQ-BRW-051): `authorization` injection closes the former
/// zero-caller gap — the registration header's `authorization` parameter is
/// the draft's opt-in for the registration POST's `Authorization` header,
/// and the driver emits it fail-closed (an unusable value aborts the
/// exchange rather than silently registering without it).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dbsc_post(
    url: ServoUrl,
    jwt: Option<&str>,
    session_id: Option<&str>,
    authorization: Option<&str>,
    cross_site: bool,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    context: &FetchContext,
) -> Result<DbscExchange, DbscPostError> {
    let mut headers = HeaderMap::new();
    // The registration/refresh POST is an HTTP fetch with credentials
    // included (the server identifies the session by its cookies — WPT's
    // server resolves its per-test state from the `test_id` cookie), so the
    // jar's cookies ride along under the third-party attachment rule.
    attach_request_cookies(&url, &mut headers, &context.state, cross_site);
    // §8.8: the proof is only created when a challenge or an authorization
    // value is non-null — a challenge-less registration sends NO
    // Secure-Session-Response header at all (WPT registration-no-challenge).
    if let Some(jwt) = jwt {
        // The JWT is base64url + dots: always a valid header value; a
        // malformed value would be a driver bug, surfaced as a network-class
        // failure.
        let value = HeaderValue::from_str(jwt).map_err(|_| DbscPostError::Network)?;
        headers.insert("Secure-Session-Response", value);
    }
    if let Some(session_id) = session_id {
        let value = HeaderValue::from_str(session_id).map_err(|_| DbscPostError::Network)?;
        headers.insert("Sec-Secure-Session-Id", value);
    }
    if let Some(authorization) = authorization {
        let value = HeaderValue::from_str(authorization).map_err(|_| DbscPostError::Network)?;
        headers.insert(header::AUTHORIZATION, value);
    }
    headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    // The draft's registration/refresh POSTs are body-less; an explicit
    // Content-Length keeps the request wire-identical to a fetch-issued
    // bodyless POST (http_network_or_cache_fetch step 8.7).
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));

    dbsc_exchange(
        url,
        Method::POST,
        headers,
        webview_id,
        pipeline_id,
        context,
    )
    .await
}

/// Fetch the `.well-known/device-bound-sessions` resource of an origin with
/// credentials omitted (the draft's registration §8.9 step 11 and federated
/// §8.11 checks). The UA-driven leg rides the same bun bridge as the DBSC
/// POSTs, but carries no cookies: `obtain_response_bun` receives a fresh
/// header map, so nothing from the jar can leak into the discovery request.
pub(crate) async fn dbsc_get_well_known(
    url: ServoUrl,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    context: &FetchContext,
) -> Result<DbscExchange, DbscPostError> {
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    dbsc_exchange(
        url,
        Method::GET,
        headers,
        webview_id,
        pipeline_id,
        context,
    )
    .await
}

/// The shared UA-driven exchange leg (used by the POST driver and the
/// well-known GET).
async fn dbsc_exchange(
    url: ServoUrl,
    method: Method,
    mut headers: HeaderMap,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    context: &FetchContext,
) -> Result<DbscExchange, DbscPostError> {
    // Scratch timing context: DBSC traffic must not perturb the triggering
    // request's observable resource timing.
    let scratch_context = FetchContext {
        timing: ResourceFetchTiming::new(ResourceTimingType::Resource).into(),
        ..context.clone()
    };

    let (fetch_terminated_sender, mut fetch_terminated_receiver) =
        tokio::sync::mpsc::unbounded_channel();
    // No request body: send the "streamed without error" confirmation now,
    // mirroring http_network_fetch's bodyless arm.
    let _ = fetch_terminated_sender.send(false);

    let request_id = format!("dbsc-{}", uuid::Uuid::new_v4());
    let (response_stream, _devtools_message) = crate::fetch::bun_bridge::obtain_response_bun(
        &url,
        &method,
        &mut headers,
        None,
        &pipeline_id,
        webview_id,
        Some(&request_id),
        Destination::None,
        false,
        None,
        &scratch_context,
        fetch_terminated_sender,
    )
    .await
    .map_err(|_| DbscPostError::Network)?;

    match fetch_terminated_receiver.recv().await {
        Some(false) => {},
        _ => return Err(DbscPostError::Network),
    }

    let status = response_stream.status().as_u16();
    let response_headers = response_stream.headers().clone();
    let body = response_stream
        .into_body()
        .try_fold(Vec::new(), |mut buffer, chunk| {
            buffer.extend_from_slice(&chunk);
            futures::future::ready(Ok(buffer))
        })
        .await
        .map_err(|_| DbscPostError::Body)?;

    Ok(DbscExchange {
        status,
        headers: response_headers,
        body,
    })
}

/// Drive one registration POST and return the exchange. The proof (when the
/// header carried `challenge` and/or `authorization`) includes the public JWK
/// in its JWT header (§8.8; a refresh never resends the key), and a
/// challenge-less registration sends no proof at all (§8.10 allows the
/// `challenge` parameter to be absent).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn registration_post(
    url: ServoUrl,
    key_pair: &DbscKeyPair,
    algorithm: DbscAlgorithm,
    challenge: Option<&str>,
    authorization: Option<&str>,
    cross_site: bool,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    context: &FetchContext,
) -> Result<DbscExchange, DbscPostError> {
    // §8.8: a proof is created only when the challenge or the authorization
    // value is non-null. A challenge-less registration (the header's
    // `challenge` parameter is optional — WPT registration-no-challenge)
    // sends the POST without `Secure-Session-Response`.
    let jwt = if challenge.is_none() && authorization.is_none() {
        None
    } else {
        Some(
            key_pair
                .sign_proof_jwt(
                    algorithm,
                    challenge,
                    authorization,
                    /* include_jwk = */ true,
                )
                .ok_or(DbscPostError::Network)?,
        )
    };
    dbsc_post(
        url,
        jwt.as_deref(),
        None,
        authorization,
        cross_site,
        webview_id,
        pipeline_id,
        context,
    )
    .await
}

/// Store the Set-Cookie headers of a DBSC response into the cookie jar (the
/// bound cookies from registration/refresh instruction responses). This is
/// the same store path a page-visible response takes (`set_cookie_for_url`).
///
/// `cross_site` applies the third-party storage policy the UA's cookie rules
/// impose: in a cross-site context only `SameSite=None; Secure` cookies may
/// be stored (RFC 6265bis). This is what makes a first-party-only session
/// impossible to register from a third-party context (WPT
/// third-party-registration, first cell) and is the "could not set its
/// cookie" side of the draft's §8.9 step 21 / §8.7 gates. Returns whether
/// any cookie was stored.
pub(crate) fn store_response_cookies(
    url: &ServoUrl,
    headers: &HeaderMap,
    state: &HttpState,
    cross_site: bool,
) -> bool {
    let mut stored = false;
    for cookie in headers.get_all(header::SET_COOKIE) {
        let Ok(cookie_str) = std::str::from_utf8(cookie.as_bytes()) else {
            continue;
        };
        if cross_site && !cookie_storable_cross_site(cookie_str) {
            continue;
        }
        crate::http_loader::set_cookie_for_url(&state.cookie_jar, url, cookie_str);
        stored = true;
    }
    stored
}

/// Whether a Set-Cookie value may be stored in a third-party context:
/// `SameSite=None` plus `Secure` (RFC 6265bis §5.4 third-party cookie
/// blocking; the UA has no separate toggle in this slice).
fn cookie_storable_cross_site(cookie_str: &str) -> bool {
    let Ok(cookie) = cookie::Cookie::parse(cookie_str.to_owned()) else {
        return false;
    };
    cookie.same_site() == Some(cookie::SameSite::None) && cookie.secure().unwrap_or(false)
}

/// The names of the cookies a response's Set-Cookie headers set — the
/// response side of the draft's "could the response set this credential's
/// cookie" checks (§8.9 step 21, §8.7).
pub(crate) fn response_cookie_names(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|cookie| std::str::from_utf8(cookie.as_bytes()).ok())
        .filter_map(|cookie_str| cookie::Cookie::parse(cookie_str.to_owned()).ok())
        .map(|cookie| cookie.name().to_owned())
        .collect()
}

/// Attach the jar's cookies to a UA-driven DBSC request (the registration /
/// refresh POSTs), applying the third-party attachment rule: in a cross-site
/// context only `SameSite=None` cookies are sent. The cookie *document* is
/// built from the jar's native cookies so the attribute can be inspected.
fn attach_request_cookies(
    url: &ServoUrl,
    headers: &mut HeaderMap,
    state: &HttpState,
    cross_site: bool,
) {
    let mut cookie_jar = state.cookie_jar.write();
    cookie_jar.remove_expired_cookies_for_url(url);
    let cookies: Vec<cookie::Cookie<'static>> = cookie_jar
        .cookies_data_for_url(url, net_traits::CookieSource::HTTP)
        .collect();
    let selected: Vec<String> = cookies
        .iter()
        .filter(|cookie| {
            !cross_site || cookie.same_site() == Some(cookie::SameSite::None)
        })
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect();
    if selected.is_empty() {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&selected.join("; ")) {
        headers.insert(header::COOKIE, value);
    }
}

/// Re-attach the Cookie header from the jar after a DBSC refresh rotated
/// the bound cookies. Used by the WebSocket handshake seam, which builds its
/// own header list before the fetch pipeline runs.
pub(crate) fn set_request_cookie_header(url: &ServoUrl, headers: &mut HeaderMap, state: &HttpState) {
    let mut cookie_jar = state.cookie_jar.write();
    cookie_jar.remove_expired_cookies_for_url(url);
    match cookie_jar.cookies_for_url(url, net_traits::CookieSource::HTTP) {
        Some(cookie_list) => {
            if let Ok(value) = HeaderValue::from_str(&cookie_list) {
                headers.insert(header::COOKIE, value);
            }
        },
        None => {
            headers.remove(header::COOKIE);
        },
    }
}

/// Snapshot the cookies the jar would attach to `url` right now, as the
/// jar's native `cookie::Cookie` values (the full attribute set survives —
/// DBSC credential matching compares Domain/Path/Secure/HttpOnly/SameSite/
/// Partitioned). The HTTP source keeps HttpOnly cookies visible, which the
/// bound cookies are by construction.
pub(crate) fn request_cookies_for_url(
    url: &ServoUrl,
    state: &HttpState,
) -> Vec<cookie::Cookie<'static>> {
    let mut cookie_jar = state.cookie_jar.write();
    cookie_jar.remove_expired_cookies_for_url(url);
    cookie_jar
        .cookies_data_for_url(url, net_traits::CookieSource::HTTP)
        .collect()
}

/// Strip DBSC-managed headers from a header map (used when copying
/// instruction-response headers into a synthesized response).
pub(crate) fn strip_dbsc_headers(headers: &mut HeaderMap) {
    for name in DBSC_REQUEST_HEADERS {
        headers.remove(name);
    }
}

/// The `Secure-Session-Skipped` request header name (draft §9.5).
pub(crate) const SKIPPED_HEADER: &str = "secure-session-skipped";

/// One session that could not be refreshed, with the draft §9.5 reason token
/// (`unreachable` / `server_error`; `quota_exceeded` has no emission point in
/// this slice — the store's per-domain session quota is a registration
/// reject, not a refresh skip, and the draft's DoS-quota policy is a UA
/// runtime decision that is not wired).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SkippedRefresh {
    pub(crate) token: &'static str,
    pub(crate) session_identifier: String,
}

/// Build the `Secure-Session-Skipped` header value (an sf-list of sf-tokens
/// each carrying the optional `session_identifier` parameter), or `None`
/// when no session was skipped. Sessions stay in registration order (the
/// store iterates in that order), which is the order WPT debug-header
/// asserts (`"0"` before `"1"`).
pub(crate) fn skipped_header_value(skipped: &[SkippedRefresh]) -> Option<HeaderValue> {
    if skipped.is_empty() {
        return None;
    }
    let items: Vec<String> = skipped
        .iter()
        .map(|item| {
            format!(
                "{};session_identifier=\"{}\"",
                item.token,
                escape_sf_string(&item.session_identifier)
            )
        })
        .collect();
    HeaderValue::from_str(&items.join(", ")).ok()
}

/// Escape an sf-string value (RFC 9651: `\` and `"` are backslash-escaped).
fn escape_sf_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The outcome of the per-request DBSC refresh determination (the hook after
/// the request's cookies were attached).
pub(crate) struct RefreshOutcome {
    /// At least one session was refreshed or terminated — the caller must
    /// re-read the cookie jar and re-attach the request cookies.
    pub(crate) restart_cookies: bool,
    /// Sessions that were in scope and needed a refresh but were not
    /// refreshed; the caller turns these into the `Secure-Session-Skipped`
    /// request header.
    pub(crate) skipped: Vec<SkippedRefresh>,
}

/// Drive the per-request DBSC refresh determination (§8.5): for every
/// session of the request's site that is in URL scope (§8.2), whose refresh
/// is permitted for the request's origin (§8.3), and whose bound credentials
/// are missing or mismatched (§8.6), run the draft's refresh loop; report
/// whether the jar changed underneath the request and which sessions were
/// skipped (for the debug header).
///
/// BAO PATCH (REQ-BRW-051): this is the "计算 cookies 后,若 in-scope +
/// 允许 initiator + 绑定 cookie 缺失/不匹配 → 先 POST refresh(阻塞原请求)
/// → 重算 cookies" seam of the draft's HTTP-network-or-cache fetch
/// integration, driven from `http_fetch` right after `set_request_cookies`.
/// Wave B: the permission test is the request **origin's host** against the
/// instruction's host patterns (§8.3/§8.4), not the fetch initiator type.
pub(crate) async fn refresh_sessions_for_request(
    request_url: &ServoUrl,
    request_origin: &RequestOrigin,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    state: &HttpState,
    context: &FetchContext,
) -> RefreshOutcome {
    let mut outcome = RefreshOutcome {
        restart_cookies: false,
        skipped: Vec::new(),
    };
    if !state.dbsc.has_sessions_for_site(request_url) {
        return outcome;
    }
    // The request's third-party state: the cookie attachment and storage
    // gates (§8.6 step 1.1, §8.9 step 21) are relative to it.
    let cross_site =
        !same_registrable_domain(request_origin.host.as_deref(), request_url.host_str());
    // Backoff is per registrable domain: while a domain cools down from a
    // failed refresh, its sessions are skipped with the failure's token
    // (they were in scope and needed a refresh, so the debug header reports
    // them rather than silently pretending there was nothing to do).
    let backoff_token = state.dbsc.backoff_token(request_url);
    for session_identifier in state.dbsc.sessions_for_url(request_url) {
        // §8.2 URL scope + §8.3 refresh permission, read inside the store
        // visitor (no lock leaves this block).
        let permitted = state
            .dbsc
            .with_session(request_url, &session_identifier, |stored| {
                stored.session.url_in_scope(request_url) &&
                    stored
                        .session
                        .refresh_permitted_for_origin(request_origin)
            })
            .unwrap_or(false);
        if !permitted {
            continue;
        }
        // §8.6 credential gate: at least one attachable credential's cookie
        // is missing or attribute-mismatched.
        let cookies = request_cookies_for_url(request_url, state);
        let needs_refresh = state
            .dbsc
            .with_session(request_url, &session_identifier, |stored| {
                stored.session.needs_refresh(&cookies, cross_site)
            });
        if needs_refresh != Some(true) {
            // `None` = the session vanished (concurrent termination);
            // `Some(false)` = every attachable credential holds. Neither
            // refreshes.
            continue;
        }
        if let Some(token) = backoff_token {
            outcome.skipped.push(SkippedRefresh {
                token,
                session_identifier,
            });
            continue;
        }
        match refresh_session(
            request_url,
            &session_identifier,
            cross_site,
            webview_id,
            pipeline_id,
            state,
            context,
        )
        .await
        {
            RefreshResult::Changed => outcome.restart_cookies = true,
            RefreshResult::Unchanged => {},
            RefreshResult::Skipped(token) => outcome.skipped.push(SkippedRefresh {
                token,
                session_identifier,
            }),
        }
    }
    outcome
}

/// The result of one session's refresh loop.
enum RefreshResult {
    /// The store or the cookie jar changed; the caller must re-read cookies.
    Changed,
    /// Nothing observable changed (all refresh attempts kept the session).
    Unchanged,
    /// The refresh was skipped for the given §9.5 reason token.
    Skipped(&'static str),
}

/// Run the draft's refresh loop for one session: a challenge-less (or
/// cached-challenge) POST, one 403-driven retry with the fresh challenge,
/// termination on terminal 4xx or `continue: false`, backoff plus a
/// `Secure-Session-Skipped` token on network-class/5xx failures. The result
/// tells the caller whether the jar/store changed (re-read cookies) or which
/// §9.5 token to report.
async fn refresh_session(
    request_url: &ServoUrl,
    session_identifier: &str,
    cross_site: bool,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    state: &HttpState,
    context: &FetchContext,
) -> RefreshResult {
    // Snapshot the addressable parts (refresh URL, algorithm) without
    // holding the store lock across the network leg.
    let Some((refresh_url, algorithm)) = state.dbsc.with_session(
        request_url,
        session_identifier,
        |stored| (stored.session.refresh_url.clone(), stored.session.algorithm),
    ) else {
        return RefreshResult::Unchanged;
    };

    // §8.8 step 3: the refresh POST only runs when the originating request
    // URL's origin is same-site with the destination (refresh URL) origin.
    if !same_registrable_domain(request_url.host_str(), refresh_url.host_str()) {
        return RefreshResult::Unchanged;
    }

    // The refresh URL is excluded from its own session's scope
    // (`DbscSession::url_in_scope`), so a refresh can never re-enter this
    // loop through the page fetch path; the loop below is the draft's
    // bounded retry (initial attempt + one 403 challenge retry).
    //
    // Refresh JWT shape (the draft's "refresh does not resend the key"):
    // signed with the session key, no `jwk` header member, no
    // `authorization` claim; the cached challenge rides as `jti` when the
    // session holds one, and a fresh refresh's first attempt runs
    // challenge-less. There is deliberately no standalone `refresh_post`
    // driver — signing must happen *inside* the store visitor (keys never
    // leave the store, the `DbscStore::with_session` contract), which a
    // `&DbscKeyPair`-taking driver cannot express.
    for attempt in 0..2 {
        // Sign inside the store visitor: keys never leave the store, and
        // the cached challenge (possibly updated by the 403 retry) is read
        // at signing time. With NO cached challenge the refresh sends no
        // proof at all (§8.8: a proof is created only when the challenge or
        // authorization is non-null) — sending an empty-claim JWT instead
        // would be a proof the server cannot verify, and WPT's refresh
        // endpoint only answers 403 + a fresh challenge when
        // `Secure-Session-Response` is absent.
        let cached_jwt = state
            .dbsc
            .with_session(request_url, session_identifier, |stored| {
                let cached = stored.session.cached_challenge.as_deref()?;
                stored.key.sign_proof_jwt(algorithm, Some(cached), None, false)
            })
            .flatten();

        let exchange = dbsc_post(
            refresh_url.clone(),
            cached_jwt.as_deref(),
            Some(session_identifier),
            None,
            cross_site,
            webview_id,
            pipeline_id,
            context,
        )
        .await;

        let exchange = match exchange {
            Ok(exchange) => exchange,
            Err(DbscPostError::Network) => {
                // Network-class failure: keep the session, cool the domain
                // down (the endpoint is unreachable right now; hammering it
                // every request would turn one dead endpoint into a
                // per-request tax), and report the §9.5 `unreachable` token.
                state
                    .dbsc
                    .record_refresh_failure(&refresh_url, "unreachable");
                return RefreshResult::Skipped("unreachable");
            },
            Err(DbscPostError::Body) => {
                // The response's body stream failed mid-drain: the server
                // did not deliver a usable response.
                state
                    .dbsc
                    .record_refresh_failure(&refresh_url, "server_error");
                return RefreshResult::Skipped("server_error");
            },
        };

        // Store the response's Set-Cookie headers whatever the status (the
        // exchange is an HTTP fetch with credentials included); a rotated
        // bound cookie on an error response still changes the jar.
        let stored_cookies =
            store_response_cookies(&refresh_url, &exchange.headers, state, cross_site);

        if exchange.status == 403 {
            // §8.8/§8.7: a 403 supplies a fresh challenge for this session
            // (the id parameter is required; an id-less challenge is
            // ignored), which is cached and then used on a single retry. A
            // 403 with no usable challenge keeps the session (403 is
            // excluded from the terminal-4xx set) and stops the loop.
            cache_challenges_from_response(
                &refresh_url,
                &exchange.headers,
                session_identifier,
                cross_site,
                state,
            );
            let has_cached_challenge = state
                .dbsc
                .with_session(&refresh_url, session_identifier, |stored| {
                    stored.session.cached_challenge.is_some()
                })
                .unwrap_or(false);
            if attempt == 0 && has_cached_challenge {
                continue;
            }
            return if stored_cookies {
                RefreshResult::Changed
            } else {
                RefreshResult::Unchanged
            };
        }

        if exchange.status == 407 || exchange.status == 429 {
            // Explicitly non-terminal: keep the session, no backoff, no
            // retry (proxy auth is the user's to answer; 429 defers to the
            // server's own pacing).
            return if stored_cookies {
                RefreshResult::Changed
            } else {
                RefreshResult::Unchanged
            };
        }

        if exchange.status >= 500 {
            // 5xx: keep the session, grow the domain's backoff, report the
            // §9.5 `server_error` token.
            state
                .dbsc
                .record_refresh_failure(&refresh_url, "server_error");
            return RefreshResult::Skipped("server_error");
        }

        if !(200..300).contains(&exchange.status) {
            // Terminal 4xx (the design excludes only 403/407/429 above):
            // the server rejected the session itself — terminate it.
            state.dbsc.remove_session(&refresh_url, session_identifier);
            return RefreshResult::Changed;
        }

        // 2xx: process the response's §8.7 challenge face, then the
        // instructions body. An EMPTY body on refresh is an explicit no-op
        // success (§8.8: the empty-body shortcut applies when a session
        // identifier is present — the session survives and the stored
        // cookies stand; WPT empty-response "An empty response is allowed on
        // refresh").
        cache_challenges_from_response(
            &refresh_url,
            &exchange.headers,
            session_identifier,
            cross_site,
            state,
        );
        if exchange.body.iter().all(|byte| byte.is_ascii_whitespace()) {
            return if stored_cookies {
                RefreshResult::Changed
            } else {
                RefreshResult::Unchanged
            };
        }
        let changed = apply_refresh_instructions(
            &refresh_url,
            session_identifier,
            algorithm,
            &exchange.body,
            state,
        );
        return if changed || stored_cookies {
            RefreshResult::Changed
        } else {
            RefreshResult::Unchanged
        };
    }
    // Unreachable: the loop's arms return on every path (the `continue` is
    // only taken with attempt == 0, so iteration 1 always returns).
    RefreshResult::Unchanged
}

/// Apply a 2xx refresh body to the stored session. Returns true when the
/// store changed (config replaced or session terminated).
fn apply_refresh_instructions(
    refresh_url: &ServoUrl,
    session_identifier: &str,
    algorithm: DbscAlgorithm,
    body: &[u8],
    state: &HttpState,
) -> bool {
    // `continue: false` ends the session; a body that fails instruction
    // validation also ends it — a server that cannot produce a valid
    // instructions document has broken the session contract, and keeping a
    // stale config would re-trigger this loop on every request.
    let Some(RefreshInstructions::Continue(instructions)) = parse_refresh_instructions(body) else {
        state.dbsc.remove_session(refresh_url, session_identifier);
        return true;
    };

    // The session identifier is the store key and must not change (the
    // draft: a refresh response cannot re-key a session); a mismatched
    // identifier fails the update and the session is terminated.
    if instructions.session_identifier != session_identifier {
        state.dbsc.remove_session(refresh_url, session_identifier);
        return true;
    }
    // The cached challenge survives a config replacement unless the response
    // supplied a fresh one (processed by the caller's §8.7 pass): the draft's
    // session field is only written by §8.7, never reset by instructions.
    let cached_challenge = state
        .dbsc
        .with_session(refresh_url, session_identifier, |stored| {
            stored.session.cached_challenge.clone()
        })
        .flatten();
    // §8.9 steps 6-9 applied to the refresh body: resolve and validate the
    // scope origin and refresh URL against this refresh URL as the
    // destination. An invalid document terminates the session (a server that
    // cannot produce valid instructions has broken the session contract).
    let Some((session, _scope_origin)) =
        build_session(&instructions, algorithm, refresh_url, cached_challenge)
    else {
        state.dbsc.remove_session(refresh_url, session_identifier);
        return true;
    };
    // A refresh may not move the session out of its store partition (the
    // store is keyed by registrable domain) — a cross-site refresh URL is
    // rejected fail-closed by terminating the session. `build_session`
    // already enforces same-site against the destination; this guard keeps
    // the store-replace invariant explicit.
    if !urls_share_site(&session.refresh_url, refresh_url) {
        state.dbsc.remove_session(refresh_url, session_identifier);
        return true;
    }
    // The same-registrable-domain check above guarantees `replace_session`
    // (keyed on the new URL) looks up the bucket that holds this entry.
    let replaced = state.dbsc.replace_session(session);
    if replaced {
        state.dbsc.record_refresh_success(refresh_url);
    }
    replaced
}

/// Build a session from parsed instructions in the §8.9 create-session
/// order: resolve the scope origin and the refresh URL, then validate them.
/// Returns the session plus the resolved scope origin (the registration
/// path needs it for the §8.9 step 11 well-known check). `None` = the
/// instructions are invalid and the session must be terminated.
fn build_session(
    instructions: &SessionInstructions,
    algorithm: DbscAlgorithm,
    destination: &ServoUrl,
    cached_challenge: Option<String>,
) -> Option<(DbscSession, ImmutableOrigin)> {
    let scope_origin = resolve_scope_origin(instructions, destination)?;
    let refresh_url = resolve_refresh_url(instructions, destination)?;
    Some((
        DbscSession {
            session_identifier: instructions.session_identifier.clone(),
            algorithm,
            refresh_url,
            scope_origin: scope_origin.clone(),
            scope: instructions.scope.clone(),
            include_site: instructions.include_site,
            scope_specification: instructions.scope_specification.clone(),
            credentials: instructions.credentials.clone(),
            allowed_refresh_initiators: instructions.allowed_refresh_initiators.clone(),
            cached_challenge,
        },
        scope_origin,
    ))
}

/// §8.9 steps 6 and 9 plus step 11's first validation: the scope origin is
/// `scope.origin` (or the destination origin), must be a non-opaque origin
/// same-site with the destination, and — with `include_site` — its host must
/// be the destination's registrable domain (WPT set-scope-origin's second
/// cell registers a session whose scope origin is a subdomain and expects
/// the registration to fail).
fn resolve_scope_origin(
    instructions: &SessionInstructions,
    destination: &ServoUrl,
) -> Option<ImmutableOrigin> {
    let origin = match instructions.scope_origin.as_deref() {
        Some(raw) => {
            let parsed = ServoUrl::parse(raw).ok()?;
            let origin = parsed.origin();
            if !origin.is_tuple() {
                return None;
            }
            origin
        },
        None => destination.origin(),
    };
    let origin_host = origin.host()?.to_string();
    if !same_registrable_domain(Some(origin_host.as_str()), destination.host_str()) {
        return None;
    }
    if instructions.include_site {
        let domain =
            net_traits::pub_domains::reg_suffix(destination.host_str()?).to_lowercase();
        if origin_host.to_lowercase() != domain {
            return None;
        }
    }
    Some(origin)
}

/// §8.9 steps 7-9: the refresh URL is `refresh_url` (or the destination),
/// must be HTTPS (or loopback HTTP), and must be same-site with the
/// destination origin.
fn resolve_refresh_url(
    instructions: &SessionInstructions,
    destination: &ServoUrl,
) -> Option<ServoUrl> {
    let refresh_url = match instructions.refresh_url.as_deref() {
        Some(raw) => destination.join(raw).ok()?,
        None => destination.clone(),
    };
    let scheme = refresh_url.scheme();
    let host = refresh_url.host_str().unwrap_or("");
    let loopback = host == "localhost" ||
        host.ends_with(".localhost") ||
        host == "127.0.0.1" ||
        host == "::1" ||
        host == "[::1]";
    if !(scheme == "https" || (scheme == "http" && loopback)) {
        return None;
    }
    if !same_registrable_domain(refresh_url.host_str(), destination.host_str()) {
        return None;
    }
    Some(refresh_url)
}

/// §8.7: cache an id-scoped `Secure-Session-Challenge` from a response for
/// an existing session. The `id` parameter is required (id-less challenges
/// are ignored), and the challenge is only stored when the response could
/// set one of the session's credential cookies — a third-party response
/// therefore cannot set the challenge of a first-party-only session (WPT
/// third-party-registration, third/fourth cells).
fn cache_challenges_from_response(
    response_url: &ServoUrl,
    headers: &HeaderMap,
    session_identifier: &str,
    cross_site: bool,
    state: &HttpState,
) {
    let Some(can_set) = state
        .dbsc
        .with_session(response_url, session_identifier, |stored| {
            stored.session.can_set_credential_cookie(cross_site)
        })
    else {
        return;
    };
    if !can_set {
        return;
    }
    let challenge = parse_challenge_header(headers)
        .into_iter()
        .find(|challenge| challenge.session_identifier.as_deref() == Some(session_identifier));
    if let Some(challenge) = challenge {
        state.dbsc.set_cached_challenge(
            response_url,
            session_identifier,
            Some(challenge.challenge),
        );
    }
}

/// The registration response header that carries registration directives.
const SECURE_SESSION_REGISTRATION: &str = "secure-session-registration";
/// The response header that clears site data (the DBSC face only).
const CLEAR_SITE_DATA: &str = "clear-site-data";

/// Process the DBSC faces of a fetched response: Clear-Site-Data session
/// removal, early challenges for existing sessions, and registration
/// directives. Driven from `http_network_fetch` right after the response's
/// Set-Cookie headers were stored.
///
/// BAO PATCH (REQ-BRW-051): the vendor tree has **no** Clear-Site-Data
/// processing at all; this slice implements exactly the DBSC session removal
/// that the draft's "clearing cookies/site data must clear sessions and
/// keys" constraint requires — the `cookies` target clears the site's
/// sessions and `storage` clears the origin's. The cookie jar / HTTP caches
/// themselves are separate faces no consumer in this tree implements; this
/// slice never pretends to have cleared them.
pub(crate) async fn process_dbsc_response_headers(
    url: &ServoUrl,
    request_origin: &RequestOrigin,
    headers: &HeaderMap,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    state: &HttpState,
    context: &FetchContext,
) {
    // The response's third-party state, from the initiating request's origin.
    let cross_site =
        !same_registrable_domain(request_origin.host.as_deref(), url.host_str());

    // Clear-Site-Data: a list of quoted targets. The draft ties two targets
    // to sessions:
    //   - `cookies` → "clear device bound sessions for the site matching the
    //     registered domain" (cookie clearing is site-scoped);
    //   - `storage` → "clear all device bound sessions whose scope matches
    //     origin" (DOM-accessible storage clearing is origin-scoped);
    //   - `*` clears everything → the site-scoped face subsumes the origin
    //     face.
    // The cookie jar / caches themselves are separate faces no consumer in
    // this vendor tree implements (registered honestly in the module doc):
    // this slice clears exactly the DBSC sessions, per the draft's
    // obligation.
    let mut clear_site = false;
    let mut clear_origin = false;
    for value in headers.get_all(CLEAR_SITE_DATA) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let Some(members) = net_traits::sfv::parse_list(value) else {
            continue;
        };
        for member in members {
            let net_traits::sfv::SfvListMember::Item(item) = member else {
                continue;
            };
            match item.bare.as_str() {
                Some("cookies") => clear_site = true,
                Some("storage") => clear_origin = true,
                Some("*") => clear_site = true,
                _ => {},
            }
        }
    }
    if clear_site {
        state.dbsc.clear_for_site(url);
    }
    if clear_origin {
        state.dbsc.clear_for_origin(url);
    }

    // §8.7: a `Secure-Session-Challenge` on any response can refresh the
    // cached challenge of an existing session. The `id` parameter is
    // required; the store-side gate (credential cookie settability) is
    // inside the helper.
    for challenge in parse_challenge_header(headers) {
        if let Some(session_identifier) = challenge.session_identifier {
            cache_challenges_from_response(
                url,
                headers,
                &session_identifier,
                cross_site,
                state,
            );
        }
    }

    // Registration directives. A present-but-unparseable header is
    // authoritative absence of registration (fail-closed — no partial
    // session is ever established).
    for value in headers.get_all(SECURE_SESSION_REGISTRATION) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let Some(registrations) = parse_registration_header(value) else {
            continue;
        };
        for registration in registrations {
            register_session(
                registration,
                url,
                request_origin,
                webview_id,
                pipeline_id,
                state,
                context,
            )
            .await;
        }
    }
}

/// Run one registration directive: resolve the challenge, generate the
/// session key, drive the registration POST (with the `Authorization`
/// header when the header asked for one), validate the instruction body,
/// and insert the session (with its key) into the store.
#[allow(clippy::too_many_arguments)]
async fn register_session(
    registration: SessionRegistration,
    response_url: &ServoUrl,
    request_origin: &RequestOrigin,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    state: &HttpState,
    context: &FetchContext,
) {
    // Resolve the endpoint first: the federated path (§8.11 step 18) needs
    // the relying origin, which is the endpoint's origin.
    let Ok(registration_url) = response_url.join(&registration.path) else {
        return;
    };
    // The registration exchange's third-party state, judged from the
    // initiating request's origin against the endpoint (the registration
    // POST and its Set-Cookie land on the endpoint, not on the response URL).
    let cross_site = !same_registrable_domain(
        request_origin.host.as_deref(),
        registration_url.host_str(),
    );

    // Key pair: federated (shared provider key) or freshly generated for the
    // entry's algorithm. No backend for the demanded algorithm
    // (ES256/none in this slice) fails closed — no session is established
    // with a key the UA cannot actually hold.
    let key_pair = match registration.federated() {
        Some(Ok((provider_session_id, provider_url, provider_key))) => {
            match federated_key_pair(
                provider_session_id,
                provider_url,
                provider_key,
                &registration_url,
                webview_id,
                pipeline_id,
                state,
                context,
            )
            .await
            {
                Some(key_pair) => key_pair,
                None => return,
            }
        },
        // A partial provider parameter set is a protocol violation: no
        // session (draft §8.11 step 4.1).
        Some(Err(())) => return,
        None => match DbscKeyPair::generate(registration.algorithm) {
            Some(key_pair) => key_pair,
            None => return,
        },
    };

    // §8.10: the registration challenge comes ONLY from the header's
    // `challenge` parameter. With no challenge and no authorization the
    // registration POST carries no proof at all (WPT registration-no-
    // challenge); a same-response `Secure-Session-Challenge` is cached only
    // after the session exists (§8.9 step 22).
    let Ok(exchange) = registration_post(
        registration_url.clone(),
        &key_pair,
        registration.algorithm,
        registration.challenge.as_deref(),
        registration.authorization.as_deref(),
        cross_site,
        webview_id,
        pipeline_id,
        context,
    )
    .await
    else {
        return;
    };
    // The response's Set-Cookie headers are stored whatever the status: the
    // registration exchange is an HTTP fetch with credentials included, and
    // WPT's failing-registration cells observe their `registrationExtraCookies`
    // on a 403 response.
    store_response_cookies(&registration_url, &exchange.headers, state, cross_site);
    if !(200..300).contains(&exchange.status) {
        // A 403 with a challenge before any instructions is a failed
        // registration (WPT registration-sends-challenge, first cell): the
        // session is never created.
        return;
    }

    // Validate the instructions body before anything is stored (fail-closed:
    // a partial session must never be registered; an empty body fails here —
    // the §8.8 empty-body shortcut does not apply to registration).
    let Some(instructions) = parse_session_instructions(&exchange.body) else {
        return;
    };
    let Some((session, scope_origin)) = build_session(
        &instructions,
        registration.algorithm,
        &registration_url,
        None,
    ) else {
        return;
    };

    // §8.9 step 11: with `include_site`, a registration whose destination
    // host is a subdomain of the destination's registrable domain must be
    // authorized by the scope origin's `.well-known/device-bound-sessions`
    // (`registering_origins` must contain the destination origin). This is
    // the whole of WPT subdomain-registration and the reason a subdomain
    // cannot silently widen a session to its whole site.
    if instructions.include_site {
        let destination_host = registration_url.host_str().unwrap_or("").to_lowercase();
        let destination_domain =
            net_traits::pub_domains::reg_suffix(&destination_host).to_lowercase();
        if destination_domain != destination_host {
            let Some(well_known) = well_known_url(&scope_origin) else {
                return;
            };
            let Ok(exchange) =
                dbsc_get_well_known(well_known, webview_id, pipeline_id, context).await
            else {
                return;
            };
            if exchange.status != 200 {
                return;
            }
            let destination_origin =
                registration_url.origin().ascii_serialization().into_owned();
            if !well_known_registering_origins_include(&exchange.body, &destination_origin) {
                return;
            }
        }
    }

    // §8.9 step 21: the session is stored only when the response could set
    // at least one of its credential cookies. In a third-party context only
    // `SameSite=None` credentials qualify, which is exactly why a
    // first-party-only session cannot be registered from a third-party
    // context (WPT third-party-registration).
    if !response_sets_a_credential(&session, &exchange.headers, cross_site) {
        return;
    }

    // §8.9 step 22 → §8.7: seed the cached challenge from the registration
    // response (needed when the server sends the challenge alongside the
    // instructions — WPT registration-sends-challenge, second cell).
    let mut session = session;
    if session.can_set_credential_cookie(cross_site) {
        session.cached_challenge = parse_challenge_header(&exchange.headers)
            .into_iter()
            .find(|challenge| {
                challenge.session_identifier.as_deref() ==
                    Some(session.session_identifier.as_str())
            })
            .map(|challenge| challenge.challenge);
    }
    state.dbsc.insert_session(session, key_pair);
}

/// Whether the response's Set-Cookie headers cover at least one of the
/// session's credentials *that could be stored in this context* (§8.9 step
/// 21's "if the response could not set its cookie, continue; otherwise …
/// store the session").
fn response_sets_a_credential(
    session: &DbscSession,
    headers: &HeaderMap,
    cross_site: bool,
) -> bool {
    let set_names = response_cookie_names(headers);
    session.credentials.iter().any(|credential| {
        credential.attachable(cross_site) &&
            set_names
                .iter()
                .any(|name| name == &credential.cookie_name)
    })
}

/// Resolve the shared provider key pair for a federated registration
/// (draft §8.11). All checks fail closed: a missing provider session, a
/// mismatched scope origin or thumbprint, or a `.well-known` document that
/// does not authorize the pair leaves the registration unacted on.
///
/// The `device-bound-session-key-sharing` permission (§8.11 step 4.2) only
/// fails the flow when **denied**; this UA has no permission store for the
/// feature, so the request is in the (non-denied) default state. Registered
/// honestly rather than inventing a prompt.
#[allow(clippy::too_many_arguments)]
async fn federated_key_pair(
    provider_session_id: &str,
    provider_url: &str,
    provider_key: &str,
    registration_url: &ServoUrl,
    webview_id: Option<WebViewId>,
    pipeline_id: Option<PipelineId>,
    state: &HttpState,
    context: &FetchContext,
) -> Option<DbscKeyPair> {
    let provider_url = ServoUrl::parse(provider_url).ok()?;
    let provider_origin = provider_url.origin();
    if !provider_origin.is_tuple() {
        return None;
    }
    let provider_origin_serialized = provider_origin.ascii_serialization().into_owned();

    // The provider session is looked up in the session store of the
    // provider's registrable domain, its scope origin must be same-origin
    // with the provider origin, and its key's thumbprint must equal the
    // header's `provider_key` (steps 9-14).
    let (thumbprint, scope_origin_matches) = state.dbsc.with_session(
        &provider_url,
        provider_session_id,
        |stored| {
            (
                stored.key.thumbprint(),
                stored.session.scope_origin_serialized() == provider_origin_serialized,
            )
        },
    )?;
    if !scope_origin_matches || thumbprint != provider_key {
        return None;
    }

    // Provider's well-known (steps 15-17): must not carry `provider_origin`,
    // must carry `relying_origins`, and that list must contain the relying
    // origin (= the registration URL's origin).
    let provider_well_known = well_known_url(&provider_origin)?;
    let exchange = dbsc_get_well_known(
        provider_well_known,
        webview_id,
        pipeline_id,
        context,
    )
    .await
    .ok()?;
    if exchange.status != 200 {
        return None;
    }
    let relying_origin = registration_url.origin().ascii_serialization().into_owned();
    if !well_known_relying_origins_include(&exchange.body, &relying_origin) {
        return None;
    }

    // Relying party's well-known (steps 19-21): must not carry
    // `relying_origins`, must carry `provider_origin` equal to the provider
    // URL's origin.
    let relying_well_known = well_known_url(&registration_url.origin())?;
    let exchange = dbsc_get_well_known(
        relying_well_known,
        webview_id,
        pipeline_id,
        context,
    )
    .await
    .ok()?;
    if exchange.status != 200 {
        return None;
    }
    if !well_known_provider_origin_matches(&exchange.body, &provider_origin_serialized) {
        return None;
    }

    // The provider session's key pair is reused; it never leaves the store
    // in any other form than this clone.
    state
        .dbsc
        .with_session(&provider_url, provider_session_id, |stored| {
            stored.key.clone()
        })
}

/// Parse a well-known body as a JSON dictionary.
fn parse_well_known(body: &[u8]) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    if !value.is_object() {
        return None;
    }
    Some(value)
}

/// Whether a `.well-known/device-bound-sessions` document lists `origin` in
/// its `registering_origins` (the §8.9 step 11 authorization).
fn well_known_registering_origins_include(body: &[u8], origin: &str) -> bool {
    let Some(value) = parse_well_known(body) else {
        return false;
    };
    value
        .get("registering_origins")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|origins| {
            origins
                .iter()
                .any(|candidate| candidate.as_str() == Some(origin))
        })
}

/// Whether a provider's well-known document lists the relying origin in
/// `relying_origins` and does not carry `provider_origin` (§8.11 steps
/// 15-16).
fn well_known_relying_origins_include(body: &[u8], relying_origin: &str) -> bool {
    let Some(value) = parse_well_known(body) else {
        return false;
    };
    if value.get("provider_origin").is_some() {
        return false;
    }
    value
        .get("relying_origins")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|origins| {
            origins
                .iter()
                .any(|candidate| candidate.as_str() == Some(relying_origin))
        })
}

/// Whether a relying party's well-known document carries `provider_origin`
/// equal to the provider URL's origin and does not carry `relying_origins`
/// (§8.11 steps 20-21).
fn well_known_provider_origin_matches(body: &[u8], provider_origin: &str) -> bool {
    let Some(value) = parse_well_known(body) else {
        return false;
    };
    if value.get("relying_origins").is_some() {
        return false;
    }
    value
        .get("provider_origin")
        .and_then(serde_json::Value::as_str) ==
        Some(provider_origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(url: &str) -> ServoUrl {
        ServoUrl::parse(url).unwrap()
    }

    /// The `Secure-Session-Skipped` value is an sf-list of sf-tokens with the
    /// `session_identifier` parameter, one item per skipped session in
    /// registration order (WPT debug-header asserts the exact wire text).
    #[test]
    fn skipped_header_value_matches_the_wpt_wire_text() {
        assert_eq!(skipped_header_value(&[]), None);
        let value = skipped_header_value(&[
            SkippedRefresh {
                token: "server_error",
                session_identifier: "0".to_owned(),
            },
            SkippedRefresh {
                token: "server_error",
                session_identifier: "1".to_owned(),
            },
        ])
        .expect("must build")
        .to_str()
        .unwrap()
        .to_owned();
        assert_eq!(
            value,
            r#"server_error;session_identifier="0", server_error;session_identifier="1""#
        );
        // A quote in a server-supplied identifier cannot break the field.
        let escaped = skipped_header_value(&[SkippedRefresh {
            token: "unreachable",
            session_identifier: "a\"b".to_owned(),
        }])
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
        assert_eq!(escaped, r#"unreachable;session_identifier="a\"b""#);
    }

    /// §8.9 step 11: the scope origin must be same-site with the
    /// destination, and with `include_site` its host must be the
    /// destination's registrable domain (WPT set-scope-origin).
    #[test]
    fn scope_origin_validation() {
        let destination = origin("https://web-platform.test/device-bound-session-credentials/x.py");
        let mut instructions = SessionInstructions {
            session_identifier: "0".to_owned(),
            refresh_url: Some("refresh.py".to_owned()),
            scope_origin: None,
            scope: crate::dbsc::model::SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![],
            credentials: vec![],
            allowed_refresh_initiators: vec![],
        };
        // Absent → destination origin.
        assert_eq!(
            resolve_scope_origin(&instructions, &destination),
            Some(destination.origin())
        );
        // `""` is parsed as absent by `parse_session_instructions`; a
        // subdomain scope origin fails the include_site host == domain rule.
        instructions.scope_origin = Some("https://www.web-platform.test".to_owned());
        assert_eq!(resolve_scope_origin(&instructions, &destination), None);
        // A cross-site scope origin fails the same-site rule.
        instructions.scope_origin = Some("https://example.com".to_owned());
        assert_eq!(resolve_scope_origin(&instructions, &destination), None);
        // Origin-scoped sessions admit a same-site subdomain scope origin.
        instructions.include_site = false;
        instructions.scope = crate::dbsc::model::SessionScope::Origin;
        instructions.scope_origin = Some("https://www.web-platform.test".to_owned());
        assert!(resolve_scope_origin(&instructions, &destination).is_some());
        // Absolute validity: an opaque/invalid origin string fails.
        instructions.scope_origin = Some("not a url".to_owned());
        assert_eq!(resolve_scope_origin(&instructions, &destination), None);
    }

    /// §8.9 steps 7-9: default/explicit refresh URL resolution, HTTPS or
    /// loopback rule, and same-site rule (WPT resolving-urls).
    #[test]
    fn refresh_url_resolution_and_validation() {
        let destination =
            origin("https://web-platform.test/device-bound-session-credentials/x.py");
        let mut instructions = SessionInstructions {
            session_identifier: "0".to_owned(),
            refresh_url: None,
            scope_origin: None,
            scope: crate::dbsc::model::SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![],
            credentials: vec![],
            allowed_refresh_initiators: vec![],
        };
        // Absent → the destination itself.
        assert_eq!(
            resolve_refresh_url(&instructions, &destination),
            Some(destination.clone())
        );
        // Relative with and without a leading slash, and absolute, all
        // resolve; query parameters survive (WPT requests-have-query-params).
        instructions.refresh_url = Some("refresh_session.py?refreshQueryParam=456".to_owned());
        assert_eq!(
            resolve_refresh_url(&instructions, &destination).map(|url| url.as_str().to_owned()),
            Some(
                "https://web-platform.test/device-bound-session-credentials/refresh_session.py?refreshQueryParam=456"
                    .to_owned()
            )
        );
        instructions.refresh_url = Some("/r.py".to_owned());
        assert_eq!(
            resolve_refresh_url(&instructions, &destination).map(|url| url.as_str().to_owned()),
            Some("https://web-platform.test/r.py".to_owned())
        );
        instructions.refresh_url = Some("https://web-platform.test/abs.py".to_owned());
        assert!(resolve_refresh_url(&instructions, &destination).is_some());
        // Cross-site refresh URL is rejected.
        instructions.refresh_url = Some("https://other.test/r.py".to_owned());
        assert_eq!(resolve_refresh_url(&instructions, &destination), None);
        // Insecure refresh URL is rejected (loopback excepted).
        instructions.refresh_url = Some("http://web-platform.test/r.py".to_owned());
        assert_eq!(resolve_refresh_url(&instructions, &destination), None);
        instructions.refresh_url = Some("http://localhost/r.py".to_owned());
        assert!(resolve_refresh_url(&instructions, &destination).is_some());
    }

    /// The `.well-known/device-bound-sessions` member checks for the two
    /// registration authorization faces.
    #[test]
    fn well_known_member_checks() {
        let registering = br#"{"registering_origins":["https://www.web-platform.test:8000"]}"#;
        assert!(well_known_registering_origins_include(
            registering,
            "https://www.web-platform.test:8000"
        ));
        assert!(!well_known_registering_origins_include(
            registering,
            "https://www1.web-platform.test:8000"
        ));
        // `null`-ish / non-object bodies fail closed.
        assert!(!well_known_registering_origins_include(b"null", "*"));

        let provider = br#"{"registering_origins":["https://www.web-platform.test:8000"],"relying_origins":["https://www.web-platform.test:8000"]}"#;
        assert!(well_known_relying_origins_include(
            provider,
            "https://www.web-platform.test:8000"
        ));
        assert!(!well_known_relying_origins_include(
            provider,
            "https://www1.web-platform.test:8000"
        ));
        // A provider file must not carry `provider_origin`.
        assert!(!well_known_relying_origins_include(
            br#"{"provider_origin":"https://x:8000","relying_origins":["https://www.web-platform.test:8000"]}"#,
            "https://www.web-platform.test:8000"
        ));

        assert!(well_known_provider_origin_matches(
            br#"{"provider_origin":"https://web-platform.test:8000"}"#,
            "https://web-platform.test:8000"
        ));
        assert!(!well_known_provider_origin_matches(
            br#"{"provider_origin":"https://other.test:8000"}"#,
            "https://web-platform.test:8000"
        ));
        // A relying file must not carry `relying_origins`.
        assert!(!well_known_provider_origin_matches(
            br#"{"provider_origin":"https://web-platform.test:8000","relying_origins":["https://www.web-platform.test:8000"]}"#,
            "https://web-platform.test:8000"
        ));
    }

    /// §8.9 step 21: the response must actually set one of the session's
    /// credential cookies, and only cookies storable in the context count.
    #[test]
    fn response_covers_a_credential_only_when_storable() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SET_COOKIE,
            HeaderValue::from_static("auth_cookie=1; Path=/"),
        );
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static("none_cookie=2; Path=/; SameSite=None; Secure"),
        );
        let session = DbscSession {
            session_identifier: "0".to_owned(),
            algorithm: DbscAlgorithm::Rs256,
            refresh_url: origin("https://web-platform.test/r.py"),
            scope_origin: origin("https://web-platform.test").origin(),
            scope: crate::dbsc::model::SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![],
            credentials: vec![crate::dbsc::model::SessionCredential {
                cookie_name: "auth_cookie".to_owned(),
                domain: String::new(),
                path: String::new(),
                secure: false,
                http_only: false,
                same_site: crate::dbsc::model::SameSiteSpec::Lax,
                partitioned: false,
            }],
            allowed_refresh_initiators: vec![],
            cached_challenge: None,
        };
        // First-party: the Lax credential's cookie was set.
        assert!(response_sets_a_credential(&session, &headers, false));
        // Third-party: a Lax cookie could not be stored there.
        assert!(!response_sets_a_credential(&session, &headers, true));
        let mut same_site_none = session.clone();
        same_site_none.credentials[0].cookie_name = "none_cookie".to_owned();
        same_site_none.credentials[0].same_site = crate::dbsc::model::SameSiteSpec::None;
        same_site_none.credentials[0].secure = true;
        assert!(response_sets_a_credential(&same_site_none, &headers, true));
    }

    #[test]
    fn strip_dbsc_headers_removes_only_the_driver_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("Secure-Session-Response", HeaderValue::from_static("jw"));
        headers.insert("Sec-Secure-Session-Id", HeaderValue::from_static("0"));
        headers.insert("Set-Cookie", HeaderValue::from_static("auth_cookie=x"));
        strip_dbsc_headers(&mut headers);
        assert!(headers.get("Secure-Session-Response").is_none());
        assert!(headers.get("Sec-Secure-Session-Id").is_none());
        assert!(headers.get("Set-Cookie").is_some());
    }
}