/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `ImageTrackList` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://wicg.github.io/image-output/#imagetracklist-interface>).
//! Populated by the `ImageDecoder` constructor: a supported, successfully
//! decoded image yields its single selected track; an unsupported type or a
//! failed decode yields the empty list (length 0, selectedIndex -1,
//! selectedTrack null) with a rejected `ready` promise — the WPT "invalid
//! mime type rejects decodeMetadata" oracle.

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{DomObject, Reflector, reflect_dom_object_with_proto};
use std::cell::Cell;

use crate::dom::bindings::codegen::Bindings::ImageTrackListBinding::ImageTrackListMethods;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;
use crate::dom::imagetrack::ImageTrack;
use crate::dom::promise::{Promise, RootedPromise};

/// <https://wicg.github.io/image-output/#dom-imagetracklist>
#[dom_struct]
pub(crate) struct ImageTrackList {
    reflector_: Reflector,
    tracks: DomRefCell<Vec<Dom<ImageTrack>>>,
    selected_index: Cell<i32>,
    /// The `ready` promise, settled by the ImageDecoder constructor outcome
    /// (resolved once tracks are populated, rejected NotSupportedError for an
    /// unsupported type or failed decode).
    ready: Dom<Promise>,
}

impl ImageTrackList {
    fn new_inherited(
        tracks: Vec<Dom<ImageTrack>>,
        selected_index: i32,
        ready: Dom<Promise>,
    ) -> ImageTrackList {
        ImageTrackList {
            reflector_: Reflector::new(),
            tracks: DomRefCell::new(tracks),
            selected_index: Cell::new(selected_index),
            ready,
        }
    }

    /// Build the list for one decoded image: a single selected track when the
    /// decode succeeded, the empty list otherwise. `ready` is created by the
    /// caller so its settlement matches the outcome.
    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        track: Option<DomRoot<ImageTrack>>,
        ready: Dom<Promise>,
    ) -> DomRoot<ImageTrackList> {
        let tracks: Vec<Dom<ImageTrack>> = track
            .as_ref()
            .map(|track| vec![Dom::from_ref(&**track)])
            .unwrap_or_default();
        let selected_index = if track.is_some() { 0 } else { -1 };
        reflect_dom_object_with_proto(
            cx,
            Box::new(ImageTrackList::new_inherited(tracks, selected_index, ready)),
            global,
            proto,
        )
    }
}

impl ImageTrackListMethods<crate::DomTypeHolder> for ImageTrackList {
    /// <https://wicg.github.io/image-output/#dom-imagetracklist-ready>
    #[allow(unsafe_code)]
    fn Ready(&self) -> RootedPromise {
        // Re-wrap the cached ready promise so every access observes the same
        // JS promise object (the fork's promise machinery hands promise
        // getters a fresh root per access — serviceworkercontainer.Ready
        // shape, non-conf getter via the thread context).
        let mut cx = unsafe { JSContext::get_from_thread() }.expect("no active JS context");
        Promise::new_with_js_promise(&cx, self.ready.reflector().get_jsobject())
    }

    /// <https://wicg.github.io/image-output/#dom-imagetracklist-length>
    fn Length(&self) -> u32 {
        self.tracks.borrow().len() as u32
    }

    /// <https://wicg.github.io/image-output/#dom-imagetracklist-selectedindex>
    fn SelectedIndex(&self) -> i32 {
        self.selected_index.get()
    }

    /// <https://wicg.github.io/image-output/#dom-imagetracklist-selectedtrack>
    fn GetSelectedTrack(&self) -> Option<DomRoot<ImageTrack>> {
        let index = self.selected_index.get();
        if index < 0 {
            return None;
        }
        self.tracks
            .borrow()
            .get(index as usize)
            .map(|track| DomRoot::from_ref(&**track))
    }

    /// <https://wicg.github.io/image-output/#dom-imagetracklist>
    fn IndexedGetter(&self, index: u32) -> Option<DomRoot<ImageTrack>> {
        self.tracks
            .borrow()
            .get(index as usize)
            .map(|track| DomRoot::from_ref(&**track))
    }
}
