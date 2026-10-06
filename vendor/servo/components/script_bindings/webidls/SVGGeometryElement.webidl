/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://svgwg.org/svg2-draft/types.html#InterfaceSVGGeometryElement
// BAO patch (fork-maintained, 2026-09-27): REQ-BRW-046 — enable getTotalLength /
// getPointAtLength. Upstream keeps these commented; the implementations live in
// components/script/dom/svg/svggeometryelement.rs + svg_geometry.rs.
[Exposed=Window, Abstract]
interface SVGGeometryElement : SVGGraphicsElement {
  //[SameObject] readonly attribute SVGAnimatedNumber pathLength;

  //boolean isPointInFill(optional DOMPointInit point = {});
  //boolean isPointInStroke(optional DOMPointInit point = {});
  float getTotalLength();
  DOMPoint getPointAtLength(float distance);
};
