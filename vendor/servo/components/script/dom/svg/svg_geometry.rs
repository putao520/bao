/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// BAO patch (fork-maintained, 2026-09-27): REQ-BRW-046 — SVG DOM geometry core.
//
// Upstream servo declares the SVG geometry/reflection surface (getBBox / getCTM /
// getScreenCTM / getTotalLength / getPointAtLength) but implements none of it; this
// module provides the real computation behind those DOM methods, derived purely from
// SVG geometry attributes (no layout-thread dependency).
//
// Library reuse (implementation-selection ladder: mature library first —
// PRD-DEC-UPSTREAM-STRATEGY-V2 ⑤; user ruling 2026-09-27):
//   * `kurbo` — path `d` parsing (`BezPath::from_svg`: full SVG path grammar incl.
//     elliptical arcs→cubics and smooth-command reflection), adaptive arc-length
//     integration (`ParamCurveArclen::arclen` / `Shape::perimeter`), control-point
//     bounding boxes (`Shape::bounding_box`), and all transform-matrix construction
//     and composition (`Affine`).
//   * `svgtypes` — SVG attribute tokenization: `TransformListParser` (transform
//     lists, incl. spec-mandated rotate(cx,cy) splitting), `PointsParser`
//     (polyline/polygon point lists), `Number` (single number attributes).
//   * `euclid` — 3D embedding for `DOMMatrix` output.
// Only thin glue is written here: geometry-attribute→kurbo-type conversion, the
// arc-length→path-parameter inversion (bisection over `ParamCurve::subsegment` +
// `arclen`, since kurbo ships no point-at-length API), and the Affine→euclid
// embedding. No Bézier math, path tokenizing, or matrix algebra lives in this file.
//
// v1 boundaries (documented, not stubs): percentages resolve to defaults (0), the
// stroke/markers/clipped expansion of `SVGBoundingBoxOptions` is not applied (the
// fill-geometry bounding box is returned), and text metrics are 0-sized at (x, y).

use euclid::default::Transform3D;
use html5ever::{LocalName, local_name};
use js::context::JSContext;
use kurbo::{Affine, BezPath, Circle, Ellipse, Line as KurboLine, ParamCurve, ParamCurveArclen, PathSeg, Point as KurboPoint, Rect as KurboRect, Shape, Vec2 as KurboVec2};

use crate::dom::bindings::inheritance::Castable;
use crate::dom::element::Element;
use crate::dom::node::Node;
use crate::dom::svg::svgcircleelement::SVGCircleElement;
use crate::dom::svg::svgellipseelement::SVGEllipseElement;
use crate::dom::svg::svgimageelement::SVGImageElement;
use crate::dom::svg::svglineelement::SVGLineElement;
use crate::dom::svg::svgpathelement::SVGPathElement;
use crate::dom::svg::svgpolygonelement::SVGPolygonElement;
use crate::dom::svg::svgpolylineelement::SVGPolylineElement;
use crate::dom::svg::svgrectelement::SVGRectElement;
use crate::dom::svg::svgsvgelement::SVGSVGElement;
use crate::dom::svg::svgtextelement::SVGTextElement;
use crate::dom::svg::svgtspanelement::SVGTSpanElement;

/// Accuracy (user units) passed to kurbo's adaptive arc-length integration
/// and its `inv_arclen` parameter inversion.
const ARCLENS_ACCURACY: f64 = 0.01;

/// Relative fitting tolerance when converting exact shapes (circles/ellipses)
/// to Bézier paths for length/point queries, relative to the shape's size hint.
const CURVE_FIT_TOLERANCE: f64 = 1e-4;

