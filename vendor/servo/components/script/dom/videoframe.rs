/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `VideoFrame` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#videoframe-interface>). Scope boundaries
//! registered in the webidl header: only the buffer-backed constructor; the
//! 10/12-bit plane surfaces are honest NotSupportedError faces (the decode
//! plane narrows to 8-bit). Additional registered boundaries of this slice:
//! `copyTo` supports same-format rect crops — the `layout` destination-layout
//! option and the `colorSpace` conversion option reject with
//! NotSupportedError; the returned `PlaneLayout` sequence reports the cropped
//! copy region (cumulative offsets, crop stride), not the source stride.

use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};
use script_bindings::root::Dom;

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
use crate::dom::bindings::codegen::Bindings::DOMRectReadOnlyBinding::DOMRectInit;
use crate::dom::bindings::codegen::Bindings::VideoFrameBinding::{
    PlaneLayout, VideoFrameBufferInit, VideoFrameCopyToOptions, VideoFrameMetadata,
    VideoFrameMethods, VideoPixelFormat,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise};
use crate::dom::videocolorspace::VideoColorSpace;
use webcodecs::video_frame::{DecodedVideoFrame, FrameColorSpace, FrameFormat};

/// <https://www.w3.org/TR/webcodecs/#videoframe>
#[dom_struct]
pub(crate) struct VideoFrame {
    reflector_: Reflector,
    /// `None` once the frame is `close()`d.
    inner: DomRefCell<Option<VideoFrameInner>>,
    color_space: Dom<VideoColorSpace>,
}

/// The immutable resource description of one frame. Plane bytes are shared by
/// `clone()` through `Rc` (frames are immutable after construction, per the
/// WebCodecs resource model).
#[derive(Clone, JSTraceable, MallocSizeOf)]
struct VideoFrameInner {
    format: VideoPixelFormat,
    coded_width: u32,
    coded_height: u32,
    /// Visible rect in coded pixels (crop applied on top of the coded frame).
    visible_x: i32,
    visible_y: i32,
    visible_width: u32,
    visible_height: u32,
    rotation: f64,
    flip: bool,
    display_width: u32,
    display_height: u32,
    timestamp: i64,
    duration: Option<u64>,
    /// Per-plane bytes. Tightly packed unless the constructor's `layout`
    /// option said otherwise (the `strides` below describe the source rows).
    /// `Rc<Vec<u8>>` (not `Rc<[u8]>`) because the traceable/malloc-size derive
    /// surface has no sized-type hole for `Rc<[u8]>`; the sharing semantics the
    /// module doc claims are unchanged.
    #[conditional_malloc_size_of]
    planes: Vec<Rc<Vec<u8>>>,
    /// Per-plane row stride in bytes of the source layout.
    strides: Vec<u32>,
}

/// Per-plane geometry of an 8-bit format for a coded size: pixel dimensions
/// plus the horizontal/vertical sub-sampling factors relative to the luma
/// plane. `bytes_per_pixel` is 2 for the NV12 interleaved-chroma plane.
#[derive(Clone, Copy)]
struct PlaneGeom {
    pixels_w: u32,
    pixels_h: u32,
    bytes_per_pixel: u32,
    hscale: u32,
    vscale: u32,
}

impl PlaneGeom {
    fn tightly_packed_stride(&self) -> u32 {
        self.pixels_w * self.bytes_per_pixel
    }
    fn plane_size(&self) -> u64 {
        u64::from(self.pixels_w) * u64::from(self.bytes_per_pixel) * u64::from(self.pixels_h)
    }
}

/// The 10/12-bit P10/P12 surfaces: registered honest boundary — the buffer
/// constructor and `copyTo` plane selection reject them (the decode plane
/// narrows every source to 8-bit per the gst backend contract).
fn is_ten_or_twelve_bit(format: VideoPixelFormat) -> bool {
    matches!(
        format,
        VideoPixelFormat::I420P10
            | VideoPixelFormat::I420P12
            | VideoPixelFormat::I420AP10
            | VideoPixelFormat::I420AP12
            | VideoPixelFormat::I422P10
            | VideoPixelFormat::I422P12
            | VideoPixelFormat::I422AP10
            | VideoPixelFormat::I422AP12
            | VideoPixelFormat::I444P10
            | VideoPixelFormat::I444P12
            | VideoPixelFormat::I444AP10
            | VideoPixelFormat::I444AP12
    )
}

