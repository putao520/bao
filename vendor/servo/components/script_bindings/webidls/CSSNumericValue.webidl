// BAO fork self-build (REQ-BRW-051 C2): CSS Typed OM. The DOM implementation
// lives in `script::dom::css` and is enabled by the `dom_typed_om_enabled` pref
// (registered in `config/prefs.rs`, expanded by
// `--enable-experimental-web-platform-features`).
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://www.mozilla.org/MPL/2.0/. */

// https://drafts.css-houdini.org/css-typed-om-1/#cssnumericvalue
typedef (double or CSSNumericValue) CSSNumberish;

// https://drafts.css-houdini.org/css-typed-om-1/#cssnumericbasetype
enum CSSNumericBaseType {
    "length",
    "angle",
    "time",
    "frequency",
    "resolution",
    "flex",
    "percent",
};

// https://drafts.css-houdini.org/css-typed-om-1/#dictdef-cssnumerictype
dictionary CSSNumericType {
    long length;
    long angle;
    long time;
    long frequency;
    long resolution;
    long flex;
    long percent;
    CSSNumericBaseType percentHint;
};

// https://drafts.css-houdini.org/css-typed-om-1/#numericvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSNumericValue : CSSStyleValue {
    [Throws] CSSNumericValue add(CSSNumberish... values);
    [Throws] CSSNumericValue sub(CSSNumberish... values);
    [Throws] CSSNumericValue mul(CSSNumberish... values);
    [Throws] CSSNumericValue div(CSSNumberish... values);
    [Throws] CSSNumericValue min(CSSNumberish... values);
    [Throws] CSSNumericValue max(CSSNumberish... values);

    boolean equals(CSSNumberish... value);

    [Throws] CSSUnitValue to(USVString unit);
    [Throws] CSSMathSum toSum(USVString... units);
    [Throws] CSSNumericType type();

    [Throws] static CSSNumericValue parse(USVString cssText);
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssunitvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSUnitValue : CSSNumericValue {
    [Throws] constructor(double value, USVString unit);
    attribute unrestricted double value;
    readonly attribute USVString unit;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathvalue
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathValue : CSSNumericValue {
    readonly attribute CSSMathOperator operator;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathsum
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathSum : CSSMathValue {
    [Throws] constructor(CSSNumberish... args);
    readonly attribute CSSNumericArray values;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathproduct
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathProduct : CSSMathValue {
    [Throws] constructor(CSSNumberish... args);
    readonly attribute CSSNumericArray values;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathnegate
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathNegate : CSSMathValue {
    [Throws] constructor(CSSNumberish arg);
    readonly attribute CSSNumericValue value;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathinvert
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathInvert : CSSMathValue {
    [Throws] constructor(CSSNumberish arg);
    readonly attribute CSSNumericValue value;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathmin
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathMin : CSSMathValue {
    [Throws] constructor(CSSNumberish... args);
    readonly attribute CSSNumericArray values;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathmax
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathMax : CSSMathValue {
    [Throws] constructor(CSSNumberish... args);
    readonly attribute CSSNumericArray values;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathclamp
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSMathClamp : CSSMathValue {
    [Throws] constructor(CSSNumberish lower, CSSNumberish value, CSSNumberish upper);
    readonly attribute CSSNumericValue lower;
    readonly attribute CSSNumericValue value;
    readonly attribute CSSNumericValue upper;
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssnumericarray
[Exposed=(Window, Worker), Pref="dom_typed_om_enabled"]
interface CSSNumericArray {
    iterable<CSSNumericValue>;
    readonly attribute unsigned long length;
    [Throws] getter CSSNumericValue (unsigned long index);
};

// https://drafts.css-houdini.org/css-typed-om-1/#cssmathoperator
enum CSSMathOperator {
    "sum",
    "product",
    "negate",
    "invert",
    "min",
    "max",
    "clamp",
};
