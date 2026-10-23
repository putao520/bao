/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DBSC session model — session state, session instructions JSON, scope
//! specification matching, and credential (cookie) matching.
//!
//! BAO PATCH (REQ-BRW-051): implements the data model of the W3C Device
//! Bound Session Credentials draft (2026-09-08 ED), section "Session store":
//! a session holds its identifier, refresh URL, cached challenge, scope,
//! credentials, and refresh-initiator allow list. Scope matching follows the
//! draft's "match a URL against a DBSC session's scope" algorithm
//! (include_site / origin / scope_specification, with the refresh URL always
//! excluded). Credential matching follows "match a DBSC credential against a
//! cookie": name plus attribute comparison where `Expires`/`Max-Age` do not
//! participate (they are lifetime, not identity, attributes) and `Partitioned`
//! does.

use cookie::SameSite;
use http::HeaderMap;
use serde_json::Value as JsonValue;
use servo_url::{ImmutableOrigin, ServoUrl};

use net_traits::sfv::{SfvListMember, parse_list, string_param_in};

/// DBSC proof-of-possession algorithm. The W3C ED defines `ES256`, `RS256`,
/// and (for testing) `none`. WPT's server side (`jwt_helper.py`) only ever
/// verifies RS256; `none` is accepted for the draft's test-only path and
/// `ES256` is parsed but fails key generation fail-closed (no EC backend is
/// wired in this slice — see `keys.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DbscAlgorithm {
    Es256,
    Rs256,
    None,
}

impl DbscAlgorithm {
    fn from_token(token: &str) -> Option<DbscAlgorithm> {
        match token {
            "ES256" => Some(DbscAlgorithm::Es256),
            "RS256" => Some(DbscAlgorithm::Rs256),
            "none" => Some(DbscAlgorithm::None),
            _ => None,
        }
    }

    /// The `alg` value for the DBSC proof JWT header.
    pub(crate) fn jwt_alg(self) -> &'static str {
        match self {
            DbscAlgorithm::Es256 => "ES256",
            DbscAlgorithm::Rs256 => "RS256",
            DbscAlgorithm::None => "none",
        }
    }
}

/// A parsed `Secure-Session-Registration` inner list — one registration
/// challenge received on a response.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionRegistration {
    pub(crate) algorithm: DbscAlgorithm,
    pub(crate) challenge: Option<String>,
    pub(crate) path: String,
    pub(crate) authorization: Option<String>,
    /// Federated (key-sharing) registration parameters — draft §8.11. All
    /// three are present together or none is; the create-session step fails
    /// closed when only some are present (`federated.is_some()` plus a
    /// missing member is caught at parse time below).
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_url: Option<String>,
    pub(crate) provider_key: Option<String>,
}

impl SessionRegistration {
    /// The federated registration parameters, if this entry is a federated
    /// registration. `Some(Err(()))` means a partial set (a protocol
    /// violation: the draft requires all three or none) — the caller must
    /// treat it as a failed registration.
    pub(crate) fn federated(
        &self,
    ) -> Option<Result<(&str, &str, &str), ()>> {
        let present = [
            self.provider_session_id.is_some(),
            self.provider_url.is_some(),
            self.provider_key.is_some(),
        ];
        if present.iter().all(|p| !p) {
            return None;
        }
        match (
            self.provider_session_id.as_deref(),
            self.provider_url.as_deref(),
            self.provider_key.as_deref(),
        ) {
            (Some(session_id), Some(url), Some(key)) => Some(Ok((session_id, url, key))),
            _ => Some(Err(())),
        }
    }
}

/// A challenge received via `Secure-Session-Challenge`, bound to a session
/// identifier (the `id` parameter, when given).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionChallenge {
    pub(crate) challenge: String,
    pub(crate) session_identifier: Option<String>,
}

/// One registered DBSC credential: a cookie name plus the attribute subset
/// that participates in matching (see `SessionCredential::matches_cookie`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionCredential {
    pub(crate) cookie_name: String,
    pub(crate) domain: String,
    pub(crate) path: String,
    pub(crate) secure: bool,
    pub(crate) http_only: bool,
    pub(crate) same_site: SameSiteSpec,
    pub(crate) partitioned: bool,
}

/// The SameSite mode of a credential, compared against stored cookies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SameSiteSpec {
    None,
    Lax,
    Strict,
}

impl SameSiteSpec {
    fn from_cookie(same_site: Option<SameSite>) -> SameSiteSpec {
        match same_site {
            Some(SameSite::Strict) => SameSiteSpec::Strict,
            Some(SameSite::Lax) | None => SameSiteSpec::Lax,
            Some(SameSite::None) => SameSiteSpec::None,
        }
    }
}

/// The scope of a session: which URLs trigger refresh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SessionScope {
    /// `include_site: false` — only same-origin URLs.
    Origin,
    /// `include_site: true` — URLs of the same site (registrable domain).
    IncludeSite,
}

/// One entry of the session's `scope_specification` list (applied last to
/// first — see `DbscSession::url_in_scope`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopeSpecificationItem {
    pub(crate) include: bool,
    pub(crate) domain: String,
    pub(crate) path: String,
}

