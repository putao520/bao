/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsapi::JSTracer;
use script_bindings::callback::{OwnerWindow, RootedCallback, TracedCallback};
use script_bindings::reflector::{Reflector, reflect_weak_referenceable_dom_object};
use smallvec::SmallVec;

use crate::dom::bindings::callback::ExceptionHandling::Rethrow;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::Bindings::NodeFilterBinding::{NodeFilter, NodeFilterConstants};
use crate::dom::bindings::codegen::Bindings::NodeIteratorBinding::NodeIteratorMethods;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::{Dom, DomRoot, MutDom};
use crate::dom::bindings::trace::JSTraceable;
use crate::dom::bindings::weakref::{WeakRef, WeakRefVec};
use crate::dom::document::Document;
use crate::dom::iterators::ShadowIncluding;
use crate::dom::node::Node;

#[dom_struct]
pub(crate) struct NodeIterator {
    reflector_: Reflector,
    root_node: Dom<Node>,
    #[ignore_malloc_size_of = "Defined in rust-mozjs"]
    reference_node: MutDom<Node>,
    pointer_before_reference_node: Cell<bool>,
    what_to_show: u32,
    filter: Filter,
    active: Cell<bool>,
}

impl NodeIterator {
    fn new_inherited(
        root_node: &Node,
        what_to_show: u32,
        node_filter: Option<RootedCallback<NodeFilter>>,
    ) -> NodeIterator {
        NodeIterator {
            reflector_: Reflector::new(),
            root_node: Dom::from_ref(root_node),
            reference_node: MutDom::new(root_node),
            pointer_before_reference_node: Cell::new(true),
            what_to_show,
            filter: match node_filter {
                None => Filter::None,
                Some(callback) => Filter::Callback(callback.to_traced()),
            },
            active: Cell::new(false),
        }
    }

    pub(crate) fn new_with_filter(
        cx: &mut JSContext,
        document: &Document,
        root_node: &Node,
        what_to_show: u32,
        node_filter: Option<RootedCallback<NodeFilter>>,
    ) -> DomRoot<NodeIterator> {
        let iterator = reflect_weak_referenceable_dom_object(
            cx,
            Rc::new(NodeIterator::new_inherited(
                root_node,
                what_to_show,
                node_filter,
            )),
            document.window(),
        );
        // Register with the root's node document so the NodeIterator pre-remove
        // steps can retarget this iterator when nodes are removed. Weak entries
        // are pruned on each pre-remove walk (and at trace time), so a dropped
        // iterator never outlives its registry entry.
        root_node
            .owner_doc()
            .node_iterators()
            .push(WeakRef::new(&iterator));
        iterator
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        document: &Document,
        root_node: &Node,
        what_to_show: u32,
        node_filter: Option<RootedCallback<NodeFilter>>,
    ) -> DomRoot<NodeIterator> {
        NodeIterator::new_with_filter(cx, document, root_node, what_to_show, node_filter)
    }
}

