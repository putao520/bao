/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The `MathMLElement` interface.
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//! MathML elements used to be created as bare `Element`s, which left them
//! without `dataset`, `style`, `tabIndex`, `focus()`/`blur()` and without the
//! global event handler attributes. MathML Core's "DOM and JavaScript" section
//! requires those to be exposed, and the MathML tests of the WPT suite use them
//! (for example `mathml/support/invalid-markup.js` reads `element.dataset`
//! while generating its fixtures).

use dom_struct::dom_struct;
use html5ever::{LocalName, Prefix, local_name, ns};
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::codegen::GenericBindings::ElementBinding::ScrollLogicalPosition;
use script_bindings::codegen::GenericBindings::WindowBinding::ScrollBehavior;
use script_bindings::str::DOMString;
use stylo_dom::ElementState;

use crate::dom::attributes::domstringmap::DOMStringMap;
use crate::dom::css::stylepropertymap::StylePropertyMap;
use crate::dom::bindings::codegen::Bindings::HTMLOrSVGElementBinding::FocusOptions;
use crate::dom::bindings::codegen::Bindings::MathMLElementBinding::MathMLElementMethods;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{Dom, DomRoot, MutNullableDom};
use crate::dom::css::cssstyledeclaration::{
    CSSModificationAccess, CSSStyleDeclaration, CSSStyleOwner,
};
use crate::dom::document::Document;
use crate::dom::document::focus::FocusableArea;
use crate::dom::element::attributes::storage::AttrRef;
use crate::dom::element::{AttributeMutation, Element};
use crate::dom::node::focus::FocusTrigger;
use crate::dom::node::virtualmethods::VirtualMethods;
use crate::dom::node::{Node, NodeTraits};
use crate::dom::window::scrolling_box::{ScrollAxisState, ScrollRequirement};

#[dom_struct]
pub(crate) struct MathMLElement {
    element: Element,
    style_decl: MutNullableDom<CSSStyleDeclaration>,
    /// Bao fork self-build (REQ-BRW-051 C2): typed-OM `attributeStyleMap`
    /// backing slot, mirroring `SVGElement`/`HTMLElement`.
    attribute_style_map: MutNullableDom<StylePropertyMap>,
    dataset: MutNullableDom<DOMStringMap>,
}

impl MathMLElement {
    fn new_inherited(
        tag_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> MathMLElement {
        MathMLElement {
            element: Element::new_inherited_with_state(
                ElementState::empty(),
                tag_name,
                ns!(mathml),
                prefix,
                document,
            ),
            style_decl: Default::default(),
            attribute_style_map: Default::default(),
            dataset: Default::default(),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        tag_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
        proto: Option<HandleObject>,
    ) -> DomRoot<MathMLElement> {
        Node::reflect_node_with_proto(
            cx,
            Box::new(MathMLElement::new_inherited(tag_name, prefix, document)),
            document,
            proto,
        )
    }

    fn as_element(&self) -> &Element {
        self.upcast::<Element>()
    }
}

impl VirtualMethods for MathMLElement {
    fn super_type(&self) -> Option<&dyn VirtualMethods> {
        Some(self.as_element() as &dyn VirtualMethods)
    }

    fn attribute_mutated(
        &self,
        cx: &mut JSContext,
        attr: AttrRef<'_>,
        mutation: AttributeMutation,
    ) {
        self.super_type()
            .unwrap()
            .attribute_mutated(cx, attr, mutation);
        // The `nonce` attribute of the HTMLOrSVGElement mixin is backed by an
        // internal slot on the element, which has to be kept in sync with the
        // content attribute.
        if let (&local_name!("nonce"), mutation) = (attr.local_name(), mutation) {
            let element = self.as_element();
            match mutation {
                AttributeMutation::Set(_) => {
                    element.update_nonce_internal_slot(String::from(&**attr.value()), cx.no_gc());
                },
                AttributeMutation::Removed => {
                    element.update_nonce_internal_slot(String::new(), cx.no_gc());
                },
            }
        }
    }
}

impl MathMLElementMethods<crate::DomTypeHolder> for MathMLElement {
    /// <https://html.spec.whatwg.org/multipage/#the-style-attribute>

    /// Bao fork self-build (REQ-BRW-051 C2): a mutable `StylePropertyMap`
    /// backed by the element's inline style declaration (the third
    /// `ElementCSSInlineStyle` includer besides HTMLElement/SVGElement).
    fn AttributeStyleMap(&self, cx: &mut JSContext) -> DomRoot<StylePropertyMap> {
        self.attribute_style_map.or_init(|| {
            let declaration = self.Style(cx);
            StylePropertyMap::new(cx, &self.owner_global(), declaration)
        })
    }

    fn Style(&self, cx: &mut JSContext) -> DomRoot<CSSStyleDeclaration> {
        self.style_decl.or_init(|| {
            let global = self.owner_window();
            CSSStyleDeclaration::new(
                cx,
                &global,
                CSSStyleOwner::Element(Dom::from_ref(self.upcast())),
                None,
                CSSModificationAccess::ReadWrite,
            )
        })
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-dataset>
    fn Dataset(&self, cx: &mut JSContext) -> DomRoot<DOMStringMap> {
        self.dataset
            .or_init(|| DOMStringMap::new(cx, self.upcast::<Element>()))
    }

    // <https://html.spec.whatwg.org/multipage/#globaleventhandlers>
    global_event_handlers!();

    /// <https://html.spec.whatwg.org/multipage/#dom-noncedelement-nonce>
    fn Nonce(&self) -> DOMString {
        self.as_element().nonce_value().into()
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-noncedelement-nonce>
    fn SetNonce(&self, cx: &mut JSContext, value: DOMString) {
        self.as_element()
            .update_nonce_internal_slot(String::from(value), cx.no_gc())
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-fe-autofocus>
    fn Autofocus(&self) -> bool {
        self.element.has_attribute(&local_name!("autofocus"))
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-fe-autofocus>
    fn SetAutofocus(&self, cx: &mut JSContext, autofocus: bool) {
        self.element
            .set_bool_attribute(cx, &local_name!("autofocus"), autofocus);
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-focus>
    fn Focus(&self, cx: &mut JSContext, options: &FocusOptions) {
        if !self
            .upcast::<Node>()
            .run_the_focusing_steps(cx, None, FocusTrigger::Other)
        {
            return;
        }
        if !options.preventScroll {
            let scroll_axis = ScrollAxisState {
                position: ScrollLogicalPosition::Center,
                requirement: ScrollRequirement::IfNotVisible,
            };
            self.upcast::<Element>().scroll_into_view_with_options(
                cx,
                ScrollBehavior::Smooth,
                scroll_axis,
                scroll_axis,
                None,
                None,
            );
        }
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-blur>
    fn Blur(&self, cx: &mut JSContext) {
        if !self.as_element().focus_state() {
            return;
        }
        self.owner_document()
            .focus_handler()
            .focus(cx, &FocusableArea::Viewport);
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-tabindex>
    fn TabIndex(&self) -> i32 {
        self.element.tab_index()
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-tabindex>
    fn SetTabIndex(&self, cx: &mut JSContext, tab_index: i32) {
        self.element
            .set_attribute(cx, &local_name!("tabindex"), tab_index.into());
    }
}
