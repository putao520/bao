/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `ImageTrack` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://wicg.github.io/image-output/#imagetrack-interface>). Tracks are
//! produced by the `ImageDecoder` constructor once the component decode
//! surface has run; the type carries the full surface for feature detection
//! and idlharness.

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};
use std::cell::Cell;

use crate::dom::bindings::codegen::Bindings::ImageTrackBinding::ImageTrackMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;

/// <https://wicg.github.io/image-output/#dom-imagetrack>
#[dom_struct]
pub(crate) struct ImageTrack {
    reflector_: Reflector,
    animated: bool,
    frame_count: u32,
    /// `None` = infinite repetition (looping animation — the `unrestricted
    /// float` surface reports `Infinity`), `Some(n)` = a finite repetition
    /// count (0 = play once).
    repetition_count: Option<u32>,
    selected: Cell<bool>,
}

impl ImageTrack {
    fn new_inherited(
        animated: bool,
        frame_count: u32,
        repetition_count: Option<u32>,
        selected: bool,
    ) -> ImageTrack {
        ImageTrack {
            reflector_: Reflector::new(),
            animated,
            frame_count,
            repetition_count,
            selected: Cell::new(selected),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        animated: bool,
        frame_count: u32,
        repetition_count: Option<u32>,
        selected: bool,
    ) -> DomRoot<ImageTrack> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(ImageTrack::new_inherited(
                animated,
                frame_count,
                repetition_count,
                selected,
            )),
            global,
            proto,
        )
    }

    /// The single track of a decoded image, selected by default (the ImageDecoder
    /// surface has exactly one track per decode in this slice).
    pub(crate) fn new_selected(
        cx: &mut JSContext,
        global: &GlobalScope,
        animated: bool,
        frame_count: u32,
        repetition_count: Option<u32>,
    ) -> DomRoot<ImageTrack> {
        ImageTrack::new(
            cx,
            global,
            None,
            animated,
            frame_count,
            repetition_count,
            true,
        )
    }
}

impl ImageTrackMethods<crate::DomTypeHolder> for ImageTrack {
    /// <https://wicg.github.io/image-output/#dom-imagetrack-animated>
    fn Animated(&self) -> bool {
        self.animated
    }

    /// <https://wicg.github.io/image-output/#dom-imagetrack-framecount>
    fn FrameCount(&self) -> u32 {
        self.frame_count
    }

    /// <https://wicg.github.io/image-output/#dom-imagetrack-repetitioncount>
    fn RepetitionCount(&self) -> f32 {
        match self.repetition_count {
            None => f32::INFINITY,
            Some(count) => count as f32,
        }
    }

    /// <https://wicg.github.io/image-output/#dom-imagetrack-selected>
    fn Selected(&self) -> bool {
        self.selected.get()
    }

    /// <https://wicg.github.io/image-output/#dom-imagetrack-selected>
    fn SetSelected(&self, value: bool) {
        self.selected.set(value);
    }
}
