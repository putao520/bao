/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/*
 * The origin of this IDL file is
 * https://w3c.github.io/mathml-core/#dom-and-javascript
 *
 * BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
 * MathML elements in this Servo snapshot are created as bare `Element`s, so
 * MathML content has no `dataset`, `style`, `tabIndex`, `focus()`/`blur()` and
 * no global event handlers at all. MathML Core §"DOM and JavaScript" requires
 * MathML elements to expose the same surface as HTML and SVG elements, and the
 * MathML tests of the WPT suite use it directly
 * (`mathml/relations/html5-tree/html-or-svg-or-mathml-element-interfaces.html`,
 * `mathml/support/invalid-markup.js`, …).
 *
 * The mixins below are the ones SVG elements already include, so MathML
 * elements end up with the identical attribute surface.
 */

[Exposed=Window]
interface MathMLElement : Element {
  readonly attribute DOMStringMap dataset;
};

MathMLElement includes GlobalEventHandlers;
MathMLElement includes ElementCSSInlineStyle;
MathMLElement includes HTMLOrSVGElement;
