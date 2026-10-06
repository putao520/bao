/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use html5ever::{LocalName, Prefix};
use stylo_dom::ElementState;

use crate::dom::bindings::codegen::Bindings::SVGGeometryElementBinding::SVGGeometryElementMethods;
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::document::Document;
use crate::dom::dompoint::DOMPoint;
use crate::dom::element::Element;
use crate::dom::node::virtualmethods::VirtualMethods;
use crate::dom::svg::svg_geometry;
use crate::dom::svg::svggraphicselement::SVGGraphicsElement;

#[dom_struct]
pub(crate) struct SVGGeometryElement {
    svggraphicselement: SVGGraphicsElement,
}

impl SVGGeometryElement {
    pub(crate) fn new_inherited(
        tag_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> SVGGeometryElement {
        SVGGeometryElement::new_inherited_with_state(
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
    ) -> SVGGeometryElement {
        SVGGeometryElement {
            svggraphicselement: SVGGraphicsElement::new_inherited_with_state(
                state, tag_name, prefix, document,
            ),
        }
    }
}

impl VirtualMethods for SVGGeometryElement {
    fn super_type(&self) -> Option<&dyn VirtualMethods> {
        Some(self.upcast::<SVGGraphicsElement>() as &dyn VirtualMethods)
    }
}

// BAO patch (fork-maintained, 2026-09-27): REQ-BRW-046 — real path computation
// methods. getTotalLength/getPointAtLength are computed from geometry attributes:
// `d` paths via stylo-parsed commands flattened adaptively (relative error
// ≤ 0.01%), other geometry shapes via their equivalent perimeters/point lists.
//
// The fork's codegen emits these methods without an explicit `cx` parameter, so
// the thread's JSContext is obtained via svg_geometry::current_cx().
impl SVGGeometryElementMethods<crate::DomTypeHolder> for SVGGeometryElement {
    /// <https://svgwg.org/svg2-draft/types.html#__svg__SVGGeometryElement__getTotalLength>
    fn GetTotalLength(&self) -> Finite<f32> {
        let element = self.upcast::<Element>();
        Finite::wrap(svg_geometry::total_length(element) as f32)
    }

    /// <https://svgwg.org/svg2-draft/types.html#__svg__SVGGeometryElement__getPointAtLength>
    fn GetPointAtLength(&self, distance: Finite<f32>) -> DomRoot<DOMPoint> {
        let mut cx = svg_geometry::current_cx();
        let element = self.upcast::<Element>();
        let (x, y) = svg_geometry::point_at_length(element, *distance as f64);
        DOMPoint::new(&mut cx, &self.global(), x, y, 0.0, 1.0)
    }
}
