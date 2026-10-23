// BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM. The DOM implementation
// lives in `script::dom::css` and is enabled by the `dom_typed_om_enabled` pref
// (registered in `config/prefs.rs`, expanded by
// `--enable-experimental-web-platform-features`).
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://www.mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#csskeywordvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSKeywordValue : CSSStyleValue {
    [Throws] constructor(USVString value);
    [Throws] attribute USVString value;
};