/// The calling thread's JSContext, for DOM-object construction inside methods
/// whose generated (fork codegen) signature carries no explicit `cx`.
///
/// DOM methods execute synchronously on the script thread; mozjs documents
/// `JSContext::get_from_thread` as "Get the `JSContext` for this thread" and
/// the returned context is the very one the codegen caller dispatched through.
/// The runtime outlives the call, so the context is valid for its duration.
#[expect(unsafe_code)]
pub(crate) fn current_cx() -> JSContext {
    // SAFETY: see above — script-thread DOM call, runtime outlives the call.
    unsafe { JSContext::get_from_thread() }.expect("JSContext must exist on the script thread")
}

// ── SVG transform attribute (svgtypes tokenization → kurbo Affine) ────────────

/// The element's own `transform` attribute, or identity when absent/invalid.
pub(crate) fn local_transform(element: &Element) -> Affine {
    element
        .get_attribute_string_value(&local_name!("transform"))
        .and_then(|value| parse_transform_attribute(&value))
        .unwrap_or(Affine::IDENTITY)
}

/// Parse an SVG `transform` attribute value into a single composed matrix via
/// svgtypes' `TransformListParser`. Returns `None` when the value is not a
/// valid transform list, in which case the attribute is ignored (identity),
/// per the SVG grammar.
fn parse_transform_attribute(value: &str) -> Option<Affine> {
    let mut matrix = Affine::IDENTITY;
    for token in svgtypes::TransformListParser::from(value) {
        // SVG list semantics: the leftmost transform is applied last, i.e.
        // `A B` ≡ A·B with B acting on user-space points first. kurbo's `Mul`
        // composes with the same right-operand-first convention.
        matrix = matrix * transform_token_to_affine(token.ok()?);
    }
    // An empty (but valid) list is the identity; any token failure above
    // already yielded `None`.
    Some(matrix)
}

/// Map one svgtypes transform token to a kurbo affine (skew takes tangent
/// values; svgtypes angles are degrees).
fn transform_token_to_affine(token: svgtypes::TransformListToken) -> Affine {
    use svgtypes::TransformListToken as Token;
    match token {
        Token::Matrix { a, b, c, d, e, f } => Affine::new([a, b, c, d, e, f]),
        Token::Translate { tx, ty } => Affine::translate(KurboVec2::new(tx, ty)),
        Token::Scale { sx, sy } => Affine::scale_non_uniform(sx, sy),
        Token::Rotate { angle } => Affine::rotate(angle.to_radians()),
        Token::SkewX { angle } => Affine::skew(angle.to_radians().tan(), 0.0),
        Token::SkewY { angle } => Affine::skew(0.0, angle.to_radians().tan()),
    }
}

/// The matrix from the element's user coordinate system to the coordinate
/// system of its nearest SVG viewport (the nearest `svg` ancestor), i.e. the
/// getCTM product: the element's own `transform` followed by every ancestor
/// `transform` up to — excluding — the nearest `svg` element. An `svg` element
/// itself yields identity (viewport-level viewBox/x/y mapping is a v1 boundary).
pub(crate) fn local_to_viewport_matrix(element: &Element) -> Affine {
    let mut matrix = Affine::IDENTITY;
    if !element.is::<SVGSVGElement>() {
        matrix = local_transform(element);
    }
    let node = element.upcast::<Node>();
    for ancestor in node.ancestors() {
        let Some(ancestor_element) = ancestor.downcast::<Element>() else {
            break;
        };
        if ancestor_element.is::<SVGSVGElement>() {
            break;
        }
        matrix = local_transform(ancestor_element) * matrix;
    }
    matrix
}

/// Embed an affine into a 3D matrix for DOMMatrix construction.
pub(crate) fn affine_to_transform_3d(affine: Affine) -> Transform3D<f64> {
    let [a, b, c, d, e, f] = affine.as_coeffs();
    euclid::default::Transform2D::new(a, b, c, d, e, f).to_3d()
}

// ── SVG attribute values → numbers / point lists (svgtypes) ───────────────────

