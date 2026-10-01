/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use html5ever::{LocalName, Prefix, local_name};
use js::context::JSContext;
use js::rust::HandleObject;

use crate::dom::bindings::codegen::Bindings::SVGAElementBinding::SVGAElementMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{DomRoot, MutNullableDom};
use crate::dom::document::Document;
use crate::dom::domtokenlist::DOMTokenList;
use crate::dom::node::Node;
use crate::dom::node::virtualmethods::VirtualMethods;
use crate::dom::svg::svggraphicselement::SVGGraphicsElement;

#[dom_struct]
pub(crate) struct SVGAElement {
    svggraphicselement: SVGGraphicsElement,
    /// <https://svgwg.org/svg2-draft/linking.html#InterfaceSVGAElement>
    rel_list: MutNullableDom<DOMTokenList>,
}

impl SVGAElement {
    fn new_inherited(
        local_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> SVGAElement {
        SVGAElement {
            svggraphicselement: SVGGraphicsElement::new_inherited(local_name, prefix, document),
            rel_list: Default::default(),
        }
    }

    pub(crate) fn new(
        cx: &mut js::context::JSContext,
        local_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
        proto: Option<HandleObject>,
    ) -> DomRoot<SVGAElement> {
        Node::reflect_node_with_proto(
            cx,
            Box::new(SVGAElement::new_inherited(local_name, prefix, document)),
            document,
            proto,
        )
    }
}

impl SVGAElementMethods<crate::DomTypeHolder> for SVGAElement {
    /// <https://svgwg.org/svg2-draft/linking.html#InterfaceSVGAElement>
    fn RelList(&self, cx: &mut JSContext) -> DomRoot<DOMTokenList> {
        self.rel_list
            .or_init(|| DOMTokenList::new(cx, self.upcast(), &local_name!("rel"), None))
    }
}

impl VirtualMethods for SVGAElement {
    fn super_type(&self) -> Option<&dyn VirtualMethods> {
        Some(self.upcast::<SVGGraphicsElement>() as &dyn VirtualMethods)
    }
}