/// A registered DBSC session, as stored in the session store.
#[derive(Clone, Debug)]
pub(crate) struct DbscSession {
    pub(crate) session_identifier: String,
    /// The proof-of-possession algorithm the session was registered with
    /// (carried by the registration header's algorithm item; the refresh
    /// instructions do not repeat it, so the session persists it for the
    /// refresh JWT's `alg` header member).
    pub(crate) algorithm: DbscAlgorithm,
    pub(crate) refresh_url: ServoUrl,
    /// The scope's origin (draft §8.9 step 6: `scope.origin`, or the
    /// destination origin when absent). Both the URL-scope test (§8.2) and
    /// the refresh-permission test (§8.3) are relative to this origin — the
    /// pre-Wave-B model wrongly used the refresh URL's origin, which differs
    /// whenever the instructions carry a `scope.origin` (WPT
    /// set-scope-origin / subdomain-registration).
    pub(crate) scope_origin: ImmutableOrigin,
    pub(crate) scope: SessionScope,
    /// True when the scope came from `include_site: true`. Always equal to
    /// `matches!(scope, SessionScope::IncludeSite)`; kept explicit because
    /// the draft's session store has both fields.
    pub(crate) include_site: bool,
    pub(crate) scope_specification: Vec<ScopeSpecificationItem>,
    pub(crate) credentials: Vec<SessionCredential>,
    /// Host patterns a cross-site initiator must match to refresh (§8.3
    /// step 3). Empty = only same-site/same-origin requests may refresh.
    pub(crate) allowed_refresh_initiators: Vec<String>,
    /// The most recent challenge for this session, used to answer a 403
    /// refresh retry (the draft's "use the cached challenge" step).
    pub(crate) cached_challenge: Option<String>,
}

/// The parsed session instructions returned by a registration or refresh
/// response body (`application/json`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionInstructions {
    pub(crate) session_identifier: String,
    /// Absent means "the destination URL itself" (draft §8.9 step 7).
    pub(crate) refresh_url: Option<String>,
    /// Raw `scope.origin` string; `None` when absent or empty. An empty
    /// string is treated as absent: the WPT server always emits
    /// `"origin": ""` when no scope origin is configured, and the WPT
    /// acceptance cells (create-session and friends) register successfully
    /// against that body — so the empty string is the "not provided" form in
    /// practice.
    pub(crate) scope_origin: Option<String>,
    pub(crate) scope: SessionScope,
    pub(crate) include_site: bool,
    pub(crate) scope_specification: Vec<ScopeSpecificationItem>,
    pub(crate) credentials: Vec<SessionCredential>,
    pub(crate) allowed_refresh_initiators: Vec<String>,
}

/// Outcome of a refresh-response body check: either fresh instructions that
/// replace the session config, or a terminal `continue: false`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RefreshInstructions {
    Continue(SessionInstructions),
    EndSession,
}

/// Parse the session instructions JSON per the draft's "session instructions"
/// definition. All required fields must be present with the right types; any
/// absence or type mismatch fails the whole parse (fail-closed — a partial
/// session must never be registered).
pub(crate) fn parse_session_instructions(body: &[u8]) -> Option<SessionInstructions> {
    let value: JsonValue = serde_json::from_slice(body).ok()?;

    let session_identifier = value
        .get("session_identifier")
        .and_then(JsonValue::as_str)?
        .to_owned();
    // `refresh_url` is optional; its absence means the destination URL
    // (§8.9 step 7). An empty string is the "not provided" form (same
    // rationale as `scope.origin` below). Resolving against the destination
    // happens in the create-session step, which owns the base URL.
    let refresh_url = value
        .get("refresh_url")
        .and_then(JsonValue::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);

    let scope_value = value.get("scope")?;
    let include_site = scope_value
        .get("include_site")
        .and_then(JsonValue::as_bool)?;
    let scope = if include_site {
        SessionScope::IncludeSite
    } else {
        SessionScope::Origin
    };
    let scope_origin = scope_value
        .get("origin")
        .and_then(JsonValue::as_str)
        .filter(|origin| !origin.is_empty())
        .map(str::to_owned);
    let mut scope_specification = Vec::new();
    if let Some(specification) = scope_value
        .get("scope_specification")
        .and_then(JsonValue::as_array)
    {
        for item in specification {
            let include = match item.get("type").and_then(JsonValue::as_str)? {
                "include" => true,
                "exclude" => false,
                _ => return None,
            };
            // `domain` defaults to `*` and `path` to `/` (§7.3 scope rule
            // members); both are optional in the JSON.
            let domain = item
                .get("domain")
                .and_then(JsonValue::as_str)
                .unwrap_or("*")
                .to_owned();
            let path = item
                .get("path")
                .and_then(JsonValue::as_str)
                .unwrap_or("/")
                .to_owned();
            scope_specification.push(ScopeSpecificationItem {
                include,
                domain,
                path,
            });
        }
    }

    let mut credentials = Vec::new();
    for credential in value.get("credentials")?.as_array()? {
        let credential_type = credential.get("type").and_then(JsonValue::as_str)?;
        if credential_type != "cookie" {
            // The draft currently defines only the "cookie" type; unknown
            // credential types fail the parse so the session is not
            // registered with credentials the UA cannot enforce.
            return None;
        }
        let cookie_name = credential
            .get("name")
            .and_then(JsonValue::as_str)?
            .to_owned();
        if cookie_name.is_empty() {
            // §8.9 step 17: the credential name is required and nonempty.
            return None;
        }
        // `attributes` is optional and defaults to the empty string.
        let attributes = credential
            .get("attributes")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let parsed = parse_credential_attributes(attributes);
        if parsed.partitioned {
            // §8.9 step 9: no JSON credential may carry `Partitioned` —
            // terminate the session rather than register a credential the
            // UA's cookie matching cannot keep stable across partitions.
            return None;
        }
        credentials.push(SessionCredential {
            cookie_name,
            domain: parsed.domain,
            path: parsed.path,
            secure: parsed.secure,
            http_only: parsed.http_only,
            same_site: parsed.same_site,
            partitioned: parsed.partitioned,
        });
    }

    let mut allowed_refresh_initiators = Vec::new();
    if let Some(initiators) = value
        .get("allowed_refresh_initiators")
        .and_then(JsonValue::as_array)
    {
        for initiator in initiators {
            allowed_refresh_initiators.push(initiator.as_str()?.to_owned());
        }
    }
    // The member's absence defaults to the EMPTY list (draft §7.3: "optional;
    // defaults to an empty list") — absent means no cross-site initiator may
    // refresh, not "everyone may". The Wave-A model defaulted to `["*"]`,
    // which turned every cross-site request into a refresh trigger (WPT
    // allowed-refresh-initiators, third cell).

    Some(SessionInstructions {
        session_identifier,
        refresh_url,
        scope_origin,
        scope,
        include_site,
        scope_specification,
        credentials,
        allowed_refresh_initiators,
    })
}