/// Read a single-number SVG attribute, falling back to the provided default.
/// Percentages and other suffixed values have no v1 viewport context and parse
/// as failures, which callers turn into the attribute's default.
fn parse_svg_number(value: &str) -> Option<f64> {
    value.trim().parse::<svgtypes::Number>().ok().map(|n| n.0)
}

/// The first number of an SVG number-list attribute (text `x`/`y` are lists).
fn first_number_attribute(element: &Element, name: &'static str) -> f64 {
    element
        .get_attribute_string_value(&LocalName::from(name))
        .and_then(|value| {
            value
                .split(|c: char| c == ',' || c.is_whitespace())
                .find(|token| !token.is_empty())
                .and_then(|token| token.parse::<svgtypes::Number>().ok())
                .map(|n| n.0)
        })
        .unwrap_or(0.0)
}

fn points_attribute(element: &Element) -> Vec<(f64, f64)> {
    element
        .get_attribute_string_value(&local_name!("points"))
        .map(|value| svgtypes::PointsParser::from(value.as_str()).collect())
        .unwrap_or_default()
}

/// Read a single-number SVG attribute, falling back to the provided default.
fn number_attribute(element: &Element, name: &'static str, default: f64) -> f64 {
    element
        .get_attribute_string_value(&LocalName::from(name))
        .and_then(|value| parse_svg_number(&value))
        .unwrap_or(default)
}

/// A size-like attribute: negative values make the shape invalid (not
/// rendered), which contributes a zero-size extent.
fn size_attribute(element: &Element, name: &'static str) -> f64 {
    number_attribute(element, name, 0.0).max(0.0)
}

// ── Geometry shapes → kurbo types ─────────────────────────────────────────────

/// Fitting tolerance for exact-shape→Bézier conversion, scaled to the shape.
fn curve_fit_tolerance(size_hint: f64) -> f64 {
    size_hint.max(1.0) * CURVE_FIT_TOLERANCE
}

fn circle_radius(element: &Element) -> f64 {
    size_attribute(element, "r")
}

/// The bounding box of an element in its own user space (getBBox value).
/// Own-transform is intentionally excluded: getBBox is in the element's own
/// user coordinate system. Geometry shapes use kurbo's exact shape bounding
/// boxes; paths use kurbo's control-point bounding box of `d` (sanctioned by
/// the v1 contract).
pub(crate) fn geometry_bbox(element: &Element) -> KurboRect {
    if let Some(shape) = geometry_shape_bbox(element) {
        return shape;
    }
    if let Some(image) = element.downcast::<SVGImageElement>() {
        return attribute_rect(image.upcast());
    }
    // Text metrics require layout (v1 boundary): anchor at (x, y), zero size.
    if element.is::<SVGTextElement>() || element.is::<SVGTSpanElement>() {
        let x = first_number_attribute(element, "x");
        let y = first_number_attribute(element, "y");
        return KurboRect::new(x, y, x, y);
    }
    // Container graphics elements (g, svg, symbol, defs, use, a, …): union of
    // children, each child's box mapped through the child's own transform.
    container_bbox(element)
}

/// Bounding boxes of the closed set of SVGGeometryElement shapes.
fn geometry_shape_bbox(element: &Element) -> Option<KurboRect> {
    if let Some(circle) = element.downcast::<SVGCircleElement>() {
        let upcast = circle.upcast();
        let circle = Circle::new(
            KurboPoint::new(
                number_attribute(upcast, "cx", 0.0),
                number_attribute(upcast, "cy", 0.0),
            ),
            circle_radius(upcast),
        );
        return Some(circle.bounding_box());
    }
    if let Some(ellipse) = element.downcast::<SVGEllipseElement>() {
        let upcast = ellipse.upcast();
        let ellipse = Ellipse::new(
            KurboPoint::new(
                number_attribute(upcast, "cx", 0.0),
                number_attribute(upcast, "cy", 0.0),
            ),
            KurboVec2::new(
                size_attribute(upcast, "rx"),
                size_attribute(upcast, "ry"),
            ),
            0.0,
        );
        return Some(ellipse.bounding_box());
    }
    if let Some(rect) = element.downcast::<SVGRectElement>() {
        return Some(attribute_rect(rect.upcast()));
    }
    if let Some(line) = element.downcast::<SVGLineElement>() {
        let upcast = line.upcast();
        let (start, end) = shape_line_endpoints(upcast);
        let line = KurboLine::new(start, end);
        return Some(line.bounding_box());
    }
    if element.is::<SVGPolygonElement>() || element.is::<SVGPolylineElement>() {
        let closed = element.is::<SVGPolygonElement>();
        let points = points_attribute(element);
        return Some(point_list_bez_path(&points, closed).bounding_box());
    }
    if element.is::<SVGPathElement>() {
        return Some(path_bez_path(element).bounding_box());
    }
    None
}

