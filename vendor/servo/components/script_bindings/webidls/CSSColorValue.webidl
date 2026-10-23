// BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM. The DOM implementation
// lives in `script::dom::css` and is enabled by the `dom_typed_om_enabled` pref
// (registered in `config/prefs.rs`, expanded by
// `--enable-experimental-web-platform-features`).
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://www.mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#csscolorvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSColorValue : CSSStyleValue {
    [Exposed=Window, Throws] static (CSSColorValue or CSSStyleValue) parse(USVString cssText);
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssrgb
typedef (CSSKeywordValue or DOMString) CSSKeywordish;
typedef (CSSNumberish or CSSKeywordish) CSSColorRGBComp;
typedef (CSSNumberish or CSSKeywordish) CSSColorPercent;
typedef (CSSNumberish or CSSKeywordish) CSSColorNumber;
typedef (CSSNumberish or CSSKeywordish) CSSColorAngle;

[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSRGB : CSSColorValue {
    [Throws] constructor(CSSColorRGBComp r, CSSColorRGBComp g, CSSColorRGBComp b, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorRGBComp r;
    [Throws] attribute CSSColorRGBComp g;
    [Throws] attribute CSSColorRGBComp b;
    [Throws] attribute CSSColorPercent alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#csshsl
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSHSL : CSSColorValue {
    [Throws] constructor(CSSColorAngle h, CSSColorPercent s, CSSColorPercent l, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorAngle h;
    [Throws] attribute CSSColorPercent s;
    [Throws] attribute CSSColorPercent l;
    [Throws] attribute CSSColorPercent alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#csshwb
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSHWB : CSSColorValue {
    [Throws] constructor(CSSNumericValue h, CSSNumberish w, CSSNumberish b, optional CSSNumberish alpha = 1);
    [Throws] attribute CSSNumericValue h;
    [Throws] attribute CSSNumberish w;
    [Throws] attribute CSSNumberish b;
    [Throws] attribute CSSNumberish alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#csslab
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSLab : CSSColorValue {
    [Throws] constructor(CSSColorPercent l, CSSColorNumber a, CSSColorNumber b, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorPercent l;
    [Throws] attribute CSSColorNumber a;
    [Throws] attribute CSSColorNumber b;
    [Throws] attribute CSSColorPercent alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#csslch
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSLCH : CSSColorValue {
    [Throws] constructor(CSSColorPercent l, CSSColorPercent c, CSSColorAngle h, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorPercent l;
    [Throws] attribute CSSColorPercent c;
    [Throws] attribute CSSColorAngle h;
    [Throws] attribute CSSColorPercent alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssoklab
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSOKLab : CSSColorValue {
    [Throws] constructor(CSSColorPercent l, CSSColorNumber a, CSSColorNumber b, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorPercent l;
    [Throws] attribute CSSColorNumber a;
    [Throws] attribute CSSColorNumber b;
    [Throws] attribute CSSColorPercent alpha;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssoklch
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSOKLCH : CSSColorValue {
    [Throws] constructor(CSSColorPercent l, CSSColorPercent c, CSSColorAngle h, optional CSSColorPercent alpha = 1);
    [Throws] attribute CSSColorPercent l;
    [Throws] attribute CSSColorPercent c;
    [Throws] attribute CSSColorAngle h;
    [Throws] attribute CSSColorPercent alpha;
};