/// <https://www.w3.org/TR/webcodecs/#videoframe-buffer-configuration-format>
fn pixel_format_geometry(
    format: VideoPixelFormat,
    width: u32,
    height: u32,
) -> Option<Vec<PlaneGeom>> {
    if is_ten_or_twelve_bit(format) {
        return None;
    }
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    let luma = PlaneGeom {
        pixels_w: width,
        pixels_h: height,
        bytes_per_pixel: 1,
        hscale: 1,
        vscale: 1,
    };
    let chroma = PlaneGeom {
        pixels_w: chroma_w,
        pixels_h: chroma_h,
        bytes_per_pixel: 1,
        hscale: 2,
        vscale: 2,
    };
    let alpha = PlaneGeom {
        pixels_w: width,
        pixels_h: height,
        bytes_per_pixel: 1,
        hscale: 1,
        vscale: 1,
    };
    let interleaved_chroma = PlaneGeom {
        pixels_w: chroma_w,
        pixels_h: chroma_h,
        bytes_per_pixel: 2,
        hscale: 2,
        vscale: 2,
    };
    let packed = PlaneGeom {
        pixels_w: width,
        pixels_h: height,
        bytes_per_pixel: 4,
        hscale: 1,
        vscale: 1,
    };
    Some(match format {
        VideoPixelFormat::I420 => vec![luma, chroma, chroma],
        VideoPixelFormat::I420A => vec![luma, chroma, chroma, alpha],
        VideoPixelFormat::I422 => vec![
            luma,
            PlaneGeom {
                pixels_w: chroma_w,
                pixels_h: height,
                bytes_per_pixel: 1,
                hscale: 2,
                vscale: 1,
            },
            PlaneGeom {
                pixels_w: chroma_w,
                pixels_h: height,
                bytes_per_pixel: 1,
                hscale: 2,
                vscale: 1,
            },
        ],
        VideoPixelFormat::I444 => vec![luma, luma, luma],
        VideoPixelFormat::NV12 => vec![luma, interleaved_chroma],
        VideoPixelFormat::RGBA
        | VideoPixelFormat::RGBX
        | VideoPixelFormat::BGRA
        | VideoPixelFormat::BGRX => vec![packed],
        // Handled above (early return).
        VideoPixelFormat::I420P10
        | VideoPixelFormat::I420P12
        | VideoPixelFormat::I420AP10
        | VideoPixelFormat::I420AP12
        | VideoPixelFormat::I422P10
        | VideoPixelFormat::I422P12
        | VideoPixelFormat::I422A
        | VideoPixelFormat::I422AP10
        | VideoPixelFormat::I422AP12
        | VideoPixelFormat::I444P10
        | VideoPixelFormat::I444P12
        | VideoPixelFormat::I444A
        | VideoPixelFormat::I444AP10
        | VideoPixelFormat::I444AP12 => return None,
    })
}

fn is_rgb_format(format: VideoPixelFormat) -> bool {
    matches!(
        format,
        VideoPixelFormat::RGBA
            | VideoPixelFormat::RGBX
            | VideoPixelFormat::BGRA
            | VideoPixelFormat::BGRX
    )
}

/// One plane's crop-copy description (spec `copyTo` steps).
#[derive(Clone, Copy)]
struct PlaneCopy {
    /// Plane-internal byte offset of the first copied row.
    src_offset: usize,
    /// Bytes between consecutive source rows.
    src_stride: usize,
    /// Bytes per copied row (the cropped width in this plane's pixels).
    row_bytes: usize,
    /// Number of copied rows.
    rows: usize,
}