fn attribute_rect(element: &Element) -> KurboRect {
    let x = number_attribute(element, "x", 0.0);
    let y = number_attribute(element, "y", 0.0);
    KurboRect::new(
        x,
        y,
        x + size_attribute(element, "width"),
        y + size_attribute(element, "height"),
    )
}

fn shape_line_endpoints(element: &Element) -> (KurboPoint, KurboPoint) {
    (
        KurboPoint::new(
            number_attribute(element, "x1", 0.0),
            number_attribute(element, "y1", 0.0),
        ),
        KurboPoint::new(
            number_attribute(element, "x2", 0.0),
            number_attribute(element, "y2", 0.0),
        ),
    )
}

/// Build a kurbo path from a polyline/polygon point list (`closed` appends
/// the closing edge for polygons).
fn point_list_bez_path(points: &[(f64, f64)], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    let Some((x, y)) = points.first() else {
        return path;
    };
    path.move_to(KurboPoint::new(*x, *y));
    for (x, y) in points.iter().skip(1) {
        path.line_to(KurboPoint::new(*x, *y));
    }
    if closed {
        path.close_path();
    }
    path
}

/// Parse the path `d` attribute with kurbo's SVG path parser. Returns an
/// empty path for invalid data (an invalid path is not rendered).
fn path_bez_path(element: &Element) -> BezPath {
    element
        .get_attribute_string_value(&local_name!("d"))
        .and_then(|d| BezPath::from_svg(&d).ok())
        .unwrap_or_default()
}

/// Map a box through an affine matrix (all four corners, re-normalized).
fn apply_matrix(box_: KurboRect, matrix: Affine) -> KurboRect {
    let p0 = matrix * KurboPoint::new(box_.x0, box_.y0);
    let p1 = matrix * KurboPoint::new(box_.x1, box_.y1);
    KurboRect::from_points(p0, p1)
}

fn container_bbox(element: &Element) -> KurboRect {
    let node = element.upcast::<Node>();
    let mut bbox: Option<KurboRect> = None;
    for child in node.child_elements() {
        let child_box = apply_matrix(geometry_bbox(&child), local_transform(&child));
        bbox = match bbox {
            Some(current) => Some(current.union(child_box)),
            None => Some(child_box),
        };
    }
    bbox.unwrap_or(KurboRect::ZERO)
}

// ── Arc-length table (the sanctioned glue over kurbo primitives) ──────────────

/// Cumulative arc lengths of a path's segments. Total length is kurbo's
/// adaptive integration; point-at-length inverts it per segment by bisection
/// over `ParamCurve::subsegment` + `arclen` (kurbo ships no point-at-length
/// API, so this thin inversion layer is written here).
struct SegmentLengthTable {
    segments: Vec<PathSeg>,
    cumulative_ends: Vec<f64>,
    total: f64,
}

impl SegmentLengthTable {
    fn from_path(path: &BezPath) -> Self {
        let mut segments = Vec::new();
        let mut cumulative_ends = Vec::new();
        let mut total = 0.0;
        for segment in path.segments() {
            total += segment.arclen(ARCLENS_ACCURACY);
            cumulative_ends.push(total);
            segments.push(segment);
        }
        SegmentLengthTable {
            segments,
            cumulative_ends,
            total,
        }
    }

