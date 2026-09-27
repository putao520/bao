/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://svgwg.org/svg2-draft/types.html#InterfaceSVGGraphicsElement
// BAO patch (fork-maintained, 2026-09-27): REQ-BRW-046 — enable the geometry/reflection
// method surface (getBBox/getCTM/getScreenCTM). Upstream keeps these commented; the
// implementations live in components/script/dom/svg/svggraphicselement.rs + svg_geometry.rs.
dictionary SVGBoundingBoxOptions {
  boolean fill = true;
  boolean stroke = false;
  boolean markers = false;
  boolean clipped = false;
};

[Exposed=Window, Abstract]
interface SVGGraphicsElement : SVGElement {
  //[SameObject] readonly attribute SVGAnimatedTransformList transform;

  DOMRect getBBox(optional SVGBoundingBoxOptions options = {});
  DOMMatrix? getCTM();
  DOMMatrix? getScreenCTM();
};

//SVGGraphicsElement includes SVGTests;
