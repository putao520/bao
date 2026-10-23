/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Device Bound Session Credentials (DBSC) — the net-crate slice.
//!
//! BAO PATCH (REQ-BRW-051): implements the W3C Device Bound Session
//! Credentials draft (2026-09-08 ED) on top of the page network's own
//! exchange path. Module map:
//!
//! - [`sfv`] — the minimal RFC 9651 Structured Field Values parser subset
//!   the DBSC headers need (lists, inner lists, items, string/token
//!   parameters).
//! - [`model`] — session state, session-instructions JSON, scope matching,
//!   credential matching.
//! - [`keys`] — per-session proof-of-possession key pairs and DBSC proof
//!   JWT signing (RS256; other algorithms fail closed).
//! - [`store`] — the per-cookie-partition session/key store hung off
//!   `HttpState` (`HttpState::dbsc`), with refresh backoff state.
//! - [`exchange`] — the single POST driver (`obtain_response_bun`-backed),
//!   the cookie-jar seams, and the session-level registration/refresh
//!   orchestration the fetch hooks call.
//!
//! The module was previously a set of orphan files (`net/lib.rs` never
//! declared it), which is why none of it compiled or tested; the `pub mod
//! dbsc;` declaration in `net/lib.rs` is the E1 fix of the DBSC audit.
//!
//! Wave B (REQ-BRW-051) aligned every face with the draft's algorithms:
//! refresh permission by the request origin's host (§8.3/§8.4), URL scope
//! relative to the session's scope origin (§8.2), the credential and
//! challenge gates with their third-party cookie rules (§8.6/§8.7), the full
//! create-session validation (§8.9, including the `registering_origins`
//! well-known authorization), federated key sharing (§8.11), the
//! `Secure-Session-Skipped` debug header (§9.5), Clear-Site-Data's
//! `cookies`/`storage` targets, and the WebSocket handshake seam.
//!
//! Registered boundaries (honest, not silent):
//! - No JS API exists in the draft; `navigator.deviceBoundSession`'s
//!   `hasSession`/`clearSession` remain default-OFF extension faces and
//!   `setSession` is not implemented (see `script/dom/deviceboundsession.rs`).
//! - Sessions carry no expiration timestamp (§8.9 step 20 is left to UA
//!   policy): they live until terminated by a protocol outcome or a
//!   Clear-Site-Data target.
//! - `quota_exceeded` has no emission point: the per-domain session quota is
//!   a registration reject, and the draft's DoS-quota policy is not wired.
//! - The `device-bound-session-key-sharing` permission is modelled as
//!   "not denied" (the UA has no permission store for it).
//! - Clear-Site-Data clears DBSC sessions only; the cookie jar and HTTP
//!   caches have no Clear-Site-Data consumer anywhere in this tree.
//! - `relying_origins` label-count capping (§8.11 step 17, a "should") is not
//!   implemented; the `.well-known` documents are fetched per registration.

pub(crate) mod exchange;
pub(crate) mod keys;
pub(crate) mod model;
pub(crate) mod store;

// The RFC 9651 (Structured Field Values) slice used by DBSC lives in
// `net_traits::sfv`, shared with `script::dom::speculation` (BAO PATCH,
// REQ-BRW-051 — hoisted from this module by the same-wave SFV convergence).
