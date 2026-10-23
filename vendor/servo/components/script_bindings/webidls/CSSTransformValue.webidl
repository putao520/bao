// BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM. The DOM implementation
// lives in `script::dom::css` and is enabled by the `dom_typed_om_enabled` pref
// (registered in `config/prefs.rs`, expanded by
// `--enable-experimental-web-platform-features`).
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://www.mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#transformvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSTransformValue : CSSStyleValue {
    [Throws] constructor(sequence<CSSTransformComponent> transforms);
    iterable<CSSTransformComponent>;
    readonly attribute unsigned long length;
    [Throws] getter CSSTransformComponent (unsigned long index);
    [Throws] setter undefined (unsigned long index, CSSTransformComponent val);

    readonly attribute boolean is2D;
    [Throws] DOMMatrix toMatrix();
};

// https://drafts.css-houdini.org/css-typed-om-1/#csstransformcomponent
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSTransformComponent {
    stringifier;
    attribute boolean is2D;
    [Throws] DOMMatrix toMatrix();
};

// https://drafts.css-houdini.org/css-typed-om-1/#csstranslate
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSTranslate : CSSTransformComponent {
    [Throws] constructor(CSSNumericValue x, CSSNumericValue y, optional CSSNumericValue z);
    [Throws] attribute CSSNumericValue x;
    [Throws] attribute CSSNumericValue y;
    [Throws] attribute CSSNumericValue z;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssrotate
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSRotate : CSSTransformComponent {
    [Throws] constructor(CSSNumericValue angle);
    [Throws] constructor(CSSNumberish x, CSSNumberish y, CSSNumberish z, CSSNumericValue angle);
    attribute CSSNumberish x;
    attribute CSSNumberish y;
    attribute CSSNumberish z;
    [Throws] attribute CSSNumericValue angle;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssscale
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSScale : CSSTransformComponent {
    [Throws] constructor(CSSNumberish x, CSSNumberish y, optional CSSNumberish z);
    [Throws] attribute CSSNumberish x;
    [Throws] attribute CSSNumberish y;
    [Throws] attribute CSSNumberish z;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssskew
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSSkew : CSSTransformComponent {
    [Throws] constructor(CSSNumericValue ax, CSSNumericValue ay);
    [Throws] attribute CSSNumericValue ax;
    [Throws] attribute CSSNumericValue ay;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssskewx
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSSkewX : CSSTransformComponent {
    [Throws] constructor(CSSNumericValue ax);
    [Throws] attribute CSSNumericValue ax;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssskewy
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSSkewY : CSSTransformComponent {
    [Throws] constructor(CSSNumericValue ay);
    [Throws] attribute CSSNumericValue ay;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssperspectivevalue
typedef (CSSNumericValue or CSSKeywordValue or DOMString) CSSPerspectiveValue;

// https://drafts.css-houdini.org/css-typed-om-1/#cssperspective
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSPerspective : CSSTransformComponent {
    [Throws] constructor(CSSPerspectiveValue length);
    [Throws] attribute CSSPerspectiveValue length;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmatrixcomponent
dictionary CSSMatrixComponentOptions {
    boolean is2D;
};

[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMatrixComponent : CSSTransformComponent {
    [Throws] constructor(DOMMatrixReadOnly matrix, optional CSSMatrixComponentOptions options = {});
    [Throws] attribute DOMMatrix matrix;
};
