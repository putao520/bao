/* @trace REQ-BRW-051 (C2: motion 族 94 格,offset-path/offset-anchor/offset-distance;
 * groundedIn→PRD REQ-BRW-051 兼容性 oracle 战役) */

//! CSS Motion Path Level 1 (<https://drafts.fxtf.org/motion-1/>) — the offset
//! transform engine.
//!
//! Reads `offset-path` from computed style and produces the paint-time
//! reference-frame transform that places the element's anchor point on the
//! path at `offset-distance`, rotated by `offset-rotate`.
//!
//! ## Value-layer contract (stylo)
//!
//! Of the motion family only `offset-path` is declared for the Servo engine
//! today (`stylo/properties/longhands.toml` leaves `engine = "gecko"` off it;
//! parsing is additionally gated behind the `layout.unimplemented`
//! preference). The sibling properties — `offset-distance`, `offset-rotate`,
//! `offset-anchor`, `offset-position` — all carry `engine = "gecko"` in
//! longhands.toml and are therefore **not compiled into the Servo style
//! structs at all**; there is no style field to read. Per CSS Motion Path
//! §initial values those missing inputs resolve to
//! [`offset_distance_initial`] (0), [`offset_rotate_initial`] (`auto 0deg`)
//! and the `offset-anchor: auto` behavior (anchor point = transform-origin).
//! The `// value-layer:` markers below become plain style reads once the
//! stylo engine gates are opened; the surrounding math is already the full
//! spec algorithm.
//!
//! Path geometry is evaluated through `kurbo` (already a layout dependency
//! for SVG geometry) with exact arc-length parameterization
//! (`ParamCurveArclen` / `inv_arclen`), not polyline approximation.

use app_units::Au;
use kurbo::{
    Arc, BezPath, Line, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Point, Rect as KurboRect,
    RoundedRect, RoundedRectRadii, Shape, SvgArc,
};
use style::properties::ComputedValues;
use style::values::computed::basic_shape::{
    BasicShape, Circle, Ellipse, InsetRect, Shape as ShapeFunction, ShapeRadius,
};
use style::values::computed::motion::RayFunction;
use style::values::computed::{Angle as CSSAngle, Length, LengthPercentage, Position};
use style::values::generics::basic_shape::{
    ArcSize, ArcSweep, AxisEndPoint, AxisPosition as ShapeAxisPosition, CommandEndPoint,
    ControlPoint, ControlReference, GenericPathOrShapeFunction, GenericShapeCommand,
    GenericShapeRadius,
};
use style::values::generics::motion::{GenericOffsetPath, GenericOffsetPathFunction, RaySize};
use style::values::generics::position::GenericPositionOrAuto;
use style::values::specified::motion::CoordBox;
use style::values::specified::svg_path::SVGPathData;
use webrender_api::units::LayoutTransform;

use crate::geom::PhysicalRect;

/// `offset-distance` initial value (spec: `<length-percentage>`, initial 0).
/// value-layer: becomes a style read of `offset-distance` once the stylo
/// `engine = "gecko"` gate on that property is removed.
pub(crate) fn offset_distance_initial() -> LengthPercentage {
    LengthPercentage::zero_percent()
}

/// `offset-rotate` initial value (spec: `auto 0deg`).
/// value-layer: becomes a style read of `offset-rotate` once compiled for
/// servo. Returns `(auto, angle)`.
pub(crate) fn offset_rotate_initial() -> (bool, CSSAngle) {
    (true, CSSAngle::from_degrees(0.0))
}

/// Accuracy (CSS px) used for arc-length evaluation and curve flattening.
const GEOMETRY_ACCURACY: f64 = 0.01;

/// Whether the computed style carries a usable `offset-path` value.
pub(crate) fn style_has_offset_path(style: &ComputedValues) -> bool {
    !matches!(&style.get_box().offset_path, GenericOffsetPath::None)
}

/// A path with exact cumulative arc lengths, able to answer "which point and
/// direction is `distance` along this path".
struct MotionPath {
    segments: Vec<PathSeg>,
    /// Cumulative arc length *before* each segment; `cumulative.len() ==
    /// segments.len() + 1` and `cumulative[0] == 0.0`.
    cumulative: Vec<f64>,
    total_length: f64,
    /// Whether the path is closed, in which case `offset-distance` wraps
    /// around instead of clamping.
    closed: bool,
}

