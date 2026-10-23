/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#stylepropertymap
[Exposed=Window, Pref="dom_typed_om_enabled"]
interface StylePropertyMap : StylePropertyMapReadOnly {
    [Throws] undefined set(USVString property, (CSSStyleValue or USVString)... values);
    [Throws] undefined append(USVString property, (CSSStyleValue or USVString)... values);
    [Throws] undefined delete(USVString property);
    [Throws] undefined clear();
};