/// Parse a refresh response body: `continue: false` ends the session;
/// otherwise the body is full session instructions (the `continue` member
/// defaults to true per the draft).
pub(crate) fn parse_refresh_instructions(body: &[u8]) -> Option<RefreshInstructions> {
    let value: JsonValue = serde_json::from_slice(body).ok()?;
    if value.get("continue").and_then(JsonValue::as_bool) == Some(false) {
        // A session_identifier is required alongside continue:false.
        value.get("session_identifier").and_then(JsonValue::as_str)?;
        return Some(RefreshInstructions::EndSession);
    }
    Some(RefreshInstructions::Continue(parse_session_instructions(body)?))
}

struct ParsedCookieAttributes {
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: SameSiteSpec,
    partitioned: bool,
}

/// Parse the `attributes` string of one credential (the attribute part of a
/// Set-Cookie-style string, e.g. `Domain=example.com; Path=/; Secure`).
/// `Expires`/`Max-Age` are lifetime attributes and are skipped: they do not
/// participate in credential matching (WPT credentials-matching asserts this
/// explicitly). A missing SameSite defaults to Lax, matching the stored
/// cookie's parsed value for an attribute-less cookie.
fn parse_credential_attributes(attributes: &str) -> ParsedCookieAttributes {
    let mut parsed = ParsedCookieAttributes {
        domain: String::new(),
        path: String::new(),
        secure: false,
        http_only: false,
        same_site: SameSiteSpec::Lax,
        partitioned: false,
    };
    for attribute in attributes.split(';') {
        let attribute = attribute.trim();
        if attribute.is_empty() {
            continue;
        }
        let (name, value) = match attribute.split_once('=') {
            Some((name, value)) => (name.trim(), value.trim()),
            None => (attribute, ""),
        };
        if name.eq_ignore_ascii_case("domain") {
            parsed.domain = value.trim_start_matches('.').to_ascii_lowercase();
        } else if name.eq_ignore_ascii_case("path") {
            parsed.path = value.to_owned();
        } else if name.eq_ignore_ascii_case("secure") {
            parsed.secure = true;
        } else if name.eq_ignore_ascii_case("httponly") {
            parsed.http_only = true;
        } else if name.eq_ignore_ascii_case("samesite") {
            parsed.same_site = match value.to_ascii_lowercase().as_str() {
                "none" => SameSiteSpec::None,
                "strict" => SameSiteSpec::Strict,
                _ => SameSiteSpec::Lax,
            };
        } else if name.eq_ignore_ascii_case("partitioned") {
            parsed.partitioned = true;
        }
        // Expires/Max-Age and unknown attributes are ignored (lifetime and
        // future attributes do not affect matching).
    }
    parsed
}

impl SessionCredential {
    /// Whether the credential's cookie would take part in a request with the
    /// given third-party state: in a cross-site context only `SameSite=None`
    /// cookies are attached (the RFC 6265bis default policy; the UA has no
    /// separate third-party cookie toggle in this slice). Used by the §8.6
    /// credential gate and the §8.7 challenge gate.
    pub(crate) fn attachable(&self, cross_site: bool) -> bool {
        !cross_site || self.same_site == SameSiteSpec::None
    }

    /// Match this credential against a stored cookie, per the draft's
    /// credential-matching algorithm: cookie name must equal the credential
    /// name, and the attribute subset that defines cookie identity
    /// (Domain, Path, Secure, HttpOnly, SameSite, Partitioned) must match.
    /// `Expires`/`Max-Age` never participate.
    ///
    /// BAO PATCH (REQ-BRW-051): the match target is the jar's native
    /// `cookie::Cookie` — the store snapshot (`CookieStorage::
    /// cookies_data_for_url` with the HTTP source) hands these out directly,
    /// preserving every attribute. Rewrapping through
    /// `ServoCookie::from_cookie_string("name=value", ..)` (the previous
    /// draft of this driver) drops Domain/Path/Secure/HttpOnly/SameSite/
    /// Partitioned and breaks matching for any credential that names an
    /// attribute; the NonHTTP-source `query_cookies` path additionally hides
    /// HttpOnly cookies — DBSC bound cookies are typically HttpOnly.
    pub(crate) fn matches_cookie(&self, cookie: &cookie::Cookie<'static>) -> bool {
        if cookie.name() != self.cookie_name {
            return false;
        }
        // Domain: the stored cookie's domain must equal the credential domain
        // when the credential specifies one. (A credential without a Domain
        // attribute covers both host-only and domain cookies, matching how
        // the cookie jar reported the bound cookie.)
        if !self.domain.is_empty() {
            let cookie_domain = cookie
                .domain()
                .unwrap_or("")
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if cookie_domain != self.domain {
                return false;
            }
        }
        // Path must match exactly when specified.
        if !self.path.is_empty() && cookie.path().unwrap_or("") != self.path {
            return false;
        }
        if cookie.secure().unwrap_or(false) != self.secure {
            return false;
        }
        if cookie.http_only().unwrap_or(false) != self.http_only {
            return false;
        }
        if SameSiteSpec::from_cookie(cookie.same_site()) != self.same_site {
            return false;
        }
        if cookie.partitioned().unwrap_or(false) != self.partitioned {
            return false;
        }
        true
    }
}

