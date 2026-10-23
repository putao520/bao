/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ViewTransitionTypeSet` — <https://drafts.csswg.org/css-view-transitions-2/#the-viewtransitiontypeset-interface>
//!
//! BAO PATCH (REQ-BRW-051 C2): self-built. The set of strings carried by a
//! `ViewTransition` (`transition.types`). The spec is explicit that the set
//! "represents a set of strings, without special semantics" — in particular a
//! member need not be a valid `<custom-ident>` — so no validation happens
//! here; `:active-view-transition-type()` matching (which does need
//! custom-ident validation) lives in the style system and is not part of this
//! landing.
//!
//! Storage is an order-preserving `Vec` rather than a hash set: the sets are
//! tiny (a handful of transition types), insertion order is observable through
//! `keys()`/iteration, and `DOMString` has no `Hash` implementation in this
//! tree.

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::cell::DomRefCell;
use script_bindings::like::Setlike;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use script_bindings::root::DomRoot;

use crate::dom::bindings::codegen::Bindings::ViewTransitionBinding::ViewTransitionTypeSetMethods;
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;

// The generated binding routes `add`/`has`/`delete`/`clear` through the
// `Setlike` trait (below) and `size` through the generated
// `ViewTransitionTypeSetMethods` trait; `keys`/`values`/`entries`/`forEach` and
// `[Symbol.iterator]` come from the generated `IterableIterator` plumbing,
// which only needs `Setlike` (it implies `Maplike`, which implies `Iterable`).
#[dom_struct]
pub(crate) struct ViewTransitionTypeSet {
    reflector_: Reflector,
    /// The set entries, in insertion order.
    entries: DomRefCell<Vec<DOMString>>,
}

impl ViewTransitionTypeSet {
    fn new_inherited(types: Option<Vec<DOMString>>) -> Self {
        let mut entries = Vec::new();
        if let Some(types) = types {
            for type_ in types {
                if !entries.contains(&type_) {
                    entries.push(type_);
                }
            }
        }
        ViewTransitionTypeSet {
            reflector_: Reflector::new(),
            entries: DomRefCell::new(entries),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        types: Option<Vec<DOMString>>,
    ) -> DomRoot<Self> {
        reflect_dom_object(
            cx,
            Box::new(ViewTransitionTypeSet::new_inherited(types)),
            global,
        )
    }
}

/// The `size` getter is generated through the interface methods trait rather
/// than through `Setlike` (`add`/`has`/`delete`/`clear` do go through
/// `Setlike`).
impl ViewTransitionTypeSetMethods<crate::DomTypeHolder> for ViewTransitionTypeSet {
    fn Size(&self) -> u32 {
        self.entries.borrow().len() as u32
    }
}

impl Setlike for ViewTransitionTypeSet {
    type Key = DOMString;

    fn get_index(&self, _cx: &mut JSContext, index: u32) -> Option<Self::Key> {
        self.entries.borrow().get(index as usize).cloned()
    }

    fn size(&self, _cx: &mut JSContext) -> u32 {
        self.entries.borrow().len() as u32
    }

    fn add(&self, _cx: &mut JSContext, key: Self::Key) {
        let mut entries = self.entries.borrow_mut();
        if !entries.contains(&key) {
            entries.push(key);
        }
    }

    fn has(&self, _cx: &mut JSContext, key: Self::Key) -> bool {
        self.entries.borrow().contains(&key)
    }

    fn clear(&self, _cx: &mut JSContext) {
        self.entries.borrow_mut().clear();
    }

    fn delete(&self, _cx: &mut JSContext, key: Self::Key) -> bool {
        let mut entries = self.entries.borrow_mut();
        match entries.iter().position(|entry| *entry == key) {
            Some(index) => {
                entries.remove(index);
                true
            },
            None => false,
        }
    }
}