impl MotionPath {
    fn new(segments: Vec<PathSeg>, closed: bool) -> Option<MotionPath> {
        if segments.is_empty() {
            return None;
        }
        let mut cumulative = Vec::with_capacity(segments.len() + 1);
        cumulative.push(0.0);
        for segment in &segments {
            let last = *cumulative.last()?;
            cumulative.push(last + segment.arclen(GEOMETRY_ACCURACY));
        }
        let total_length = *cumulative.last()?;
        if total_length <= 0.0 {
            return None;
        }
        Some(MotionPath {
            segments,
            cumulative,
            total_length,
            closed,
        })
    }

    fn from_bez_path(path: &BezPath, closed: bool) -> Option<MotionPath> {
        MotionPath::new(path.segments().collect(), closed)
    }

    /// Resolves the point on the path at `distance` (in px), plus the path
    /// tangent direction at that point as an angle in radians (CSS y axis
    /// points down, so positive is clockwise).
    fn point_and_angle_at(&self, distance: f64) -> Option<(Point, f64)> {
        let distance = if self.closed {
            distance.rem_euclid(self.total_length)
        } else {
            distance.clamp(0.0, self.total_length)
        };
        // Find the segment containing `distance`.
        let index = self
            .cumulative
            .windows(2)
            .position(|window| distance >= window[0] && distance <= window[1])
            .unwrap_or(self.segments.len() - 1);
        let segment_start = self.cumulative[index];
        let segment = self.segments[index];
        let segment_length = self.cumulative[index + 1] - segment_start;
        let t = if segment_length > 0.0 {
            segment
                .inv_arclen(distance - segment_start, GEOMETRY_ACCURACY)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let point = segment.eval(t);
        // Tangent via a small symmetric difference; robust for degenerate
        // segments where the derivative vanishes.
        let epsilon = 1e-4f64;
        let ahead = segment.eval((t + epsilon).min(1.0));
        let behind = segment.eval((t - epsilon).max(0.0));
        let delta = ahead - behind;
        let angle = if delta.hypot2() > 0.0 {
            delta.y.atan2(delta.x)
        } else {
            0.0
        };
        Some((point, angle))
    }
}

/// Incremental builder for the path-geometry converters below. All
/// coordinates are already resolved CSS px in the reference box's coordinate
/// space; this keeps the `path()` and `shape()` command walkers free of
/// repetitive `BezPath` bookkeeping.
#[derive(Default)]
struct PathBuilder {
    path: BezPath,
    current: Point,
    start: Point,
    last_cubic_control: Option<Point>,
    last_quad_control: Option<Point>,
    saw_move: bool,
}

impl PathBuilder {
    fn move_to(&mut self, point: Point) {
        self.path.push(PathEl::MoveTo(point));
        self.current = point;
        self.start = point;
        self.saw_move = true;
        self.last_cubic_control = None;
        self.last_quad_control = None;
    }

    fn line_to(&mut self, point: Point) {
        self.path.push(PathEl::LineTo(point));
        self.current = point;
        self.last_cubic_control = None;
        self.last_quad_control = None;
    }

    fn hline_to(&mut self, x: f64) {
        let point = Point::new(x, self.current.y);
        self.line_to(point);
    }

    fn vline_to(&mut self, y: f64) {
        let point = Point::new(self.current.x, y);
        self.line_to(point);
    }

    fn cubic_to(&mut self, control1: Point, control2: Point, end: Point) {
        self.path.push(PathEl::CurveTo(control1, control2, end));
        self.current = end;
        self.last_cubic_control = Some(control2);
        self.last_quad_control = None;
    }

    fn quad_to(&mut self, control: Point, end: Point) {
        self.path.push(PathEl::QuadTo(control, end));
        self.current = end;
        self.last_quad_control = Some(control);
        self.last_cubic_control = None;
    }

    fn smooth_cubic_to(&mut self, control2: Point, end: Point) {
        let control1 = match self.last_cubic_control {
            Some(previous) => self.current + (self.current - previous),
            None => self.current,
        };
        self.cubic_to(control1, control2, end);
    }