impl VideoFrame {
    fn new_inherited(inner: VideoFrameInner, color_space: Dom<VideoColorSpace>) -> VideoFrame {
        VideoFrame {
            reflector_: Reflector::new(),
            inner: DomRefCell::new(Some(inner)),
            color_space,
        }
    }

    fn open_inner(&self) -> Option<VideoFrameInner> {
        // `Ref` itself is `Clone`, so deref before cloning the option.
        (*self.inner.borrow()).clone()
    }

    #[allow(unsafe_code)]
    fn thread_cx() -> JSContext {
        unsafe { JSContext::get_from_thread() }.expect("no active JS context")
    }

    fn new_dom_rect(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> DomRoot<crate::dom::geometry::domrectreadonly::DOMRectReadOnly> {
        let global = self.global();
        let mut cx = Self::thread_cx();
        crate::dom::geometry::domrectreadonly::DOMRectReadOnly::new(
            &mut cx,
            &global,
            None,
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height),
        )
    }

    /// Validate a `DOMRectInit`-shaped crop against a containing rect.
    /// Returns `(x, y, width, height)` in coded pixels. Non-integral members
    /// are a TypeError; a rect outside `contain_w × contain_h` is a
    /// RangeError (the copy/crop face the WPT copyTo oracle asserts).
    fn validated_crop(
        rect: &DOMRectInit,
        contain_w: u32,
        contain_h: u32,
    ) -> Fallible<(i32, i32, u32, u32)> {
        let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
        if x.fract() != 0.0 || y.fract() != 0.0 || w.fract() != 0.0 || h.fract() != 0.0 {
            return Err(Error::Type(c"rect members must be integral".to_owned()));
        }
        let (xi, yi, wi, hi) = (x as i64, y as i64, w as i64, h as i64);
        if xi < 0
            || yi < 0
            || wi <= 0
            || hi <= 0
            || xi + wi > i64::from(contain_w)
            || yi + hi > i64::from(contain_h)
        {
            return Err(Error::Range(c"rect is out of bounds".to_owned()));
        }
        Ok((xi as i32, yi as i32, wi as u32, hi as u32))
    }

    /// The crop rect for a copy: `options.rect` if supplied (validated against
    /// the *visible* rect — the copy surface is the visible region), the
    /// visible rect otherwise.
    fn copy_crop(
        &self,
        f: &VideoFrameInner,
        options: &VideoFrameCopyToOptions,
    ) -> Fallible<(i32, i32, u32, u32)> {
        match options.rect {
            Some(ref rect) => {
                let contain_w = f.visible_x.unsigned_abs() + f.visible_width;
                let contain_h = f.visible_y.unsigned_abs() + f.visible_height;
                // Containment is checked against the visible rect below; the
                // containing rect computation above covers the coded frame.
                let crop = Self::validated_crop(rect, contain_w, contain_h)?;
                let (cx_, cy, cw, ch) = crop;
                let in_visible = cx_ >= f.visible_x
                    && cy >= f.visible_y
                    && cx_ + cw as i32 <= f.visible_x + f.visible_width as i32
                    && cy + ch as i32 <= f.visible_y + f.visible_height as i32;
                if !in_visible {
                    return Err(Error::Range(
                        c"rect is not contained in the visible rect".to_owned(),
                    ));
                }
                Ok(crop)
            },
            None => Ok((f.visible_x, f.visible_y, f.visible_width, f.visible_height)),
        }
    }