/// A request's origin in the form the DBSC request-side algorithms need
/// (§8.3 refresh permission, §8.6 credential check). Built from the net
/// crate's `ImmutableOrigin`; `serialized` is the ASCII serialization, so
/// same-origin comparisons never depend on the origin's internal repr.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestOrigin {
    pub(crate) serialized: String,
    pub(crate) host: Option<String>,
    pub(crate) opaque: bool,
}

impl Default for RequestOrigin {
    /// No usable origin (e.g. an unresolved `Origin::Client`): opaque, so no
    /// session's refresh is permitted for it.
    fn default() -> Self {
        RequestOrigin {
            serialized: String::new(),
            host: None,
            opaque: true,
        }
    }
}

impl RequestOrigin {
    pub(crate) fn from_origin(origin: &ImmutableOrigin) -> RequestOrigin {
        RequestOrigin {
            serialized: origin.ascii_serialization().into_owned(),
            host: origin.host().map(|host| host.to_string()),
            opaque: !origin.is_tuple(),
        }
    }
}

/// The `.well-known/device-bound-sessions` URL of an exact origin (scheme +
/// host + port), as the registration (§8.9 step 11) and federated (§8.11)
/// checks fetch it. The URL parser normalizes the default port away, so the
/// explicit `:{port}` here never leaks into the request's Host header.
pub(crate) fn well_known_url(origin: &ImmutableOrigin) -> Option<ServoUrl> {
    let ImmutableOrigin::Tuple(scheme, host, port) = origin else {
        return None;
    };
    ServoUrl::parse(&format!(
        "{scheme}://{host}:{port}/.well-known/device-bound-sessions"
    ))
    .ok()
}

impl DbscSession {
    /// The host of the session's scope origin.
    pub(crate) fn scope_origin_host(&self) -> Option<String> {
        self.scope_origin.host().map(|host| host.to_string())
    }

    /// The ASCII serialization of the session's scope origin.
    pub(crate) fn scope_origin_serialized(&self) -> String {
        self.scope_origin.ascii_serialization().into_owned()
    }

    /// The §8.6 credential gate: does a request from this origin need a
    /// refresh? True when at least one credential's cookie *would be
    /// attached* to the request under cookie rules but is missing or
    /// attribute-mismatched. A credential whose cookie could not be attached
    /// at all (third-party context and no `SameSite=None`) does not gate the
    /// refresh — refreshing could not make it appear on this request.
    pub(crate) fn needs_refresh(
        &self,
        attached: &[cookie::Cookie<'static>],
        cross_site: bool,
    ) -> bool {
        self.credentials.iter().any(|credential| {
            if !credential.attachable(cross_site) {
                return false;
            }
            !attached
                .iter()
                .any(|cookie| credential.matches_cookie(cookie))
        })
    }

    /// The §8.7 gate for caching a response's `Secure-Session-Challenge`:
    /// the challenge is only stored when the response could set at least one
    /// of the session's credential cookies (this is what makes a third-party
    /// response unable to set the challenge of a first-party-only session).
    pub(crate) fn can_set_credential_cookie(&self, cross_site: bool) -> bool {
        self.credentials
            .iter()
            .any(|credential| credential.attachable(cross_site))
    }

    /// Whether a request from `origin` may refresh this session (§8.3): a
    /// same-site (include_site) or same-origin request is always allowed;
    /// otherwise the request origin's host must match an
    /// `allowed_refresh_initiators` host pattern (§8.4).
    pub(crate) fn refresh_permitted_for_origin(&self, origin: &RequestOrigin) -> bool {
        if origin.opaque {
            return false;
        }
        if self.include_site {
            if same_registrable_domain(origin.host.as_deref(), self.scope_origin_host().as_deref())
            {
                return true;
            }
        } else if self.scope_origin_serialized() == origin.serialized {
            return true;
        }
        match origin.host.as_deref() {
            Some(host) => self
                .allowed_refresh_initiators
                .iter()
                .any(|pattern| host_matches(pattern, host)),
            None => false,
        }
    }

    /// Whether a URL is in this session's scope, per §8.2: the refresh URL
    /// itself is always out of scope; then same-origin/same-site against the
    /// **scope origin** applies; then the scope_specification is applied from
    /// last to first (first match wins, default include).
    pub(crate) fn url_in_scope(&self, url: &ServoUrl) -> bool {
        // The refresh URL is always excluded — a refresh request must never
        // trigger a nested refresh of the same session.
        if url.as_str() == self.refresh_url.as_str() {
            return false;
        }

        let in_base_scope = match self.scope {
            SessionScope::Origin => url.origin() == self.scope_origin,
            SessionScope::IncludeSite => {
                same_registrable_domain(url.host_str(), self.scope_origin_host().as_deref())
            },
        };
        if !in_base_scope {
            return false;
        }

        for item in self.scope_specification.iter().rev() {
            if host_matches(item.domain.as_str(), url.host_str().unwrap_or("")) &&
                path_prefix_match(item.path.as_str(), url.path())
            {
                return item.include;
            }
        }
        true
    }
}

/// Registrable-domain equality (the include_site scope test). Uses the same
/// public-suffix table as the cookie store via `net_traits::pub_domains`.
pub(crate) fn same_registrable_domain(host_a: Option<&str>, host_b: Option<&str>) -> bool {
    let (Some(host_a), Some(host_b)) = (host_a, host_b) else {
        return false;
    };
    let suffix_a = net_traits::pub_domains::reg_suffix(host_a).to_lowercase();
    let suffix_b = net_traits::pub_domains::reg_suffix(host_b).to_lowercase();
    suffix_a == suffix_b
}

/// Host-pattern match for scope_specification entries and
/// `allowed_refresh_initiators` entries (draft §8.4): `*` matches anything,
/// `*.suffix` matches any subdomain of suffix (but not suffix itself),
/// otherwise exact (case insensitive) equality.
pub(crate) fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        let suffix = suffix.to_ascii_lowercase();
        return host.ends_with(&suffix) &&
            host.len() > suffix.len() &&
            host.as_bytes()[host.len() - suffix.len() - 1] == b'.';
    }
    pattern.to_ascii_lowercase() == host
}

