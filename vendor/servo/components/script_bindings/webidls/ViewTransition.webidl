/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// https://drafts.csswg.org/css-view-transitions-2/#idl-index
//
// BAO PATCH (REQ-BRW-051 C2): self-built View Transitions DOM surface.
// Neither servo nor its upstream ships the View Transitions API (no webidl on
// either tree — the bclass-scope report §1 records 373 css-view-transitions
// WPT grid cells as "ViewTransition.webidl absent"), while Chromium ships it
// enabled by default, so the absence is a fingerprinting vector.
//
// The Level 1 state machine (`updateCallbackDone` / `ready` / `finished` /
// `skipTransition()`) plus the Level 2 single-document members (`types`,
// `waitUntil()`) are implemented by `script/dom/viewtransition.rs`. Cross
// document transitions (`pageswap` / `pagereveal`, `@view-transition`) are out
// of scope for this landing.

callback ViewTransitionUpdateCallback = Promise<any> ();

dictionary StartViewTransitionOptions {
  ViewTransitionUpdateCallback? update = null;
  sequence<DOMString>? types = null;
};

[Exposed=Window]
interface ViewTransition {
  readonly attribute Promise<undefined> updateCallbackDone;
  readonly attribute Promise<undefined> ready;
  readonly attribute Promise<undefined> finished;
  undefined skipTransition();
  [SameObject] readonly attribute ViewTransitionTypeSet types;
  undefined waitUntil(Promise<any> promise);
};

// https://drafts.csswg.org/css-view-transitions-2/#the-viewtransitiontypeset-interface
[Exposed=Window]
interface ViewTransitionTypeSet {
  setlike<DOMString>;
};
