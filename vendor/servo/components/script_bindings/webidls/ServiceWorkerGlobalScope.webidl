/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// https://w3c.github.io/ServiceWorker/#serviceworkerglobalscope

[Global=(Worker,ServiceWorker), Exposed=ServiceWorker,
 Pref="dom_serviceworker_enabled"]
interface ServiceWorkerGlobalScope : WorkerGlobalScope {
  // A container for a list of Client objects that correspond to
  // browsing contexts (or shared workers) that are on the origin of this SW
  // Bao vendor patch (REQ-BRW-004 e58 contract B, user ruling 2026-10-04,
  // Chromium-parity; replayed 2026-10-06 after the a7272f16 snapshot swap
  // dissolved the e58 SWGS exposure face): exposed with the minimal Clients
  // implementation (matchAll over the manager's origin-wide enrolled client
  // set — the e70 supersedes form; see Clients.webidl).
  [SameObject] readonly attribute Clients clients;

  //[SameObject] readonly attribute ServiceWorkerRegistration registration;

  // Bao vendor patch (e58 contract B; replayed 2026-10-06): resolves
  // immediately — the worker is only running once active, and the fork has
  // no activation-wait queue. (`Promise<undefined>`, this parser's spelling
  // of the spec's void.)
  [NewObject] Promise<undefined> skipWaiting();

  // Bao vendor patch (e58 contract B; replayed 2026-10-06): exposed;
  // "install" has no dispatch site yet (exposure only, Chrome-parity
  // surface). "activate" is live — dispatch_activate fires it after the
  // worker script evaluates.
  attribute EventHandler oninstall;
  attribute EventHandler onactivate;
  // Bao vendor patch (user ruling 2026-09-09): exposed with the FetchEvent
  // pipeline (REQ-BRW-004 C19 S2a).
  attribute EventHandler onfetch;

  // event
  attribute EventHandler onmessage; // event.source of the message events is Client object
  attribute EventHandler onmessageerror;
};
