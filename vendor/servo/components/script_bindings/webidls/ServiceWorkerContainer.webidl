/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://w3c.github.io/ServiceWorker/#serviceworkercontainer-interface
[Pref="dom_serviceworker_enabled", Exposed=(Window,Worker)]
interface ServiceWorkerContainer : EventTarget {
  readonly attribute ServiceWorker? controller;
  readonly attribute Promise<ServiceWorkerRegistration> ready;

  [NewObject] Promise<ServiceWorkerRegistration> register(USVString scriptURL,
                                                          optional RegistrationOptions options = {});

  [NewObject] Promise<(ServiceWorkerRegistration or undefined)> getRegistration(optional USVString clientURL = "");
  //[NewObject] Promise<FrozenArray<ServiceWorkerRegistration>> getRegistrations();

  //void startMessages();

  // events
  // Bao vendor patch (REQ-BRW-004 e57 contract A, user ruling 2026-10-04,
  // Chromium-parity): ready/onmessage/onmessageerror/oncontrollerchange
  // exposed — upstream leaves all of these commented out. `onmessage` is
  // live because the container already dispatches worker→client message
  // events (the MessageFromWorker arm); `onmessageerror` has no dispatch
  // site yet (exposure only, matching Chrome's surface).
  attribute EventHandler oncontrollerchange;
  //attribute EventHandler onerror;
  attribute EventHandler onmessage; // event.source of message events is ServiceWorker object
  attribute EventHandler onmessageerror;
};

dictionary RegistrationOptions {
  USVString scope;
  WorkerType type = "classic";
  ServiceWorkerUpdateViaCache updateViaCache = "imports";
};
