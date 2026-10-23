/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// BAO PATCH (REQ-BRW-051): Device Bound Session Credentials — the JS query
// face of the net-crate DBSC slice (session store in `DbscStore`).
//
// Wave-A scope: hasSession/clearSession only. The imperative `setSession`
// registration API is deliberately absent, and Wave B confirmed that it has
// no authority to pin from: no WPT fixture in this tree uses it, and the
// spec (https://w3c.github.io/webappsec-dbsc/) defines no JS API at all —
// registration is header-driven. See script/dom/deviceboundsession.rs for
// the full evidence. Carries its own `Pref="dom_dbsc_enabled"` kill switch,
// default OFF — with the pref off neither this interface nor the navigator
// attribute is exposed.
[SecureContext, Exposed=Window, Pref="dom_dbsc_enabled"]
interface DeviceBoundSession : EventTarget {
  [NewObject] Promise<boolean> hasSession(USVString url);
  [NewObject] Promise<undefined> clearSession(USVString url, optional USVString sessionIdentifier);
};