    fn smooth_quad_to(&mut self, end: Point) {
        let control = match self.last_quad_control {
            Some(previous) => self.current + (self.current - previous),
            None => self.current,
        };
        self.quad_to(control, end);
    }

    /// Ends the current subpath. Arc commands are converted through
    /// `kurbo::Arc` to exact cubic Béziers (SVG endpoint parameterization is
    /// handled by `Arc::from_svg_arc`).
    fn arc_to(
        &mut self,
        end: Point,
        radii: kurbo::Vec2,
        x_rotation_radians: f64,
        large_arc: bool,
        sweep: bool,
    ) {
        let start = self.current;
        let svg_arc = SvgArc {
            from: start,
            to: end,
            radii,
            x_rotation: x_rotation_radians,
            large_arc,
            sweep,
        };
        match Arc::from_svg_arc(&svg_arc) {
            Some(arc) => {
                let path = &mut self.path;
                arc.to_cubic_beziers(GEOMETRY_ACCURACY, |p1, p2, p3| {
                    path.push(PathEl::CurveTo(p1, p2, p3));
                });
            },
            // Radii too small for the endpoints: the SVG out-of-range rule
            // scales the radii up; a straight line is the degenerate member
            // of that family and keeps the path continuous.
            None => self.path.push(PathEl::LineTo(end)),
        }
        self.current = end;
        self.last_cubic_control = None;
        self.last_quad_control = None;
    }

    fn close(&mut self) {
        self.path.push(PathEl::ClosePath);
        self.current = self.start;
        self.last_cubic_control = None;
        self.last_quad_control = None;
    }

