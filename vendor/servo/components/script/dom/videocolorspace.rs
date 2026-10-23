/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `VideoColorSpace` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#videocolorspace-interface>). The
//! `toJSON()` JSONifier is a registered boundary (webidl header).

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};

use crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::{
    VideoColorSpaceInit, VideoColorSpaceMethods, VideoColorPrimaries,
    VideoMatrixCoefficients, VideoTransferCharacteristics,
};
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;

/// <https://www.w3.org/TR/webcodecs/#videocolorspace>
#[dom_struct]
pub(crate) struct VideoColorSpace {
    reflector_: Reflector,
    primaries: Option<VideoColorPrimaries>,
    transfer: Option<VideoTransferCharacteristics>,
    matrix: Option<VideoMatrixCoefficients>,
    full_range: Option<bool>,
}

impl VideoColorSpace {
    fn new_inherited(
        primaries: Option<VideoColorPrimaries>,
        transfer: Option<VideoTransferCharacteristics>,
        matrix: Option<VideoMatrixCoefficients>,
        full_range: Option<bool>,
    ) -> VideoColorSpace {
        VideoColorSpace {
            reflector_: Reflector::new(),
            primaries,
            transfer,
            matrix,
            full_range,
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoColorSpaceInit,
    ) -> DomRoot<VideoColorSpace> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(VideoColorSpace::new_inherited(
                init.primaries,
                init.transfer,
                init.matrix,
                init.fullRange,
            )),
            global,
            proto,
        )
    }

    /// Construct from an optional `VideoColorSpaceInit`, filling absent
    /// members with the format-family defaults the WebCodecs buffer-init
    /// steps prescribe: RGB family (RGBA/RGBX/BGRA/BGRX) defaults to
    /// `bt709 / bt709 / rgb` with full range; the YUV families default to
    /// `bt709 / bt709 / bt709` limited range. Consumed by `VideoFrame`
    /// construction (buffer init and decoded frames). Member values the page
    /// supplied always win over the defaults.
    pub(crate) fn from_init(
        cx: &mut JSContext,
        global: &GlobalScope,
        init: Option<&VideoColorSpaceInit>,
        is_rgb: bool,
    ) -> DomRoot<VideoColorSpace> {
        let mut color_space = VideoColorSpace::new_inherited(None, None, None, None);
        if let Some(init) = init {
            color_space.primaries = init.primaries;
            color_space.transfer = init.transfer;
            color_space.matrix = init.matrix;
            color_space.full_range = init.fullRange;
        }
        if color_space.primaries.is_none() {
            color_space.primaries = Some(VideoColorPrimaries::Bt709);
        }
        if color_space.transfer.is_none() {
            color_space.transfer = Some(VideoTransferCharacteristics::Bt709);
        }
        if color_space.matrix.is_none() {
            color_space.matrix = Some(if is_rgb {
                VideoMatrixCoefficients::Rgb
            } else {
                VideoMatrixCoefficients::Bt709
            });
        }
        if color_space.full_range.is_none() {
            color_space.full_range = Some(is_rgb);
        }
        reflect_dom_object_with_proto(cx, Box::new(color_space), global, None)
    }

    /// Construct from already-resolved member values — the decoded-frame
    /// delivery path (VideoDecoder/ImageDecoder) carries plain string values
    /// from the component `FrameColorSpace`, which the caller maps to enums.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        cx: &mut JSContext,
        global: &GlobalScope,
        primaries: Option<VideoColorPrimaries>,
        transfer: Option<VideoTransferCharacteristics>,
        matrix: Option<VideoMatrixCoefficients>,
        full_range: bool,
    ) -> DomRoot<VideoColorSpace> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(VideoColorSpace::new_inherited(
                primaries,
                transfer,
                matrix,
                Some(full_range),
            )),
            global,
            None,
        )
    }
}

impl VideoColorSpaceMethods<crate::DomTypeHolder> for VideoColorSpace {
    /// <https://www.w3.org/TR/webcodecs/#dom-videocolorspace-videocolorspace>
    /// Non-throwing constructor (no `[Throws]` in the webidl): returns the
    /// object directly.
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &VideoColorSpaceInit,
    ) -> DomRoot<VideoColorSpace> {
        VideoColorSpace::new(cx, global, proto, init)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videocolorspace-primaries>
    fn GetPrimaries(&self) -> Option<VideoColorPrimaries> {
        self.primaries
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videocolorspace-transfer>
    fn GetTransfer(&self) -> Option<VideoTransferCharacteristics> {
        self.transfer
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videocolorspace-matrix>
    fn GetMatrix(&self) -> Option<VideoMatrixCoefficients> {
        self.matrix
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videocolorspace-fullrange>
    fn GetFullRange(&self) -> Option<bool> {
        self.full_range
    }
}
