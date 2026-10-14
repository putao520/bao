// @trace REQ-CDP-007 [req:REQ-CDP-007] [criterion:6]
// REQ-CDP-007 criterion 6 "Overlay.highlightNode 高亮节点(截图标注)" — the
// screenshot-annotation highlight drawing face (E1 of the CDP-007 overlay
// contract chain; design /tmp/cdp-007-overlay-design.md §5).
//
// The highlight is a pure embedder-side image annotation: a Chromium-
// DevTools-style four-layer box highlight (painted geometrically outside-in:
// margin → border → padding → content) composited onto the already-rendered
// [`RgbaImage`] before encoding. Nothing here touches the DOM, the servo
// display list or any page-observable state — the stealth invariant is that
// the highlight exists only in the embedder's output buffer (design §2.2).
//
// Geometry contract: the four quads are the axis-aligned boxes the
// DOM.getBoxModel face produces (`bao_cdp` devtools_dom::box_model_body —
// getBoundingClientRect + computed border/padding/margin widths), in
// viewport coordinates == the screenshot coordinate system. There is no
// additional transform here: `Page.captureScreenshot` captures the current
// viewport, and the screencast downscale runs AFTER this draw (design §4/§9
// — the draw point is pinned to "original-resolution RgbaImage, before any
// transform").
//
// Raster rules (deterministic, integer math — every pixel value below is
// hand-verifiable, and the unit tests pin exact blended numbers):
//   * Rect bounds round to the integer pixel grid (half away from zero);
//     right/bottom edges are exclusive.
//   * Zero-area layers (rounded `w<=0` or `h<=0`, which includes the
//     degenerate zero-width / zero-height and negative boxes box_model_body
//     can produce when border+padding exceed the rect) draw NOTHING — no
//     fill and no cosmetic outline line. Registered deviation from Chrome's
//     inspector, which draws a 1px line for zero-size boxes; cosmetic-only,
//     and skipping it keeps the raster total (no half-drawn-line cases).
//   * A fully transparent layer (`a == 0`) is a no-op for both fill and
//     stroke — an invisible layer must not paint an opaque outline.
//   * Layers clip to the image bounds; out-of-bounds quads never panic.
//   * Fills blend straight-alpha source-over in u32 integer math with
//     round-to-nearest on every division (see [`blend_over`]).
//   * Each layer additionally strokes its (clipped) boundary ring with its
//     own RGB at FULL opacity — the "亮色描边" of design §5 (the fill is the
//     translucent reading of the color, the ring is its solid reading).

use image::{Rgba, RgbaImage};

/// One axis-aligned box in viewport (== screenshot) pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl OverlayQuad {
    /// The zero-area quad — layer-absent placeholder (draws nothing).
    pub const EMPTY: OverlayQuad = OverlayQuad {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };
}

/// The four nested boxes of a highlighted element (content ⊆ padding ⊆
/// border ⊆ margin for positive widths; degenerate / negative boxes are
/// possible and are skipped by the raster).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxQuads {
    pub content: OverlayQuad,
    pub padding: OverlayQuad,
    pub border: OverlayQuad,
    pub margin: OverlayQuad,
}

/// A highlight layer color — the CDP `DOM.RGBA` shape, channels in
/// `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlightColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl HighlightColor {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From 0-255 channel values (the DevTools palette notation) + alpha in
    /// `0.0..=1.0`.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }
}

/// Per-layer colors of one highlight — the CDP `Overlay.HighlightSpec`
/// shape (contentColor / paddingColor / borderColor / marginColor, all
/// accepted; design §3.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlightSpec {
    pub content: HighlightColor,
    pub padding: HighlightColor,
    pub border: HighlightColor,
    pub margin: HighlightColor,
}

impl Default for HighlightSpec {
    /// The Chromium DevTools inspector default palette (OverlayModel
    /// defaults): content blue, padding green, border yellow, margin orange.
    fn default() -> Self {
        Self {
            content: HighlightColor::from_rgba8(111, 168, 220, 0.66),
            padding: HighlightColor::from_rgba8(147, 196, 125, 0.55),
            border: HighlightColor::from_rgba8(255, 229, 153, 0.66),
            margin: HighlightColor::from_rgba8(246, 178, 107, 0.66),
        }
    }
}

/// Which node a highlight is attached to — the same three-form node identity
/// the CDP `Overlay.highlightNode` params carry (nodeId | backendNodeId |
/// objectId), mirroring the `bao_cdp` devtools_dom `NodeRef` shape so the
/// bridge layer (E2/E3) can hand the reference across without minting a new
/// identity space (design §4).
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayNodeRef {
    NodeId(i64),
    BackendNodeId(i64),
    ObjectId(String),
}