    /// Compute the per-plane crop copies of `crop` for this frame's format.
    fn compute_copy_planes(
        f: &VideoFrameInner,
        crop: (i32, i32, u32, u32),
    ) -> Fallible<Vec<PlaneCopy>> {
        let geometry = pixel_format_geometry(f.format, f.coded_width, f.coded_height)
            .ok_or(Error::NotSupported(None))?;
        let (rx, ry, rw, rh) = crop;
        let mut copies = Vec::with_capacity(geometry.len());
        for ((geom, plane_bytes), stride) in
            geometry.iter().zip(f.planes.iter()).zip(f.strides.iter())
        {
            let sub_x = geom.hscale;
            let sub_y = geom.vscale;
            // Chroma follows the luma crop with the 4:2:x siting convention
            // (floor the origin, ceil the extent) — same arithmetic as the
            // component's plane_copy_layout.
            let plane_x = rx as u32 / sub_x;
            let plane_y = ry as u32 / sub_y;
            let plane_w = rw.div_ceil(sub_x);
            let plane_h = rh.div_ceil(sub_y);
            let stride = *stride;
            if u64::from(plane_y) * u64::from(stride) >= plane_bytes.len() as u64 {
                return Err(Error::Range(c"crop is outside the frame plane".to_owned()));
            }
            let row_bytes = (plane_w.min(geom.pixels_w) * geom.bytes_per_pixel) as usize;
            let src_offset = plane_y as usize * stride as usize
                + plane_x as usize * geom.bytes_per_pixel as usize;
            let rows = plane_h as usize;
            if src_offset + row_bytes > plane_bytes.len() && rows > 0 {
                return Err(Error::Range(c"crop is outside the frame plane".to_owned()));
            }
            copies.push(PlaneCopy {
                src_offset,
                src_stride: stride as usize,
                row_bytes,
                rows,
            });
        }
        Ok(copies)
    }

    /// Materialize a `VideoFrame` from a plain decoded frame produced by the
    /// decode plane (VideoDecoder gst drain and ImageDecoder RGBA frames —
    /// REQ-BRW-051 output delivery). Decoded frames carry display-sized,
    /// tightly-packed planes (the gst backend crops before delivery).
    pub(crate) fn from_decoded_frame(
        cx: &mut JSContext,
        global: &GlobalScope,
        frame: &DecodedVideoFrame,
    ) -> Fallible<DomRoot<VideoFrame>> {
        let format = match frame.format {
            FrameFormat::I420 => VideoPixelFormat::I420,
            FrameFormat::I422 => VideoPixelFormat::I422,
            FrameFormat::I444 => VideoPixelFormat::I444,
            FrameFormat::Rgba => VideoPixelFormat::RGBA,
            FrameFormat::Bgra => VideoPixelFormat::BGRA,
        };
        let planes: Vec<Rc<Vec<u8>>> = frame
            .planes
            .iter()
            .map(|plane| Rc::new(plane.as_slice().to_vec()))
            .collect();
        let inner = VideoFrameInner {
            format,
            coded_width: frame.display_width,
            coded_height: frame.display_height,
            visible_x: 0,
            visible_y: 0,
            visible_width: frame.display_width,
            visible_height: frame.display_height,
            rotation: 0.0,
            flip: false,
            display_width: frame.display_width,
            display_height: frame.display_height,
            timestamp: frame.timestamp,
            duration: frame.duration.and_then(|d| u64::try_from(d).ok()),
            planes,
            strides: frame.strides.clone(),
        };
        let color_space = VideoColorSpace::from_parts(
            cx,
            global,
            color_space_primaries(&frame.color_space.primaries),
            color_space_transfer(&frame.color_space.transfer),
            color_space_matrix(&frame.color_space.matrix),
            frame.color_space.full_range,
        );
        Ok(reflect_dom_object_with_proto(
            cx,
            Box::new(VideoFrame::new_inherited(
                inner,
                Dom::from_ref(&*color_space),
            )),
            global,
            None,
        ))
    }
}

/// Component `FrameColorSpace` string values → DOM enums (the gst backend
/// emits exactly the registry strings; an unknown value keeps the member
/// absent rather than fabricating a family).
fn color_space_primaries(
    value: &str,
) -> Option<crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoColorPrimaries> {
    use crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoColorPrimaries;
    match value {
        "bt709" => Some(VideoColorPrimaries::Bt709),
        "bt470bg" => Some(VideoColorPrimaries::Bt470bg),
        "smpte170m" => Some(VideoColorPrimaries::Smpte170m),
        "bt2020" => Some(VideoColorPrimaries::Bt2020),
        "smpte432" => Some(VideoColorPrimaries::Smpte432),
        _ => None,
    }
}

