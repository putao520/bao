/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#stylepropertymapreadonly
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface StylePropertyMapReadOnly {
    iterable<USVString, sequence<CSSStyleValue>>;

    [Throws] (undefined or CSSStyleValue) get(USVString property);
    [Throws] sequence<CSSStyleValue> getAll(USVString property);
    [Throws] boolean has(USVString property);

    readonly attribute unsigned long size;
};