/// Per-target highlight state — the single highlight slot stored on the
/// page's webview state (design §3.1): one highlight at a time, replaced by
/// each `Overlay.highlightNode`, cleared by `Overlay.hideHighlight`, and
/// dropped with the page state on navigation (node ids do not survive a
/// document change — the same lifecycle as the DOM-face nodeId identity).
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightState {
    pub node: OverlayNodeRef,
    pub spec: HighlightSpec,
}

/// Draw one node highlight onto `image` (design §5): the four layers painted
/// geometrically outside-in (margin → border → padding → content — CSS box
/// nesting; see the paint-order note in the body), each as a translucent
/// fill blended over whatever is beneath plus a fully-opaque 1px stroke of
/// its own color on the layer's boundary ring.
///
/// Pure function: no IO, no engine state, and it never panics on
/// out-of-bounds or degenerate geometry.
///
/// @trace REQ-CDP-007 [req:REQ-CDP-007] [criterion:6]
pub fn draw_highlight(image: &mut RgbaImage, quads: &BoxQuads, spec: &HighlightSpec) {
    // Outside-in paint order (design §5 由外向内): each later fill covers the
    // inner region of the earlier ones, so every layer shows as a ring around
    // the next-inner one — the DevTools look. GEOMETRIC nesting is
    // margin ⊇ border ⊇ padding ⊇ content (CSS box model), so the border box
    // is painted before the padding box — the design doc's layer NAME list
    // ("margin, padding, border, content") is not the paint order: painting
    // padding before border would let border's fill cover padding's whole
    // rect (padding ⊆ border) and the padding color would never be visible.
    // Same order Chromium's InspectorOverlay paints its four quads.
    let layers = [
        (quads.margin, spec.margin),
        (quads.border, spec.border),
        (quads.padding, spec.padding),
        (quads.content, spec.content),
    ];
    for (quad, color) in layers {
        draw_layer(image, quad, color);
    }
}

/// Paint one highlight layer: opaque 1px boundary ring + translucent fill,
/// clipped to the image. Zero-area and fully-transparent layers are no-ops
/// (see the module docs for the registered raster rules).
fn draw_layer(image: &mut RgbaImage, quad: OverlayQuad, color: HighlightColor) {
    let fill = to_fill(color);
    if fill[3] == 0 {
        // Fully transparent layer: neither its fill nor its stroke may paint.
        return;
    }
    let (x0, y0, x1, y1) = rounded_bounds(quad);
    if x1 <= x0 || y1 <= y0 {
        // Zero/negative-area layer: no fill, no cosmetic outline line.
        return;
    }
    let stroke = to_stroke(color);
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (ix0, iy0) = (x0.max(0), y0.max(0));
    let (ix1, iy1) = (x1.min(w), y1.min(h));
    if ix1 <= ix0 || iy1 <= iy0 {
        // Clipped out entirely.
        return;
    }
    for y in iy0..iy1 {
        for x in ix0..ix1 {
            let on_ring = x == ix0 || x == ix1 - 1 || y == iy0 || y == iy1 - 1;
            if on_ring {
                *image.get_pixel_mut(x as u32, y as u32) = stroke;
            } else {
                let px = image.get_pixel_mut(x as u32, y as u32);
                *px = blend_over(*px, fill);
            }
        }
    }
}

/// Rect bounds rounded to the integer pixel grid (half away from zero),
/// right/bottom exclusive.
fn rounded_bounds(quad: OverlayQuad) -> (i64, i64, i64, i64) {
    (
        quad.x.round() as i64,
        quad.y.round() as i64,
        (quad.x + quad.w).round() as i64,
        (quad.y + quad.h).round() as i64,
    )
}

/// CDP float color → straight-alpha u8 fill (channels clamp to `0..=1` then
/// round half away from zero).
fn to_fill(color: HighlightColor) -> Rgba<u8> {
    Rgba([
        to_channel_u8(color.r),
        to_channel_u8(color.g),
        to_channel_u8(color.b),
        to_channel_u8(color.a),
    ])
}

/// The layer's stroke color: same RGB at full opacity (design §5 亮色描边).
fn to_stroke(color: HighlightColor) -> Rgba<u8> {
    Rgba([
        to_channel_u8(color.r),
        to_channel_u8(color.g),
        to_channel_u8(color.b),
        255,
    ])
}

