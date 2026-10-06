/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use html5ever::{LocalName, Prefix};
use stylo_dom::ElementState;

use crate::dom::bindings::codegen::Bindings::SVGGraphicsElementBinding::{
    SVGBoundingBoxOptions, SVGGraphicsElementMethods,
};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::document::Document;
use crate::dom::dommatrix::DOMMatrix;
use crate::dom::domrect::DOMRect;
use crate::dom::element::Element;
use crate::dom::node::virtualmethods::VirtualMethods;
use crate::dom::svg::svg_geometry;
use crate::dom::svg::svgelement::SVGElement;

#[dom_struct]
pub(crate) struct SVGGraphicsElement {
    svgelement: SVGElement,
}

impl SVGGraphicsElement {
    pub(crate) fn new_inherited(
        tag_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> SVGGraphicsElement {
        SVGGraphicsElement::new_inherited_with_state(
            ElementState::empty(),
            tag_name,
            prefix,
            document,
        )
    }

    pub(crate) fn new_inherited_with_state(
        state: ElementState,
        tag_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> SVGGraphicsElement {
        SVGGraphicsElement {
            svgelement: SVGElement::new_inherited_with_state(state, tag_name, prefix, document),
        }
    }
}

// BAO patch (fork-maintained, 2026-09-27): REQ-BRW-046 — real geometry/reflection
// methods. getBBox returns the bounding box in the element's own user space
// (computed from geometry attributes; container elements union their children).
// getCTM maps the element's user space to the nearest SVG viewport. getScreenCTM
// currently reports the same local→viewport matrix: servo has no viewport→screen
// channel wired for SVG here (v1 boundary, see REQ-BRW-046).
//
// The fork's codegen emits these methods without an explicit `cx` parameter, so
// the thread's JSContext is obtained via svg_geometry::current_cx().
impl SVGGraphicsElementMethods<crate::DomTypeHolder> for SVGGraphicsElement {
    /// <https://svgwg.org/svg2-draft/types.html#__svg__SVGGraphicsElement__getBBox>
    fn GetBBox(&self, _options: &SVGBoundingBoxOptions) -> DomRoot<DOMRect> {
        let mut cx = svg_geometry::current_cx();
        let element = self.upcast::<Element>();
        let bbox = svg_geometry::geometry_bbox(element);
        DOMRect::new(
            &mut cx,
            &self.global(),
            bbox.x0,
            bbox.y0,
            bbox.width(),
            bbox.height(),
        )
    }

    /// <https://svgwg.org/svg2-draft/types.html#__svg__SVGGraphicsElement__getCTM>
    fn GetCTM(&self) -> Option<DomRoot<DOMMatrix>> {
        let mut cx = svg_geometry::current_cx();
        let element = self.upcast::<Element>();
        let matrix = svg_geometry::local_to_viewport_matrix(element);
        Some(DOMMatrix::new(
            &mut cx,
            &self.global(),
            true,
            svg_geometry::affine_to_transform_3d(matrix),
        ))
    }

    /// <https://svgwg.org/svg2-draft/types.html#__svg__SVGGraphicsElement__getScreenCTM>
    fn GetScreenCTM(&self) -> Option<DomRoot<DOMMatrix>> {
        // v1 boundary: no viewport→screen channel is wired for SVG yet, so the
        // screen CTM is the local→viewport CTM (documented in REQ-BRW-046).
        self.GetCTM()
    }
}

impl VirtualMethods for SVGGraphicsElement {
    fn super_type(&self) -> Option<&dyn VirtualMethods> {
        Some(self.upcast::<SVGElement>() as &dyn VirtualMethods)
    }
}