impl NodeIteratorMethods<crate::DomTypeHolder> for NodeIterator {
    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-root>
    fn Root(&self) -> DomRoot<Node> {
        DomRoot::from_ref(&*self.root_node)
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-whattoshow>
    fn WhatToShow(&self) -> u32 {
        self.what_to_show
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-filter>
    fn GetFilter(&self) -> Option<RootedCallback<NodeFilter>> {
        match self.filter {
            Filter::None => None,
            Filter::Callback(ref nf) => Some(nf.root()),
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-referencenode>
    fn ReferenceNode(&self) -> DomRoot<Node> {
        self.reference_node.get()
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-pointerbeforereferencenode>
    fn PointerBeforeReferenceNode(&self) -> bool {
        self.pointer_before_reference_node.get()
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-nextnode>
    fn NextNode(&self, cx: &mut JSContext) -> Fallible<Option<DomRoot<Node>>> {
        // https://dom.spec.whatwg.org/#concept-NodeIterator-traverse
        // Step 1.
        let node = self.reference_node.get();

        // Step 2.
        let mut before_node = self.pointer_before_reference_node.get();

        // Step 3-1.
        if before_node {
            before_node = false;

            // Step 3-2.
            let result = self.accept_node(cx, &node)?;

            // Step 3-3.
            if result == NodeFilterConstants::FILTER_ACCEPT {
                // Step 4.
                self.reference_node.set(&node);
                self.pointer_before_reference_node.set(before_node);

                return Ok(Some(node));
            }
        }

        // Step 3-1.
        for following_node in node.following_nodes(&self.root_node, ShadowIncluding::No) {
            // Step 3-2.
            let result = self.accept_node(cx, &following_node)?;

            // Step 3-3.
            if result == NodeFilterConstants::FILTER_ACCEPT {
                // Step 4.
                self.reference_node.set(&following_node);
                self.pointer_before_reference_node.set(before_node);

                return Ok(Some(following_node));
            }
        }

        Ok(None)
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-previousnode>
    fn PreviousNode(&self, cx: &mut JSContext) -> Fallible<Option<DomRoot<Node>>> {
        // https://dom.spec.whatwg.org/#concept-NodeIterator-traverse
        // Step 1.
        let node = self.reference_node.get();

        // Step 2.
        let mut before_node = self.pointer_before_reference_node.get();

        // Step 3-1.
        if !before_node {
            before_node = true;

            // Step 3-2.
            let result = self.accept_node(cx, &node)?;

            // Step 3-3.
            if result == NodeFilterConstants::FILTER_ACCEPT {
                // Step 4.
                self.reference_node.set(&node);
                self.pointer_before_reference_node.set(before_node);

                return Ok(Some(node));
            }
        }

        // Step 3-1.
        for preceding_node in node.preceding_nodes(&self.root_node) {
            // Step 3-2.
            let result = self.accept_node(cx, &preceding_node)?;

            // Step 3-3.
            if result == NodeFilterConstants::FILTER_ACCEPT {
                // Step 4.
                self.reference_node.set(&preceding_node);
                self.pointer_before_reference_node.set(before_node);

                return Ok(Some(preceding_node));
            }
        }

        Ok(None)
    }

    /// <https://dom.spec.whatwg.org/#dom-nodeiterator-detach>
    fn Detach(&self) {
        // This method intentionally left blank.
    }
}

impl NodeIterator {
    /// <https://dom.spec.whatwg.org/#concept-node-filter>
    fn accept_node(&self, cx: &mut JSContext, node: &Node) -> Fallible<u16> {
        // Step 1.
        if self.active.get() {
            return Err(Error::InvalidState(Some(
                "Node iterator cannot be active".into(),
            )));
        }
        // Step 2.
        let n = node.NodeType() - 1;
        // Step 3.
        if (self.what_to_show & (1 << n)) == 0 {
            return Ok(NodeFilterConstants::FILTER_SKIP);
        }

        match self.filter {
            // Step 4.
            Filter::None => Ok(NodeFilterConstants::FILTER_ACCEPT),
            Filter::Callback(ref callback) => {
                // Step 5.
                self.active.set(true);
                // Step 6.
                let result = callback.AcceptNode_(cx, self, node, Rethrow);
                // Step 7.
                self.active.set(false);
                // Step 8.
                result
            },
        }
    }
}

#[derive(JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
pub(crate) enum Filter {
    None,
    Callback(TracedCallback<NodeFilter>),
}

impl OwnerWindow<crate::DomTypeHolder> for NodeIterator {}

/// <https://dom.spec.whatwg.org/#concept-node-remove> step 4 and
/// <https://dom.spec.whatwg.org/#concept-node-move> step 10 entry point: run
/// the <https://dom.spec.whatwg.org/#nodeiterator-pre-removing-steps> for
/// `node` against every iterator whose root's node document is `node`'s node
/// document.
///
/// The filter is not consulted here and no user code runs, so the walk cannot
/// re-enter the registry.
pub(crate) fn node_iterator_pre_remove(node: &Node) {
    let document = node.owner_doc();
    for iterator in document.node_iterators().live_node_iterators() {
        iterator.pre_remove_step(node);
    }
}

/// <https://dom.spec.whatwg.org/#concept-node-adopt> bookkeeping for the
/// pre-remove scope: the node iterator pre-removing steps
/// ([node_iterator_pre_remove]) run for every iterator "whose root's node
/// document is node's node document", so a live iterator's registration must
/// follow its root's node document. When adoption moves `node` (and its
/// subtree) from `old_document` to `new_document`, re-register every live
/// iterator rooted at `node` or at one of its descendants.
///
/// Must run before the adoption loop swaps the subtree's node documents (the
/// caller passes `old_document` explicitly). No user code runs here — the
/// filter is not consulted and adoption holds both documents' script and
/// layout blockers — so the remove+push pair is atomic as observed.
pub(crate) fn node_iterators_migrate_on_adopt(
    node: &Node,
    old_document: &Document,
    new_document: &Document,
) {
    let old_registry = old_document.node_iterators();
    for iterator in old_registry.live_node_iterators() {
        let root: &Node = &iterator.root_node;
        if root == node || node.is_inclusive_ancestor_of(root) {
            new_document.node_iterators().push(old_registry.remove(&iterator));
        }
    }
}

impl NodeIterator {
    /// <https://dom.spec.whatwg.org/#nodeiterator-pre-removing-steps>
    fn pre_remove_step(&self, removed: &Node) {
        // Step 1. Set reference to the result of adjusting a node pointer given
        // reference, this iterator, and toBeRemovedNode. (The candidate
        // reference is transient traversal state this implementation never
        // materializes — step 2's adjustment of it is unobservable here.)
        if let Some((node, pointer_before)) = Self::adjust_node_pointer(
            &self.root_node,
            &self.reference_node.get(),
            self.pointer_before_reference_node.get(),
            removed,
        ) {
            self.reference_node.set(&node);
            self.pointer_before_reference_node.set(pointer_before);
        }
    }

    /// <https://dom.spec.whatwg.org/#nodeiterator-adjust-a-node-pointer>
    ///
    /// Returns `None` when the pointer is to be returned unchanged.
    fn adjust_node_pointer(
        root: &Node,
        reference: &Node,
        pointer_before: bool,
        removed: &Node,
    ) -> Option<(DomRoot<Node>, bool)> {
        // Step 1. If toBeRemovedNode is not an inclusive ancestor of
        // nodePointer's node, or toBeRemovedNode is an inclusive ancestor of
        // root, then return nodePointer.
        if !removed.is_inclusive_ancestor_of(reference) ||
            removed.is_inclusive_ancestor_of(root)
        {
            return None;
        }

        // Step 2. If nodePointer's pointer before is true:
        if pointer_before {
            // Step 2-1. Let next be toBeRemovedNode's first following node that
            // is an inclusive descendant of root and is not an inclusive
            // descendant of toBeRemovedNode, if there is such a node; otherwise
            // null.
            //
            // "Following" is the tree-following concept, which excludes
            // toBeRemovedNode's own subtree (the FollowingNodeIterator used by
            // nextNode() walks into it instead). Walk up to the nearest
            // ancestor with a next sibling: toBeRemovedNode passed the gate, so
            // it is a strict descendant of root — a sibling found before
            // reaching root is an inclusive descendant of root, and reaching
            // root itself means no such node exists.
            //
            // Step 2-2. If next is non-null, then return (next, true).
            let mut current = DomRoot::from_ref(removed);
            let next = loop {
                if let Some(sibling) = current.GetNextSibling() {
                    break Some(sibling);
                }
                match current.GetParentNode() {
                    Some(parent) if &*parent != root => current = parent,
                    _ => break None,
                }
            };
            if let Some(next) = next {
                return Some((next, true));
            }
        }

        // Step 3. Let newNode be toBeRemovedNode's parent, if toBeRemovedNode's
        // previous sibling is null; otherwise the inclusive descendant of
        // toBeRemovedNode's previous sibling that appears last in tree order.
        let new_node = match removed.GetPreviousSibling() {
            Some(sibling) => {
                let mut last = sibling;
                while let Some(child) = last.GetLastChild() {
                    last = child;
                }
                last
            },
            // toBeRemovedNode passed the gate above, so it is a strict
            // descendant of root and always has a parent.
            None => removed
                .GetParentNode()
                .expect("toBeRemovedNode is not an inclusive ancestor of root"),
        };

        // Step 4. Return (newNode, false).
        Some((new_node, false))
    }
}

/// The weak list of live iterators registered on a document, pruned lazily.
/// Mirrors [`crate::dom::range::WeakRangeVec`].
#[derive(MallocSizeOf)]
pub(crate) struct WeakNodeIteratorVec {
    cell: RefCell<WeakRefVec<NodeIterator>>,
}

impl Default for WeakNodeIteratorVec {
    fn default() -> Self {
        WeakNodeIteratorVec {
            cell: RefCell::new(WeakRefVec::new()),
        }
    }
}

impl WeakNodeIteratorVec {
    /// Get a rooted version of the contents of this [`WeakNodeIteratorVec`]
    pub(crate) fn live_node_iterators(&self) -> SmallVec<[DomRoot<NodeIterator>; 4]> {
        let cell = self.cell.borrow();
        if cell.is_empty() {
            return Default::default();
        }
        cell.iter().filter_map(|iterator| iterator.root()).collect()
    }

    pub(crate) fn push(&self, ref_: WeakRef<NodeIterator>) {
        self.cell.borrow_mut().push(ref_);
    }

    /// Remove an iterator from this vector, returning the removed weak
    /// reference (mirrors [`crate::dom::range::WeakRangeVec::remove`]). The
    /// caller keeps the iterator alive across the call.
    fn remove(&self, iterator: &NodeIterator) -> WeakRef<NodeIterator> {
        let mut iterators = self.cell.borrow_mut();
        let position = iterators
            .iter()
            .position(|ref_| ref_ == iterator)
            .expect("live iterator must be registered in its root's node document");
        iterators.swap_remove(position)
    }
}

#[expect(unsafe_code)]
unsafe impl JSTraceable for WeakNodeIteratorVec {
    unsafe fn trace(&self, _: *mut JSTracer) {
        self.cell.borrow_mut().retain_alive()
    }
}