fn color_space_transfer(
    value: &str,
) -> Option<
    crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoTransferCharacteristics,
> {
    use crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoTransferCharacteristics;
    match value {
        "bt709" => Some(VideoTransferCharacteristics::Bt709),
        "smpte170m" => Some(VideoTransferCharacteristics::Smpte170m),
        "iec61966-2-1" => Some(VideoTransferCharacteristics::Iec61966_2_1),
        "linear" => Some(VideoTransferCharacteristics::Linear),
        "pq" => Some(VideoTransferCharacteristics::Pq),
        "hlg" => Some(VideoTransferCharacteristics::Hlg),
        _ => None,
    }
}

fn color_space_matrix(
    value: &str,
) -> Option<crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoMatrixCoefficients>
{
    use crate::dom::bindings::codegen::Bindings::VideoColorSpaceBinding::VideoMatrixCoefficients;
    match value {
        "rgb" => Some(VideoMatrixCoefficients::Rgb),
        "bt709" => Some(VideoMatrixCoefficients::Bt709),
        "bt470bg" => Some(VideoMatrixCoefficients::Bt470bg),
        "smpte170m" => Some(VideoMatrixCoefficients::Smpte170m),
        "bt2020-ncl" => Some(VideoMatrixCoefficients::Bt2020_ncl),
        _ => None,
    }
}

// Keep the component color-space type referenced (the from_decoded_frame path
// consumes `FrameColorSpace` through the mapping helpers above).
#[allow(dead_code)]
fn _color_space_witness(_c: FrameColorSpace) {}