/// Path-prefix match: `path` is a prefix of the request path at a `/`
/// boundary (or the paths are equal). The empty pattern matches everything.
fn path_prefix_match(pattern: &str, path: &str) -> bool {
    if pattern.is_empty() || pattern == "/" {
        return true;
    }
    if !path.starts_with(pattern) {
        return false;
    }
    path.len() == pattern.len() ||
        pattern.ends_with('/') ||
        path.as_bytes().get(pattern.len()) == Some(&b'/')
}

/// Extract registrations from a `Secure-Session-Registration` header value.
/// Returns `None` when the header is present but unparseable (caller must
/// then not register anything — the header is authoritative, not optional).
pub(crate) fn parse_registration_header(value: &str) -> Option<Vec<SessionRegistration>> {
    let members = parse_list(value)?;
    let mut registrations = Vec::new();
    for member in members {
        // BAO PATCH (REQ-BRW-051): RFC 9651 binds the parameters after an
        // inner list's `)` to the inner list — DBSC's `path`/`challenge`/
        // `authorization` members are read from `params`, not from the
        // algorithm item's parameters (reading them from the item yields
        // `None` for every legal header; the sfv.rs tests pin the shape).
        let SfvListMember::InnerList { items, params } = member else {
            // A bare item at top level is not a valid registration.
            return None;
        };
        let mut algorithm = None;
        for item in &items {
            if let Some(token) = item.token() {
                algorithm = Some(DbscAlgorithm::from_token(token)?);
            }
        }
        let challenge = string_param_in(&params, "challenge").map(str::to_owned);
        let path = string_param_in(&params, "path").map(str::to_owned);
        let authorization = string_param_in(&params, "authorization").map(str::to_owned);
        // Federated (key-sharing) parameters — draft header members
        // `provider_session_id` / `provider_url` / `provider_key`. All three
        // together or none; `SessionRegistration::federated` enforces that.
        let provider_session_id =
            string_param_in(&params, "provider_session_id").map(str::to_owned);
        let provider_url = string_param_in(&params, "provider_url").map(str::to_owned);
        let provider_key = string_param_in(&params, "provider_key").map(str::to_owned);
        registrations.push(SessionRegistration {
            algorithm: algorithm?,
            challenge,
            path: path?,
            authorization,
            provider_session_id,
            provider_url,
            provider_key,
        });
    }
    Some(registrations)
}