    /// The point at arc length `distance` (clamped to [0, total]).
    fn point_at(&self, distance: f64) -> KurboPoint {
        if self.segments.is_empty() {
            return KurboPoint::ZERO;
        }
        let distance = distance.clamp(0.0, self.total);
        // Segment spans are contiguous and monotonically increasing, so the
        // first segment whose end reaches the clamped distance brackets it.
        let index = self
            .cumulative_ends
            .partition_point(|&end| end < distance)
            .min(self.segments.len() - 1);
        let segment = self.segments[index];
        let span_start = if index == 0 {
            0.0
        } else {
            self.cumulative_ends[index - 1]
        };
        let span = self.cumulative_ends[index] - span_start;
        let offset = (distance - span_start).clamp(0.0, span);
        if span <= 0.0 {
            return segment.eval(0.0);
        }
        segment.eval(segment.inv_arclen(offset, ARCLENS_ACCURACY))
    }
}

// ── Length / point-at-length per geometry shape ───────────────────────────────

/// `getTotalLength` value for any SVGGeometryElement shape.
pub(crate) fn total_length(element: &Element) -> f64 {
    length_table(element)
        .map(|table| table.total)
        .unwrap_or(0.0)
}

/// `getPointAtLength` for any SVGGeometryElement shape.
pub(crate) fn point_at_length(element: &Element, distance: f64) -> (f64, f64) {
    match length_table(element) {
        Some(table) => {
            let point = table.point_at(distance);
            (point.x, point.y)
        },
        None => (0.0, 0.0),
    }
}

/// The cumulative-length table of an element's shape path; `None` when the
/// element is not one of the closed SVGGeometryElement shapes.
fn length_table(element: &Element) -> Option<SegmentLengthTable> {
    if let Some(path) = element.downcast::<SVGPathElement>() {
        return Some(SegmentLengthTable::from_path(&path_bez_path(path.upcast())));
    }
    if let Some(circle) = element.downcast::<SVGCircleElement>() {
        let r = circle_radius(circle.upcast());
        let center = shape_center(circle.upcast());
        let path = Circle::new(center, r).to_path(curve_fit_tolerance(r));
        return Some(SegmentLengthTable::from_path(&path));
    }
    if let Some(ellipse) = element.downcast::<SVGEllipseElement>() {
        let upcast = ellipse.upcast();
        let (rx, ry) = (
            size_attribute(upcast, "rx"),
            size_attribute(upcast, "ry"),
        );
        let ellipse = Ellipse::new(shape_center(upcast), KurboVec2::new(rx, ry), 0.0);
        let path = ellipse.to_path(curve_fit_tolerance(rx.max(ry)));
        return Some(SegmentLengthTable::from_path(&path));
    }
    if let Some(rect) = element.downcast::<SVGRectElement>() {
        return Some(SegmentLengthTable::from_path(
            &attribute_rect(rect.upcast()).to_path(0.0),
        ));
    }
    if let Some(line) = element.downcast::<SVGLineElement>() {
        let (start, end) = shape_line_endpoints(line.upcast());
        let mut path = BezPath::new();
        path.move_to(start);
        path.line_to(end);
        return Some(SegmentLengthTable::from_path(&path));
    }
    if element.is::<SVGPolygonElement>() || element.is::<SVGPolylineElement>() {
        let closed = element.is::<SVGPolygonElement>();
        return Some(SegmentLengthTable::from_path(&point_list_bez_path(
            &points_attribute(element),
            closed,
        )));
    }
    None
}

fn shape_center(element: &Element) -> KurboPoint {
    KurboPoint::new(
        number_attribute(element, "cx", 0.0),
        number_attribute(element, "cy", 0.0),
    )
}
