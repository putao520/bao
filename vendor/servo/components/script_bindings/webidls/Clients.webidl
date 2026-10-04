/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://w3c.github.io/ServiceWorker/#clients-interface

// Bao vendor patch (REQ-BRW-004 e58 contract B, user ruling 2026-10-04,
// Chromium-parity): upstream has no Clients interface at all. Minimal subset
// per the ruling: `matchAll` resolves with the single registering client the
// manager models (its creation URL), `get` resolves with no match (no client
// registry exists). `includeUncontrolled`/`type` are accepted but not
// filtered on (the one-client model has no controlled set);
// `claim()` is deliberately not implemented (spec claim()/clients full
// family stays out of the minimal face).
[Pref="dom_serviceworker_enabled", Exposed=ServiceWorker]
interface Clients {
  // `Promise<FrozenArray<Client>>` is the spec type; this parser cannot nest
  // FrozenArray inside Promise (every other such form in this tree is
  // commented out — getRegistrations precedent), so the array travels as
  // `any` (resolved with a real JS Array of Client objects).
  [NewObject] Promise<any> matchAll(optional ClientQueryOptions options = {});
  [NewObject] Promise<Client> get(DOMString id);
  //[NewObject] Promise<WindowClient?> openWindow(USVString url);
  //[NewObject] Promise<void> claim();
};

// https://w3c.github.io/ServiceWorker/#clientqueryoptions
dictionary ClientQueryOptions {
  boolean includeUncontrolled = false;
  ClientType type = "window";
};

// https://w3c.github.io/ServiceWorker/#clienttype
enum ClientType { "window", "worker", "sharedworker", "all" };