/// Extract challenges from `Secure-Session-Challenge` header value(s). Each
/// member is a string item with an optional `id` parameter naming the
/// session it applies to.
pub(crate) fn parse_challenge_header(values: &http::HeaderMap) -> Vec<SessionChallenge> {
    let mut challenges = Vec::new();
    for header_value in values.get_all("Secure-Session-Challenge") {
        let Ok(value) = header_value.to_str() else {
            continue;
        };
        let Some(members) = parse_list(value) else {
            continue;
        };
        for member in members {
            let SfvListMember::Item(item) = member else {
                continue;
            };
            let Some(challenge) = item.bare.as_str() else {
                continue;
            };
            challenges.push(SessionChallenge {
                challenge: challenge.to_owned(),
                session_identifier: item.string_param("id").map(str::to_owned),
            });
        }
    }
    challenges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_origin(url: &str) -> ImmutableOrigin {
        ImmutableOrigin::new(ServoUrl::parse(url).unwrap().as_url())
    }

    fn instructions_json() -> String {
        serde_json::json!({
            "session_identifier": "0",
            "refresh_url": "/device-bound-session-credentials/refresh_session.py",
            "scope": {
                "origin": "",
                "include_site": true,
                "scope_specification": [
                    { "type": "exclude", "domain": "web-platform.test", "path": "/excluded" }
                ],
            },
            "credentials": [
                { "type": "cookie", "name": "auth_cookie", "attributes": "Domain=web-platform.test; Path=/device-bound-session-credentials" }
            ],
            "allowed_refresh_initiators": ["*"],
        })
        .to_string()
    }

    #[test]
    fn parses_full_session_instructions() {
        let instructions =
            parse_session_instructions(instructions_json().as_bytes()).expect("must parse");
        assert_eq!(instructions.session_identifier, "0");
        assert_eq!(instructions.scope, SessionScope::IncludeSite);
        assert_eq!(instructions.credentials.len(), 1);
        assert_eq!(instructions.credentials[0].cookie_name, "auth_cookie");
        assert_eq!(instructions.credentials[0].domain, "web-platform.test");
        assert_eq!(
            instructions.allowed_refresh_initiators,
            vec!["*".to_string()]
        );
        assert_eq!(instructions.scope_specification.len(), 1);
        assert!(!instructions.scope_specification[0].include);
    }

    #[test]
    fn missing_required_field_fails_parse() {
        // `credentials` is required (draft §7.3); its absence fails the parse.
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value.as_object_mut().unwrap().remove("credentials");
        assert!(parse_session_instructions(value.to_string().as_bytes()).is_none());
        // Credentials with an unknown type fail the parse.
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value["credentials"][0]["type"] = "something-else".into();
        assert!(parse_session_instructions(value.to_string().as_bytes()).is_none());
        // A Partitioned credential terminates the session (§8.9 step 9).
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value["credentials"][0]["attributes"] =
            "Domain=web-platform.test; Path=/x; Partitioned".into();
        assert!(parse_session_instructions(value.to_string().as_bytes()).is_none());
    }

    #[test]
    fn optional_members_take_their_documented_defaults() {
        // `refresh_url` absent → None (the create-session step defaults it to
        // the destination URL).
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value.as_object_mut().unwrap().remove("refresh_url");
        let instructions =
            parse_session_instructions(value.to_string().as_bytes()).expect("must parse");
        assert!(instructions.refresh_url.is_none());

        // `allowed_refresh_initiators` absent → EMPTY list (not `["*"]`).
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("allowed_refresh_initiators");
        let instructions =
            parse_session_instructions(value.to_string().as_bytes()).expect("must parse");
        assert!(instructions.allowed_refresh_initiators.is_empty());

        // `scope.origin: ""` is the "not provided" form (the WPT server
        // emits it when no scope origin is configured).
        let instructions =
            parse_session_instructions(instructions_json().as_bytes()).expect("must parse");
        assert!(instructions.scope_origin.is_none());

        // Scope-rule `domain` defaults to `*` and `path` to `/`; credential
        // `attributes` defaults to "".
        let mut value: JsonValue = serde_json::from_str(&instructions_json()).unwrap();
        value["scope"]["scope_specification"] = serde_json::json!([{ "type": "include" }]);
        value["credentials"][0]
            .as_object_mut()
            .unwrap()
            .remove("attributes");
        let instructions =
            parse_session_instructions(value.to_string().as_bytes()).expect("must parse");
        assert_eq!(instructions.scope_specification[0].domain, "*");
        assert_eq!(instructions.scope_specification[0].path, "/");
        assert_eq!(instructions.credentials[0].domain, "");
        assert_eq!(instructions.credentials[0].path, "");
    }

    #[test]
    fn refresh_continue_false_ends_session() {
        let body = serde_json::json!({
            "session_identifier": "0",
            "continue": false,
        })
        .to_string();
        assert_eq!(
            parse_refresh_instructions(body.as_bytes()),
            Some(RefreshInstructions::EndSession)
        );
    }

    #[test]
    fn refresh_defaults_continue_true() {
        let instructions =
            parse_refresh_instructions(instructions_json().as_bytes()).expect("must parse");
        assert!(matches!(instructions, RefreshInstructions::Continue(_)));
    }

    #[test]
    fn registration_header_single_and_multiple() {
        let registrations = parse_registration_header(
            r#"(RS256);path="/start_session.py";challenge="c1", (RS256);path="/start_session.py""#,
        )
        .expect("must parse");
        assert_eq!(registrations.len(), 2);
        assert_eq!(registrations[0].challenge.as_deref(), Some("c1"));
        assert!(registrations[1].challenge.is_none());
        // Header form without a challenge is valid (registration-no-challenge).
        assert!(parse_registration_header(r#"(RS256);path="/x""#).is_some());
    }

    #[test]
    fn registration_header_with_authorization() {
        let registrations =
            parse_registration_header(r#"(RS256);path="/start_session.py";authorization="authcode""#)
                .expect("must parse");
        assert_eq!(registrations[0].authorization.as_deref(), Some("authcode"));
    }

    #[test]
    fn credential_matching_ignores_lifetime_attributes() {
        let credential = SessionCredential {
            cookie_name: "auth_cookie".to_owned(),
            domain: "web-platform.test".to_owned(),
            path: "/device-bound-session-credentials".to_owned(),
            secure: false,
            http_only: false,
            same_site: SameSiteSpec::Lax,
            partitioned: false,
        };
        let cookie_str = "auth_cookie=abcdef0123; Domain=web-platform.test; Path=/device-bound-session-credentials; Expires=Wed, 21 Oct 2027 07:28:00 GMT";
        // The match target is the jar's native cookie (what
        // `cookies_data_for_url` hands out) — the full attribute set must
        // survive into matching.
        let cookie = cookie::Cookie::parse(cookie_str.to_owned()).expect("must parse");
        // Expires on the stored cookie does not break matching.
        assert!(credential.matches_cookie(&cookie));
        // HttpOnly mismatch does break matching.
        let mut credential_http_only = credential.clone();
        credential_http_only.http_only = true;
        assert!(!credential_http_only.matches_cookie(&cookie));
        // Domain mismatch does break matching.
        let mut credential_other_domain = credential.clone();
        credential_other_domain.domain = "other-platform.test".to_owned();
        assert!(!credential_other_domain.matches_cookie(&cookie));
    }

    #[test]
    fn scope_specification_last_to_first() {
        let session = DbscSession {
            session_identifier: "0".to_owned(),
            algorithm: DbscAlgorithm::Rs256,
            refresh_url: ServoUrl::parse(
                "https://web-platform.test/device-bound-session-credentials/refresh_session.py",
            )
            .unwrap(),
            scope_origin: test_origin("https://web-platform.test"),
            scope: SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![
                ScopeSpecificationItem {
                    include: true,
                    domain: "*".to_owned(),
                    path: "/".to_owned(),
                },
                ScopeSpecificationItem {
                    include: false,
                    domain: "web-platform.test".to_owned(),
                    path: "/device-bound-session-credentials".to_owned(),
                },
            ],
            credentials: vec![],
            allowed_refresh_initiators: vec!["*".to_owned()],
            cached_challenge: None,
        };
        // The refresh URL itself is always excluded.
        assert!(!session.url_in_scope(&session.refresh_url));
        // Last-to-first: the exclude entry wins for its path.
        let excluded = ServoUrl::parse(
            "https://web-platform.test/device-bound-session-credentials/other.py",
        )
        .unwrap();
        assert!(!session.url_in_scope(&excluded));
        // The include entry applies outside the excluded path.
        let included = ServoUrl::parse("https://web-platform.test/other/thing.py").unwrap();
        assert!(session.url_in_scope(&included));
        // Same registrable domain (include_site) matches; other site does not.
        let subdomain = ServoUrl::parse("https://a.web-platform.test/x.py").unwrap();
        assert!(session.url_in_scope(&subdomain));
        let other_site = ServoUrl::parse("https://example.com/x.py").unwrap();
        assert!(!session.url_in_scope(&other_site));
    }

    #[test]
    fn origin_scope_excludes_cross_origin() {
        let session = DbscSession {
            session_identifier: "0".to_owned(),
            algorithm: DbscAlgorithm::Rs256,
            refresh_url: ServoUrl::parse("https://web-platform.test/r.py").unwrap(),
            scope_origin: test_origin("https://web-platform.test"),
            scope: SessionScope::Origin,
            include_site: false,
            scope_specification: vec![],
            credentials: vec![],
            allowed_refresh_initiators: vec!["*".to_owned()],
            cached_challenge: None,
        };
        let same_origin = ServoUrl::parse("https://web-platform.test/other.py").unwrap();
        assert!(session.url_in_scope(&same_origin));
        let subdomain = ServoUrl::parse("https://a.web-platform.test/x.py").unwrap();
        assert!(!session.url_in_scope(&subdomain));
    }

    /// The §8.3 refresh permission: same-site / same-origin requests are
    /// always allowed; a cross-site initiator must match a host pattern in
    /// `allowed_refresh_initiators` (hosts, not fetch initiator types — the
    /// Wave-A model compared the fetch initiator label instead, which no
    /// instruction member names: WPT allowed-refresh-initiators sets hosts).
    #[test]
    fn refresh_permission_uses_request_origin_hosts() {
        let mut session = DbscSession {
            session_identifier: "0".to_owned(),
            algorithm: DbscAlgorithm::Rs256,
            refresh_url: ServoUrl::parse("https://web-platform.test/r.py").unwrap(),
            scope_origin: test_origin("https://web-platform.test"),
            scope: SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![],
            credentials: vec![],
            allowed_refresh_initiators: vec!["*.trusted.example".to_owned()],
            cached_challenge: None,
        };
        let same_site = RequestOrigin::from_origin(&test_origin("https://a.web-platform.test"));
        let same_origin = RequestOrigin::from_origin(&test_origin("https://web-platform.test"));
        let cross_site_allowed =
            RequestOrigin::from_origin(&test_origin("https://x.trusted.example"));
        let cross_site_denied = RequestOrigin::from_origin(&test_origin("https://other.example"));
        let apex_of_pattern =
            RequestOrigin::from_origin(&test_origin("https://trusted.example"));

        // Same-site (include_site) and same-origin always pass.
        assert!(session.refresh_permitted_for_origin(&same_site));
        assert!(session.refresh_permitted_for_origin(&same_origin));
        // Cross-site: only listed hosts.
        assert!(session.refresh_permitted_for_origin(&cross_site_allowed));
        assert!(!session.refresh_permitted_for_origin(&cross_site_denied));
        // `*.trusted.example` does not match the apex (draft §8.4).
        assert!(!session.refresh_permitted_for_origin(&apex_of_pattern));

        // Origin-scoped sessions do not admit same-site-but-cross-origin
        // initiators unless listed.
        session.include_site = false;
        session.scope = SessionScope::Origin;
        assert!(!session.refresh_permitted_for_origin(&same_site));
        assert!(session.refresh_permitted_for_origin(&same_origin));

        // Empty list denies every cross-site initiator.
        session.allowed_refresh_initiators.clear();
        assert!(!session.refresh_permitted_for_origin(&cross_site_allowed));
        session.allowed_refresh_initiators = vec!["*".to_owned()];
        assert!(session.refresh_permitted_for_origin(&cross_site_allowed));
        assert!(session.refresh_permitted_for_origin(&cross_site_denied));

        // Opaque origins never refresh.
        let opaque = RequestOrigin::from_origin(&ImmutableOrigin::new_opaque());
        assert!(!session.refresh_permitted_for_origin(&opaque));
    }

    /// §8.6: a credential whose cookie could not be attached to a cross-site
    /// request does not trigger a refresh; a `SameSite=None` credential does.
    #[test]
    fn credential_gate_skips_unattachable_cookies() {
        let lax_credential = SessionCredential {
            cookie_name: "auth_cookie".to_owned(),
            domain: "web-platform.test".to_owned(),
            path: "/device-bound-session-credentials".to_owned(),
            secure: false,
            http_only: false,
            same_site: SameSiteSpec::Lax,
            partitioned: false,
        };
        let mut none_credential = lax_credential.clone();
        none_credential.same_site = SameSiteSpec::None;
        none_credential.secure = true;
        none_credential.cookie_name = "third_party_cookie".to_owned();

        let mut session = DbscSession {
            session_identifier: "0".to_owned(),
            algorithm: DbscAlgorithm::Rs256,
            refresh_url: ServoUrl::parse("https://web-platform.test/r.py").unwrap(),
            scope_origin: test_origin("https://web-platform.test"),
            scope: SessionScope::IncludeSite,
            include_site: true,
            scope_specification: vec![],
            credentials: vec![lax_credential.clone()],
            allowed_refresh_initiators: vec!["*".to_owned()],
            cached_challenge: None,
        };
        // First-party request, cookie missing → refresh needed.
        assert!(session.needs_refresh(&[], false));
        // Cross-site request, cookie would not be attached → no refresh.
        assert!(!session.needs_refresh(&[], true));

        session.credentials = vec![none_credential.clone(), lax_credential];
        // Cross-site: the SameSite=None credential can be attached and is
        // missing → refresh needed.
        assert!(session.needs_refresh(&[], true));
        // Credentials satisfied → no refresh.
        let attached = cookie::Cookie::parse(
            "third_party_cookie=1; Domain=web-platform.test; Path=/device-bound-session-credentials; Secure; SameSite=None".to_owned(),
        )
        .unwrap();
        let lax_attached = cookie::Cookie::parse(
            "auth_cookie=1; Domain=web-platform.test; Path=/device-bound-session-credentials".to_owned(),
        )
        .unwrap();
        assert!(!session.needs_refresh(&[attached, lax_attached], true));
        // §8.7 gate: a cross-site response can set the SameSite=None cookie,
        // so it may cache a challenge.
        assert!(session.can_set_credential_cookie(true));
        session.credentials = vec![none_credential];
        assert!(session.can_set_credential_cookie(true));
        session.credentials = vec![SessionCredential {
            cookie_name: "auth_cookie".to_owned(),
            domain: String::new(),
            path: String::new(),
            secure: false,
            http_only: false,
            same_site: SameSiteSpec::Lax,
            partitioned: false,
        }];
        assert!(!session.can_set_credential_cookie(true));
        assert!(session.can_set_credential_cookie(false));
    }

    /// The well-known URL is built from the exact origin (scheme+host+port);
    /// the URL parser drops the default port again.
    #[test]
    fn well_known_url_from_exact_origin() {
        let url = well_known_url(&test_origin("https://web-platform.test:8443")).unwrap();
        assert_eq!(
            url.as_str(),
            "https://web-platform.test:8443/.well-known/device-bound-sessions"
        );
        let default_port = well_known_url(&test_origin("https://web-platform.test")).unwrap();
        assert_eq!(
            default_port.as_str(),
            "https://web-platform.test/.well-known/device-bound-sessions"
        );
        assert!(well_known_url(&ImmutableOrigin::new_opaque()).is_none());
    }

    /// Federated registration parameters are parsed from the header and
    /// enforced all-or-none.
    #[test]
    fn registration_header_federated_params() {
        let registrations = parse_registration_header(
            r#"(RS256);path="/start";provider_session_id="0";provider_url="https://provider.test/";provider_key="thumb""#,
        )
        .expect("must parse");
        assert_eq!(registrations.len(), 1);
        let federated = registrations[0].federated().expect("federated");
        assert_eq!(
            federated,
            Ok(("0", "https://provider.test/", "thumb"))
        );
        // Non-federated entries have no federated parameters.
        let plain = parse_registration_header(r#"(RS256);path="/start""#).expect("must parse");
        assert!(plain[0].federated().is_none());
        // A partial set is a protocol violation, not a plain registration.
        let partial = parse_registration_header(
            r#"(RS256);path="/start";provider_session_id="0""#,
        )
        .expect("must parse");
        assert_eq!(partial[0].federated(), Some(Err(())));
    }

    #[test]
    fn challenge_header_multiple_ids() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Secure-Session-Challenge",
            http::HeaderValue::from_str(r#""early_challenge1";id="0", "early_challenge2";id="1""#)
                .unwrap(),
        );
        let challenges = parse_challenge_header(&headers);
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].session_identifier.as_deref(), Some("0"));
        assert_eq!(challenges[1].challenge, "early_challenge2");
    }
}