fn to_channel_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Straight-alpha source-over: `src` blended over `dst` in u32 integer math
/// with round-to-nearest on every division — deterministic and hand-checkable
/// (the unit tests pin exact numbers):
///
/// ```text
/// out_a = sa + round(da·(255-sa) / 255)
/// out_c = round((255·s·sa + d·da·(255-sa)) / (255·sa + da·(255-sa)))
/// ```
fn blend_over(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let sa = src[3] as u32;
    if sa == 0 {
        return dst;
    }
    if sa == 255 {
        return src;
    }
    let da = dst[3] as u32;
    let inv = 255 - sa;
    let alpha = sa + div_round(da * inv, 255);
    let den = 255 * sa + da * inv;
    let ch = |s: u8, d: u8| div_round(255 * s as u32 * sa + d as u32 * da * inv, den) as u8;
    Rgba([
        ch(src[0], dst[0]),
        ch(src[1], dst[1]),
        ch(src[2], dst[2]),
        alpha as u8,
    ])
}

/// Integer division rounded to nearest (ties toward +infinity).
fn div_round(n: u32, d: u32) -> u32 {
    (n + d / 2) / d
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn solid(w: u32, h: u32, px: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(px))
    }

    fn q(x: f32, y: f32, w: f32, h: f32) -> OverlayQuad {
        OverlayQuad { x, y, w, h }
    }

    /// Only the content layer carries geometry; the rest are zero-area
    /// no-op placeholders.
    fn quads(content: OverlayQuad) -> BoxQuads {
        BoxQuads {
            content,
            padding: OverlayQuad::EMPTY,
            border: OverlayQuad::EMPTY,
            margin: OverlayQuad::EMPTY,
        }
    }

    fn content_spec(color: HighlightColor) -> HighlightSpec {
        let clear = HighlightColor::new(0.0, 0.0, 0.0, 0.0);
        HighlightSpec {
            content: color,
            padding: clear,
            border: clear,
            margin: clear,
        }
    }

    fn green_opaque() -> HighlightColor {
        HighlightColor::new(0.0, 1.0, 0.0, 1.0)
    }

    #[test]
    fn four_layers_stack_outside_in() {
        let mut image = solid(40, 40, WHITE);
        let spec = HighlightSpec {
            content: HighlightColor::new(0.0, 0.0, 0.0, 1.0),
            padding: HighlightColor::new(0.0, 0.0, 1.0, 1.0),
            border: HighlightColor::new(0.0, 1.0, 0.0, 1.0),
            margin: HighlightColor::new(1.0, 0.0, 0.0, 1.0),
        };
        let quads = BoxQuads {
            margin: q(0.0, 0.0, 40.0, 40.0),
            border: q(5.0, 5.0, 30.0, 30.0),
            padding: q(10.0, 10.0, 20.0, 20.0),
            content: q(15.0, 15.0, 10.0, 10.0),
        };
        draw_highlight(&mut image, &quads, &spec);
        // Margin ring (outside every other box) keeps the margin color.
        assert_eq!(image.get_pixel(2, 2), &Rgba([255, 0, 0, 255]));
        // Border ring shows border drawn OVER margin.
        assert_eq!(image.get_pixel(7, 7), &Rgba([0, 255, 0, 255]));
        // Padding ring over border.
        assert_eq!(image.get_pixel(12, 12), &Rgba([0, 0, 255, 255]));
        // Content interior is content — proving content is painted LAST
        // (any other order would leave margin/border/padding color here).
        assert_eq!(image.get_pixel(18, 18), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn translucent_layers_blend_with_exact_integer_math() {
        let mut image = solid(4, 4, [10, 20, 30, 255]);
        let spec = HighlightSpec {
            content: HighlightColor::new(1.0, 0.0, 0.0, 0.5),
            padding: HighlightColor::new(0.0, 0.0, 1.0, 0.5),
            border: HighlightColor::new(0.0, 0.0, 0.0, 0.0),
            margin: HighlightColor::new(0.0, 0.0, 0.0, 0.0),
        };
        let quads = BoxQuads {
            margin: OverlayQuad::EMPTY,
            border: OverlayQuad::EMPTY,
            padding: q(0.0, 0.0, 4.0, 4.0),
            content: q(0.0, 0.0, 4.0, 4.0),
        };
        draw_highlight(&mut image, &quads, &spec);
        // Interior (1,1): padding rgba(0,0,255,.5) over bg, then content
        // rgba(255,0,0,.5) over that — integer source-over step by step:
        //   [10,20,30] →(blue .5) [5,10,143] →(red .5) [130,5,71]
        assert_eq!(image.get_pixel(1, 1), &Rgba([130, 5, 71, 255]));
        // Boundary (0,0): the last-painted layer's opaque stroke.
        assert_eq!(image.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn quad_partially_out_of_bounds_clips_to_image() {
        let mut image = solid(4, 4, WHITE);
        draw_highlight(
            &mut image,
            &quads(q(2.0, 2.0, 10.0, 10.0)),
            &content_spec(green_opaque()),
        );
        // Visible part is [2..4) × [2..4) only.
        assert_eq!(image.get_pixel(3, 3), &Rgba([0, 255, 0, 255]));
        assert_eq!(image.get_pixel(1, 1), &Rgba(WHITE));
    }

    #[test]
    fn quad_fully_outside_image_draws_nothing() {
        let mut image = solid(4, 4, WHITE);
        draw_highlight(
            &mut image,
            &quads(q(-20.0, -20.0, 10.0, 10.0)),
            &content_spec(green_opaque()),
        );
        assert!(image.pixels().all(|p| p == &Rgba(WHITE)));
    }

    #[test]
    fn quad_crossing_image_origin_clips_to_visible_part() {
        let mut image = solid(4, 4, WHITE);
        draw_highlight(
            &mut image,
            &quads(q(-2.0, -2.0, 4.0, 4.0)),
            &content_spec(green_opaque()),
        );
        // Only [0..2) × [0..2) is inside the image.
        assert_eq!(image.get_pixel(0, 0), &Rgba([0, 255, 0, 255]));
        assert_eq!(image.get_pixel(2, 2), &Rgba(WHITE));
    }

    #[test]
    fn oversized_quad_clips_to_whole_image() {
        let mut image = solid(4, 4, WHITE);
        draw_highlight(
            &mut image,
            &quads(q(0.0, 0.0, 100.0, 100.0)),
            &content_spec(green_opaque()),
        );
        assert_eq!(image.get_pixel(0, 0), &Rgba([0, 255, 0, 255]));
        assert_eq!(image.get_pixel(3, 3), &Rgba([0, 255, 0, 255]));
    }

    #[test]
    fn zero_area_quad_variants_draw_nothing() {
        // The four degenerate shapes box_model_body geometry can produce:
        // ① zero-area point, ② zero-width, ③ zero-height, ④ negative size
        // (border+padding wider than the rect). All are no-ops — no fill and
        // no cosmetic outline — and none of them panic.
        let cases = [
            ("zero-area point", q(2.0, 2.0, 0.0, 0.0)),
            ("zero-width", q(2.0, 2.0, 0.0, 2.0)),
            ("zero-height", q(2.0, 2.0, 2.0, 0.0)),
            ("negative size", q(2.0, 2.0, -4.0, -4.0)),
        ];
        for (name, quad) in cases {
            let mut image = solid(4, 4, WHITE);
            draw_highlight(&mut image, &quads(quad), &content_spec(green_opaque()));
            assert!(
                image.pixels().all(|p| p == &Rgba(WHITE)),
                "{name} quad must draw nothing"
            );
        }
    }

    #[test]
    fn fully_transparent_layer_draws_nothing() {
        let mut image = solid(4, 4, WHITE);
        draw_highlight(
            &mut image,
            &quads(q(0.0, 0.0, 4.0, 4.0)),
            // a == 0: even the opaque boundary stroke must not appear.
            &content_spec(HighlightColor::new(1.0, 0.0, 0.0, 0.0)),
        );
        assert!(image.pixels().all(|p| p == &Rgba(WHITE)));
    }

    #[test]
    fn default_spec_is_the_devtools_palette() {
        let spec = HighlightSpec::default();
        let assert_close = |c: HighlightColor, r: u8, g: u8, b: u8, a: f32| {
            let close = |v: f32, want: f32| assert!((v - want).abs() < 1e-4, "{v} != {want}");
            close(c.r, r as f32 / 255.0);
            close(c.g, g as f32 / 255.0);
            close(c.b, b as f32 / 255.0);
            close(c.a, a);
        };
        // Chromium DevTools inspector defaults (OverlayModel):
        assert_close(spec.content, 111, 168, 220, 0.66);
        assert_close(spec.padding, 147, 196, 125, 0.55);
        assert_close(spec.border, 255, 229, 153, 0.66);
        assert_close(spec.margin, 246, 178, 107, 0.66);
    }

    #[test]
    fn highlight_state_holds_node_ref_and_spec() {
        let state = HighlightState {
            node: OverlayNodeRef::NodeId(42),
            spec: HighlightSpec::default(),
        };
        assert_eq!(state.node, OverlayNodeRef::NodeId(42));
        assert_eq!(state.spec, HighlightSpec::default());
        // The other two CDP node-identity forms construct and distinguish.
        assert_ne!(state.node, OverlayNodeRef::BackendNodeId(42));
        let object = OverlayNodeRef::ObjectId("1:3".to_string());
        assert!(matches!(object, OverlayNodeRef::ObjectId(_)));
    }
}