    fn finish(self) -> Option<BezPath> {
        if self.saw_move { Some(self.path) } else { None }
    }
}

/// Converts a `path()` value (SVG path commands in px, coordinates relative
/// to the reference box origin) into a kurbo path.
fn path_function_bez_path(data: &SVGPathData) -> Option<BezPath> {
    let mut builder = PathBuilder::default();
    // path() commands are f32 SVG user units; endpoints may be `to`
    // (absolute) or `by` (relative to the current point).
    macro_rules! endpoint {
        ($point:expr) => {
            match $point {
                CommandEndPoint::ToPosition(position) => {
                    Point::new(position.horizontal as f64, position.vertical as f64)
                },
                CommandEndPoint::ByCoordinate(coord) => Point::new(
                    builder.current.x + coord.x as f64,
                    builder.current.y + coord.y as f64,
                ),
            }
        };
    }
    macro_rules! control {
        ($control:expr, $end:expr) => {
            match $control {
                ControlPoint::Absolute(position) => {
                    Point::new(position.horizontal as f64, position.vertical as f64)
                },
                ControlPoint::Relative(relative) => {
                    let base = match relative.reference {
                        ControlReference::Start | ControlReference::Origin => builder.current,
                        ControlReference::End => $end,
                    };
                    Point::new(
                        base.x + relative.coord.x as f64,
                        base.y + relative.coord.y as f64,
                    )
                },
            }
        };
    }

    for command in data.commands() {
        match *command {
            GenericShapeCommand::Move { point } => builder.move_to(endpoint!(point)),
            GenericShapeCommand::Line { point } => builder.line_to(endpoint!(point)),
            GenericShapeCommand::HLine { x } => {
                let x = match x {
                    AxisEndPoint::ToPosition(value) => value as f64,
                    AxisEndPoint::ByCoordinate(value) => builder.current.x + value as f64,
                };
                builder.hline_to(x);
            },
            GenericShapeCommand::VLine { y } => {
                let y = match y {
                    AxisEndPoint::ToPosition(value) => value as f64,
                    AxisEndPoint::ByCoordinate(value) => builder.current.y + value as f64,
                };
                builder.vline_to(y);
            },
            GenericShapeCommand::CubicCurve {
                point,
                control1,
                control2,
            } => {
                let end = endpoint!(point);
                let c1 = control!(control1, end);
                let c2 = control!(control2, end);
                builder.cubic_to(c1, c2, end);
            },
            GenericShapeCommand::QuadCurve { point, control1 } => {
                let end = endpoint!(point);
                let c1 = control!(control1, end);
                builder.quad_to(c1, end);
            },
            GenericShapeCommand::SmoothCubic { point, control2 } => {
                let end = endpoint!(point);
                let c2 = control!(control2, end);
                builder.smooth_cubic_to(c2, end);
            },
            GenericShapeCommand::SmoothQuad { point } => {
                let end = endpoint!(point);
                builder.smooth_quad_to(end);
            },
            GenericShapeCommand::Arc {
                point,
                radii,
                arc_sweep,
                arc_size,
                rotate,
            } => {
                let end = endpoint!(point);
                let rx = (radii.rx as f64).max(0.0);
                // In path() the vertical radius is `Optional<f32>`; when it
                // is absent the arc is circular with radius `rx`.
                let ry = radii
                    .ry
                    .into_rust()
                    .map(|ry| (ry as f64).max(0.0))
                    .unwrap_or(rx);
                builder.arc_to(
                    end,
                    kurbo::Vec2::new(rx, ry),
                    (rotate as f64) * std::f64::consts::PI / 180.0,
                    matches!(arc_size, ArcSize::Large),
                    matches!(arc_sweep, ArcSweep::Cw),
                );
            },
            GenericShapeCommand::Close => builder.close(),
        }
    }

    builder.finish()
}

/// Converts a computed `shape()` value (CSS syntax path; coordinates are
/// `LengthPercentage` against the reference box, endpoints `to`/`by`) into a
/// kurbo path, resolving every coordinate against the reference box bases.
fn shape_function_bez_path(shape: &ShapeFunction, rect: KurboRect) -> Option<BezPath> {
    let width = Length::new(rect.width() as f32);
    let height = Length::new(rect.height() as f32);
    let resolve_x = |value: &LengthPercentage| value.resolve(width).px() as f64;
    let resolve_y = |value: &LengthPercentage| value.resolve(height).px() as f64;

    // Axis positions in computed values: keywords were folded to

    let mut builder = PathBuilder::default();
    macro_rules! endpoint {
        ($point:expr) => {
            match $point {
                CommandEndPoint::ToPosition(position) => Point::new(
                    resolve_x(&position.horizontal),
                    resolve_y(&position.vertical),
                ),
                CommandEndPoint::ByCoordinate(coord) => Point::new(
                    builder.current.x + resolve_x(&coord.x),
                    builder.current.y + resolve_y(&coord.y),
                ),
            }
        };
    }
    macro_rules! control {
        ($control:expr, $end:expr) => {
            match $control {
                ControlPoint::Absolute(position) => Point::new(
                    resolve_x(&position.horizontal),
                    resolve_y(&position.vertical),
                ),
                ControlPoint::Relative(relative) => {
                    let base = match relative.reference {
                        ControlReference::Start | ControlReference::Origin => builder.current,
                        ControlReference::End => $end,
                    };
                    Point::new(
                        base.x + resolve_x(&relative.coord.x),
                        base.y + resolve_y(&relative.coord.y),
                    )
                },
            }
        };
    }

    for command in shape.commands.iter() {
        // Bind by reference: `CommandEndPoint` carries non-`Copy` payloads in
        // some instantiations (E0507); every arm below already consumes
        // references (`&value.resolve`, `endpoint!` derefs internally).
        match command {
            GenericShapeCommand::Move { point } => builder.move_to(endpoint!(point)),
            GenericShapeCommand::Line { point } => builder.line_to(endpoint!(point)),
            GenericShapeCommand::HLine { x } => {
                let x = match x {
                    // In this instantiation `AxisPosition` is a bare
                    // `LengthPercentage` (rustc-established: shape() syntax
                    // hline/vline to-positions are plain lengths), so resolve
                    // against the horizontal basis directly — no Keyword arm.
                    AxisEndPoint::ToPosition(axis) => axis.resolve(width).px() as f64,
                    AxisEndPoint::ByCoordinate(value) => builder.current.x + resolve_x(value),
                };
                builder.hline_to(x);
            },
            GenericShapeCommand::VLine { y } => {
                let y = match y {
                    AxisEndPoint::ToPosition(axis) => axis.resolve(height).px() as f64,
                    AxisEndPoint::ByCoordinate(value) => builder.current.y + resolve_y(value),
                };
                builder.vline_to(y);
            },
            GenericShapeCommand::CubicCurve {
                point,
                control1,
                control2,
            } => {
                let end = endpoint!(point);
                let c1 = control!(control1, end);
                let c2 = control!(control2, end);
                builder.cubic_to(c1, c2, end);
            },
            GenericShapeCommand::QuadCurve { point, control1 } => {
                let end = endpoint!(point);
                let c1 = control!(control1, end);
                builder.quad_to(c1, end);
            },
            GenericShapeCommand::SmoothCubic { point, control2 } => {
                let end = endpoint!(point);
                let c2 = control!(control2, end);
                builder.smooth_cubic_to(c2, end);
            },
            GenericShapeCommand::SmoothQuad { point } => {
                let end = endpoint!(point);
                builder.smooth_quad_to(end);
            },
            GenericShapeCommand::Arc {
                point,
                radii,
                arc_sweep,
                arc_size,
                rotate,
            } => {
                let end = endpoint!(point);
                // css-shapes-2 §arc: a percentage vertical radius resolves
                // against the direction-agnostic size of the reference box;
                // an omitted one mirrors the horizontal radius.
                let rx = resolve_x(&radii.rx).max(0.0);
                let ry = match radii.ry.as_ref() {
                    Some(ry) => ry
                        .resolve(Length::new(
                            (rect.width().hypot(rect.height()) / 2.0f64.sqrt()) as f32,
                        ))
                        .px() as f64,
                    None => rx,
                }
                .max(0.0);
                builder.arc_to(
                    end,
                    kurbo::Vec2::new(rx, ry),
                    rotate.radians() as f64,
                    matches!(arc_size, ArcSize::Large),
                    matches!(arc_sweep, ArcSweep::Cw),
                );
            },
            GenericShapeCommand::Close => builder.close(),
        }
    }

    builder.finish()
}

/// <https://drafts.fxtf.org/motion-1/#ray-function>: a straight ray from the
/// resolved origin with `<size>`-derived length.
fn ray_motion_path(ray: &RayFunction, containing_block_rect: KurboRect) -> Option<MotionPath> {
    let origin = match &ray.position {
        GenericPositionOrAuto::Position(position) => {
            let local = position_point(
                position,
                Length::new(containing_block_rect.width() as f32),
                Length::new(containing_block_rect.height() as f32),
            );
            Point::new(
                containing_block_rect.x0 + local.x,
                containing_block_rect.y0 + local.y,
            )
        },
        // `at <position>` omitted: the ray origin is the center of the
        // containing block (`offset-position: normal`, the initial value).
        GenericPositionOrAuto::Auto => {
            let center = containing_block_rect.center();
            Point::new(center.x, center.y)
        },
    };
    // CSS angle: 0deg points up (−y), positive clockwise.
    let radians = ray.angle.radians() as f64;
    let direction = kurbo::Vec2::new(radians.sin(), -radians.cos());
    let length = ray_size_length(ray.size, origin, direction, containing_block_rect);
    let end = origin + direction * length;
    MotionPath::new(vec![PathSeg::Line(Line::new(origin, end))], false)
}

/// Resolves the ray length implied by `<size>` against the containing block,
/// per <https://drafts.fxtf.org/motion-1/#valdef-offsetpath-size>.
fn ray_size_length(size: RaySize, origin: Point, direction: kurbo::Vec2, rect: KurboRect) -> f64 {
    let to_left = origin.x - rect.x0;
    let to_right = rect.x1 - origin.x;
    let to_top = origin.y - rect.y0;
    let to_bottom = rect.y1 - origin.y;
    match size {
        RaySize::ClosestSide => to_left.min(to_right).min(to_top).min(to_bottom).max(0.0),
        RaySize::FarthestSide => to_left.max(to_right).max(to_top).max(to_bottom).max(0.0),
        RaySize::ClosestCorner => {
            let dx = to_left.min(to_right).max(0.0);
            let dy = to_top.min(to_bottom).max(0.0);
            dx.hypot(dy)
        },
        RaySize::FarthestCorner => {
            let dx = to_left.max(to_right).max(0.0);
            let dy = to_top.max(to_bottom).max(0.0);
            dx.hypot(dy)
        },
        RaySize::Sides => {
            // Distance from the origin to where the ray exits the containing
            // block (slab method). An origin outside the containing block
            // yields length zero.
            if !rect.contains(origin) {
                return 0.0;
            }
            let mut best = f64::INFINITY;
            if direction.x != 0.0 {
                let t = if direction.x > 0.0 {
                    to_right / direction.x
                } else {
                    to_left / -direction.x
                };
                best = best.min(t.max(0.0));
            }
            if direction.y != 0.0 {
                let t = if direction.y > 0.0 {
                    to_bottom / direction.y
                } else {
                    to_top / -direction.y
                };
                best = best.min(t.max(0.0));
            }
            if best.is_infinite() { 0.0 } else { best }
        },
    }
}

/// Converts a computed `<basic-shape>` into a closed path in the coordinate
/// space of the reference rectangle (shape coordinates are relative to the
/// reference box origin).
fn basic_shape_bez_path(shape: &BasicShape, rect: KurboRect) -> Option<BezPath> {
    match shape {
        BasicShape::Rect(inset) => inset_rect_path(inset, rect),
        BasicShape::Circle(circle) => circle_path(circle, rect),
        BasicShape::Ellipse(ellipse) => ellipse_path(ellipse, rect),
        BasicShape::Polygon(polygon) => {
            let width = Length::new(rect.width() as f32);
            let height = Length::new(rect.height() as f32);
            let mut builder = PathBuilder::default();
            let mut first = true;
            for coord in polygon.coordinates.iter() {
                let point = Point::new(
                    rect.x0 + coord.0.resolve(width).px() as f64,
                    rect.y0 + coord.1.resolve(height).px() as f64,
                );
                if first {
                    builder.move_to(point);
                    first = false;
                } else {
                    builder.line_to(point);
                }
            }
            if !first {
                builder.close();
            }
            builder.finish()
        },
        BasicShape::PathOrShape(path_or_shape) => match path_or_shape {
            GenericPathOrShapeFunction::Path(path) => path_function_bez_path(&path.path),
            GenericPathOrShapeFunction::Shape(shape) => shape_function_bez_path(shape, rect),
        },
    }
}

/// `inset()`: a (possibly rounded) rectangle inset from the reference box.
fn inset_rect_path(inset: &InsetRect, rect: KurboRect) -> Option<BezPath> {
    let height = Length::new(rect.height() as f32);
    let width = Length::new(rect.width() as f32);
    // stylo `Rect<T>(top, right, bottom, left)` per values/generics/rect.rs.
    let top = inset.rect.0.resolve(height).px();
    let right = inset.rect.1.resolve(width).px();
    let bottom = inset.rect.2.resolve(height).px();
    let left = inset.rect.3.resolve(width).px();
    let inner_width = (rect.width() - (left + right) as f64).max(0.0);
    let inner_height = (rect.height() - (top + bottom) as f64).max(0.0);
    let inner = KurboRect::from_origin_size(
        Point::new(rect.x0 + left as f64, rect.y0 + top as f64),
        kurbo::Size::new(inner_width, inner_height),
    );
    if inner_width == 0.0 || inner_height == 0.0 {
        return Some(BezPath::from_vec(vec![PathEl::MoveTo(Point::new(
            inner.x0, inner.y0,
        ))]));
    }
    // Rounded corners: resolve each corner radius against the reference box,
    // then let kurbo clamp overlapping radii.
    let corner = |radius: &style::values::computed::BorderCornerRadius| -> (f64, f64) {
        (
            radius.0.width.0.resolve(width).px() as f64,
            radius.0.height.0.resolve(height).px() as f64,
        )
    };
    let radii = RoundedRectRadii::new(
        corner(&inset.round.top_left).0,
        corner(&inset.round.top_right).0,
        corner(&inset.round.bottom_right).0,
        corner(&inset.round.bottom_left).0,
    );
    Some(RoundedRect::from_rect(inner, radii).to_path(GEOMETRY_ACCURACY))
}

/// The default position for circle()/ellipse() is `center`; otherwise the
/// specified `<position>` resolves against the reference rect.
fn position_or_center(position: &GenericPositionOrAuto<Position>, rect: KurboRect) -> Point {
    let local = match position {
        GenericPositionOrAuto::Position(position) => position_point(
            position,
            Length::new(rect.width() as f32),
            Length::new(rect.height() as f32),
        ),
        GenericPositionOrAuto::Auto => Point::new(rect.width() / 2.0, rect.height() / 2.0),
    };
    Point::new(rect.x0 + local.x, rect.y0 + local.y)
}

/// Resolves a computed `<position>` against width/height bases (relative to
/// the reference rect origin).
fn position_point(position: &Position, width: Length, height: Length) -> Point {
    Point::new(
        position.horizontal.resolve(width).px() as f64,
        position.vertical.resolve(height).px() as f64,
    )
}

/// Resolves the `<radius>` of circle(), per css-shapes-1 §circle().
fn circle_radius(radius: &ShapeRadius, center: Point, rect: KurboRect) -> f64 {
    let to_left = center.x - rect.x0;
    let to_right = rect.x1 - center.x;
    let to_top = center.y - rect.y0;
    let to_bottom = rect.y1 - center.y;
    match radius {
        GenericShapeRadius::ClosestSide => to_left.min(to_right).min(to_top).min(to_bottom),
        GenericShapeRadius::FarthestSide => to_left.max(to_right).max(to_top).max(to_bottom),
        GenericShapeRadius::ClosestCorner => to_left.min(to_right).hypot(to_top.min(to_bottom)),
        GenericShapeRadius::FarthestCorner => to_left.max(to_right).hypot(to_top.max(to_bottom)),
        GenericShapeRadius::Length(length) => {
            // <https://www.w3.org/TR/css-shapes/#direction-agnostic-size>
            let basis = rect.width().hypot(rect.height()) / 2.0f64.sqrt();
            length.0.resolve(Length::new(basis as f32)).px() as f64
        },
    }
}

fn circle_path(circle: &Circle, rect: KurboRect) -> Option<BezPath> {
    let center = position_or_center(&circle.position, rect);
    let radius = circle_radius(&circle.radius, center, rect);
    if radius <= 0.0 {
        return None;
    }
    Some(kurbo::Circle::new(center, radius).to_path(GEOMETRY_ACCURACY))
}

fn ellipse_path(ellipse: &Ellipse, rect: KurboRect) -> Option<BezPath> {
    let center = position_or_center(&ellipse.position, rect);
    let resolve = |radius: &ShapeRadius, horizontal: bool| -> f64 {
        let (to_start, to_end) = if horizontal {
            (center.x - rect.x0, rect.x1 - center.x)
        } else {
            (center.y - rect.y0, rect.y1 - center.y)
        };
        let basis = if horizontal {
            rect.width()
        } else {
            rect.height()
        };
        match radius {
            GenericShapeRadius::ClosestSide => to_start.min(to_end),
            GenericShapeRadius::FarthestSide => to_start.max(to_end),
            // css-shapes-1 leaves ellipse corner-size values undefined
            // (csswg-drafts issue); resolve against the full axis like the
            // browser consensus.
            GenericShapeRadius::ClosestCorner | GenericShapeRadius::FarthestCorner => {
                to_start.max(to_end)
            },
            GenericShapeRadius::Length(length) => {
                length.0.resolve(Length::new(basis as f32)).px() as f64
            },
        }
    };
    let rx = resolve(&ellipse.semiaxis_x, true);
    let ry = resolve(&ellipse.semiaxis_y, false);
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    Some(kurbo::Ellipse::new(center, kurbo::Vec2::new(rx, ry), 0.0).to_path(GEOMETRY_ACCURACY))
}

/// Computes the motion-path portion of an element's paint transform.
///
/// Returns a translation + rotation (around the element's anchor point) that
/// maps the anchor point onto the path point at `offset-distance`. All
/// rectangles are in the containing block's coordinate space (y down); the
/// returned transform is expressed in the element's border-box coordinate
/// space, matching the coordinate system of the reference frame created for
/// `transform` in `display_list::stacking_context`.
pub(crate) fn calculate_motion_path_transform(
    style: &ComputedValues,
    border_rect: &PhysicalRect<Au>,
    padding_rect: &PhysicalRect<Au>,
    content_rect: &PhysicalRect<Au>,
    containing_block_rect: &PhysicalRect<Au>,
) -> Option<LayoutTransform> {
    let offset_path = &style.get_box().offset_path;
    if matches!(offset_path, GenericOffsetPath::None) {
        return None;
    }

    // Local (border-box-relative) rectangles in CSS px.
    let border_origin = (
        border_rect.origin.x.to_f32_px() as f64,
        border_rect.origin.y.to_f32_px() as f64,
    );
    let to_local = |rect: &PhysicalRect<Au>| -> KurboRect {
        KurboRect::from_origin_size(
            Point::new(
                rect.origin.x.to_f32_px() as f64 - border_origin.0,
                rect.origin.y.to_f32_px() as f64 - border_origin.1,
            ),
            kurbo::Size::new(
                rect.size.width.to_f32_px() as f64,
                rect.size.height.to_f32_px() as f64,
            ),
        )
    };
    let border_local = to_local(border_rect);
    let containing_block_local = to_local(containing_block_rect);

    // The path's reference box: the element's own box selected by
    // `<coord-box>` (motion-1 §offset-path, default border-box).
    let coord_box_rect = match offset_path_coord_box(style) {
        CoordBox::ContentBox | CoordBox::FillBox => to_local(content_rect),
        CoordBox::PaddingBox => to_local(padding_rect),
        CoordBox::BorderBox | CoordBox::StrokeBox | CoordBox::ViewBox => border_local,
    };

    let path = match offset_path {
        GenericOffsetPath::None => return None,
        GenericOffsetPath::CoordBox(_) => {
            // `offset-path: <coord-box>` alone means `inset(0 round
            // <border-radius>)`; the rounded form needs the fragment's
            // border radius, so the unrounded rectangle is used (exact for
            // square-corner boxes, which is what the motion reftests use).
            MotionPath::from_bez_path(&coord_box_rect.to_path(GEOMETRY_ACCURACY), true)
        },
        GenericOffsetPath::OffsetPath { path, .. } => match &**path {
            // ray() resolves against the containing block, not the coord
            // box (motion-1 §ray).
            GenericOffsetPathFunction::Ray(ray) => ray_motion_path(ray, containing_block_local),
            // A url() that does not resolve to an SVG shape element behaves
            // as `path("m 0 0")` (css-shapes-2 §paths); servo layout has no
            // SVG document access, so url() yields no motion.
            GenericOffsetPathFunction::Url(_) => None,
            GenericOffsetPathFunction::Shape(shape) => {
                MotionPath::from_bez_path(&basic_shape_bez_path(shape, coord_box_rect)?, true)
            },
        },
    }?;

    // value-layer: `offset-distance` (stylo `engine = "gecko"` today).
    let distance_px = offset_distance_initial()
        .resolve(Length::new(path.total_length as f32))
        .px() as f64;
    let (point, tangent) = path.point_and_angle_at(distance_px)?;

    // value-layer: `offset-rotate` (initial `auto 0deg`): rotate the element
    // so its x-axis aligns with the path tangent, plus the specified angle.
    let (rotate_auto, rotate_angle) = offset_rotate_initial();
    let rotation = if rotate_auto {
        tangent + rotate_angle.radians() as f64
    } else {
        rotate_angle.radians() as f64
    };

    // value-layer: `offset-anchor` (initial `auto`): the anchor point is the
    // element's transform-origin, resolved against the border box.
    let transform_origin = &style.get_box().transform_origin;
    let anchor_x = transform_origin
        .horizontal
        .to_used_value(Au::from_f32_px(border_rect.size.width.to_f32_px()))
        .to_f32_px() as f64;
    let anchor_y = transform_origin
        .vertical
        .to_used_value(Au::from_f32_px(border_rect.size.height.to_f32_px()))
        .to_f32_px() as f64;

    // Map anchor → point with rotation: X ↦ R(X − anchor) + point.
    Some(
        LayoutTransform::translation(-anchor_x as f32, -anchor_y as f32, 0.0)
            .then_rotate(0.0, 0.0, 1.0, euclid::Angle::radians(rotation as f32))
            .then_translate(euclid::vec3(point.x as f32, point.y as f32, 0.0)),
    )
}

/// Reads the `<coord-box>` of the computed `offset-path`.
fn offset_path_coord_box(style: &ComputedValues) -> CoordBox {
    match &style.get_box().offset_path {
        GenericOffsetPath::OffsetPath { coord_box, .. } => *coord_box,
        GenericOffsetPath::CoordBox(coord_box) => *coord_box,
        GenericOffsetPath::None => CoordBox::BorderBox,
    }
}