impl VideoFrameMethods<crate::DomTypeHolder> for VideoFrame {
    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-videoframe>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        data: ArrayBufferViewOrArrayBuffer,
        init: &VideoFrameBufferInit,
    ) -> Fallible<DomRoot<VideoFrame>> {
        let geometry = pixel_format_geometry(init.format, init.codedWidth, init.codedHeight)
            .ok_or(Error::NotSupported(None))?;
        let no_gc = cx.no_gc();
        let bytes = get_buffer_source_slice(&data, no_gc);
        // Buffer-init step: compute allocationSize from the format table; an
        // undersized buffer is a TypeError.
        let required: u64 = geometry.iter().map(PlaneGeom::plane_size).sum();
        if (bytes.len() as u64) < required {
            return Err(Error::Type(
                c"VideoFrameBufferInit.data is smaller than the format allocation".to_owned(),
            ));
        }
        // Plane bytes: tightly packed by default; an explicit `layout` option
        // supplies per-plane offsets and strides (each plane must fit).
        let mut planes: Vec<Rc<Vec<u8>>> = Vec::with_capacity(geometry.len());
        let mut strides: Vec<u32> = Vec::with_capacity(geometry.len());
        match init.layout {
            None => {
                let mut offset = 0usize;
                for geom in &geometry {
                    let size = geom.plane_size() as usize;
                    planes.push(Rc::new(bytes[offset..offset + size].to_vec()));
                    strides.push(geom.tightly_packed_stride());
                    offset += size;
                }
            },
            Some(ref layout) => {
                if layout.len() < geometry.len() {
                    return Err(Error::Type(
                        c"VideoFrameBufferInit.layout needs one entry per plane".to_owned(),
                    ));
                }
                for (geom, entry) in geometry.iter().zip(layout.iter()) {
                    let offset = entry.offset as usize;
                    let size = u64::from(entry.stride) * u64::from(geom.pixels_h);
                    if offset as u64 + size > bytes.len() as u64 {
                        return Err(Error::Type(
                            c"VideoFrameBufferInit.layout exceeds the provided data".to_owned(),
                        ));
                    }
                    planes.push(Rc::new(bytes[offset..offset + size as usize].to_vec()));
                    strides.push(entry.stride);
                }
            },
        }
        // Visible rect defaults to the full coded frame positioned at (0, 0).
        let (visible_x, visible_y, visible_width, visible_height) = match init.visibleRect {
            Some(ref rect) => Self::validated_crop(rect, init.codedWidth, init.codedHeight)?,
            None => (0, 0, init.codedWidth, init.codedHeight),
        };
        // Default display dimensions match the visible rect.
        let display_width = init.displayWidth.unwrap_or(visible_width);
        let display_height = init.displayHeight.unwrap_or(visible_height);
        let color_space = VideoColorSpace::from_init(
            cx,
            global,
            init.colorSpace.as_ref(),
            is_rgb_format(init.format),
        );
        let frame = VideoFrame::new_inherited(
            VideoFrameInner {
                format: init.format,
                coded_width: init.codedWidth,
                coded_height: init.codedHeight,
                visible_x,
                visible_y,
                visible_width,
                visible_height,
                rotation: *init.rotation,
                flip: init.flip,
                display_width,
                display_height,
                timestamp: init.timestamp,
                duration: init.duration,
                planes,
                strides,
            },
            Dom::from_ref(&*color_space),
        );
        Ok(reflect_dom_object_with_proto(
            cx,
            Box::new(frame),
            global,
            proto,
        ))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-format>
    fn GetFormat(&self) -> Option<VideoPixelFormat> {
        self.inner.borrow().as_ref().map(|f| f.format)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-codedwidth>
    fn CodedWidth(&self) -> u32 {
        self.inner.borrow().as_ref().map_or(0, |f| f.coded_width)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-codedheight>
    fn CodedHeight(&self) -> u32 {
        self.inner.borrow().as_ref().map_or(0, |f| f.coded_height)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-codedrect>
    fn GetCodedRect(
        &self,
    ) -> Option<DomRoot<crate::dom::geometry::domrectreadonly::DOMRectReadOnly>> {
        let f = self.open_inner()?;
        Some(self.new_dom_rect(0, 0, f.coded_width, f.coded_height))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-visiblerect>
    fn GetVisibleRect(
        &self,
    ) -> Option<DomRoot<crate::dom::geometry::domrectreadonly::DOMRectReadOnly>> {
        let f = self.open_inner()?;
        Some(self.new_dom_rect(f.visible_x, f.visible_y, f.visible_width, f.visible_height))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-rotation>
    fn Rotation(&self) -> Finite<f64> {
        Finite::wrap(self.inner.borrow().as_ref().map_or(0.0, |f| f.rotation))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-flip>
    fn Flip(&self) -> bool {
        self.inner.borrow().as_ref().is_some_and(|f| f.flip)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-displaywidth>
    fn DisplayWidth(&self) -> u32 {
        self.inner.borrow().as_ref().map_or(0, |f| f.display_width)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-displayheight>
    fn DisplayHeight(&self) -> u32 {
        self.inner.borrow().as_ref().map_or(0, |f| f.display_height)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-duration>
    fn GetDuration(&self) -> Option<u64> {
        self.inner.borrow().as_ref().and_then(|f| f.duration)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-timestamp>
    fn Timestamp(&self) -> i64 {
        self.inner.borrow().as_ref().map_or(0, |f| f.timestamp)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-colorspace>
    fn ColorSpace(&self) -> DomRoot<VideoColorSpace> {
        DomRoot::from_ref(&*self.color_space)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-metadata>
    /// The members of `VideoFrameMetadata` live in the VideoFrame Metadata
    /// Registry — none are produced by this slice, so the empty dictionary is
    /// the honest shape.
    fn Metadata(&self) -> VideoFrameMetadata {
        VideoFrameMetadata {}
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-allocationsize>
    fn AllocationSize(&self, options: &VideoFrameCopyToOptions) -> Fallible<u32> {
        let f = self.open_inner().ok_or(Error::InvalidState(None))?;
        let crop = self.copy_crop(&f, options)?;
        let copies = Self::compute_copy_planes(&f, crop)?;
        let total: u64 = copies
            .iter()
            .map(|copy| copy.row_bytes as u64 * copy.rows as u64)
            .sum();
        u32::try_from(total).map_err(|_| Error::Range(c"allocation overflows".to_owned()))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-copyto>
    fn CopyTo(
        &self,
        mut destination: ArrayBufferViewOrArrayBuffer,
        options: &VideoFrameCopyToOptions,
    ) -> RootedPromise {
        // The decode plane is synchronous, so validation and the copy run in
        // this turn and the promise settles immediately (registered: no
        // in-flight window, matching the AudioDecoder flush model minus the
        // microtask hop — there is no queued backend work to wait for).
        let global = self.global();
        let mut cx = Self::thread_cx();
        let promise = Promise::new(&mut cx, &global);
        match self.copy_to_sync(&mut destination, options) {
            Ok(layouts) => promise.resolve_native(&mut cx, &layouts),
            Err(error) => promise.reject_error(&mut cx, error),
        }
        promise
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-clone>
    fn Clone(&self) -> Fallible<DomRoot<VideoFrame>> {
        // clone() shares the frame resource: plane bytes are Rc-shared and the
        // color space is a separate immutable DOM object.
        let inner = self.open_inner().ok_or(Error::InvalidState(None))?;
        let global = self.global();
        let mut cx = Self::thread_cx();
        let global: &GlobalScope = &global;
        Ok(reflect_dom_object_with_proto(
            &mut cx,
            Box::new(VideoFrame::new_inherited(
                inner,
                Dom::from_ref(&*self.color_space),
            )),
            global,
            None,
        ))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-videoframe-close>
    fn Close(&self) {
        *self.inner.borrow_mut() = None;
    }
}

impl VideoFrame {
    /// Synchronous body of `copyTo`: validate, copy, and build the reported
    /// layout. `destination` is passed as the owned union because the write
    /// needs `as_mut_slice_safe`.
    fn copy_to_sync(
        &self,
        destination: &mut ArrayBufferViewOrArrayBuffer,
        options: &VideoFrameCopyToOptions,
    ) -> Fallible<Vec<PlaneLayout>> {
        let f = self.open_inner().ok_or(Error::InvalidState(None))?;
        // Registered boundaries (module doc): format conversion, color-space
        // conversion and destination layouts reject with NotSupportedError.
        if options.format.is_some_and(|target| target != f.format) {
            return Err(Error::NotSupported(None));
        }
        if options.colorSpace.is_some() {
            return Err(Error::NotSupported(None));
        }
        if options.layout.is_some() {
            return Err(Error::NotSupported(None));
        }
        let crop = self.copy_crop(&f, options)?;
        let copies = Self::compute_copy_planes(&f, crop)?;
        let total: u64 = copies
            .iter()
            .map(|copy| copy.row_bytes as u64 * copy.rows as u64)
            .sum();
        let cx = Self::thread_cx();
        let destination_slice = match destination {
            ArrayBufferViewOrArrayBuffer::ArrayBufferView(view) => {
                view.as_mut_slice_safe(cx.no_gc())
            },
            ArrayBufferViewOrArrayBuffer::ArrayBuffer(buffer) => {
                buffer.as_mut_slice_safe(cx.no_gc())
            },
        }
        .unwrap_or(&mut []);
        if (destination_slice.len() as u64) < total {
            return Err(Error::Range(c"destination is too small".to_owned()));
        }
        let mut layouts: Vec<PlaneLayout> = Vec::with_capacity(copies.len());
        let mut written = 0usize;
        for (copy, plane_bytes) in copies.iter().zip(f.planes.iter()) {
            for row in 0..copy.rows {
                let src = copy.src_offset + row * copy.src_stride;
                let dst = written + row * copy.row_bytes;
                destination_slice[dst..dst + copy.row_bytes]
                    .copy_from_slice(&plane_bytes[src..src + copy.row_bytes]);
            }
            layouts.push(PlaneLayout {
                offset: written as u32,
                stride: copy.row_bytes as u32,
            });
            written += copy.row_bytes * copy.rows;
        }
        Ok(layouts)
    }
}
