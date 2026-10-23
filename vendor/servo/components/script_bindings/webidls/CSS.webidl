/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://drafts.csswg.org/cssom/#namespacedef-css
 */

// https://drafts.csswg.org/cssom/#the-css.escape()-method
[Exposed=(Window, Worker)]
namespace CSS {
  [Throws] CSSOMString escape(CSSOMString ident);
};

// https://drafts.csswg.org/css-conditional-3/#dom-css-supports
partial namespace CSS {
  boolean supports(CSSOMString property, CSSOMString value);
  boolean supports(CSSOMString conditionText);
};

// https://drafts.css-houdini.org/css-paint-api/#dom-css-paintworklet
partial namespace CSS {
    [SameObject, Pref="dom_worklet_enabled", Exposed=Window] readonly attribute Worklet paintWorklet;
};

// https://drafts.css-houdini.org/css-properties-values-api/#the-registerproperty-function
dictionary PropertyDefinition {
  required DOMString name;
           DOMString syntax       = "*";
  required boolean   inherits;
           DOMString initialValue;
};

partial namespace CSS {
  [Throws, Exposed=Window] undefined registerProperty(PropertyDefinition definition);
};

// https://drafts.css-houdini.org/css-typed-om-1/#numeric-factory
partial namespace CSS {
    [Pref="dom_typed_om_enabled"] CSSUnitValue number(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue percent(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cap(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue ch(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue em(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue ex(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue ic(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rcap(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rch(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rem(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rex(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue ric(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rlh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vw(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vb(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vmin(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue vmax(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svw(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svb(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svmin(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue svmax(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvw(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvb(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvmin(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue lvmax(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvw(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvb(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvmin(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dvmax(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqw(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqh(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqb(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqmin(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cqmax(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue cm(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue mm(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue Q(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue in(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue pt(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue pc(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue px(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue deg(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue grad(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue rad(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue turn(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue s(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue ms(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue Hz(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue kHz(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dpi(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dpcm(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue dppx(double value);
    [Pref="dom_typed_om_enabled"] CSSUnitValue fr(double value);
};
