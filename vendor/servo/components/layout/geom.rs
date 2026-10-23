/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::convert::From;
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::sync::atomic::{AtomicI32, Ordering};

use app_units::Au;
use euclid::{Point2D, Size2D};
use malloc_size_of_derive::MallocSizeOf;
use style::Zero;
use style::logical_geometry::{BlockFlowDirection, InlineBaseDirection, WritingMode};
use style::values::computed::{CSSPixelLength, LengthPercentage};
use style::values::generics::length::GenericLengthPercentageOrAuto as AutoOr;
use style_traits::CSSPixel;

use crate::ContainingBlock;
use crate::sizing::Size;

pub type PhysicalPoint<U> = euclid::Point2D<U, CSSPixel>;
pub type PhysicalSize<U> = euclid::Size2D<U, CSSPixel>;
pub type PhysicalVec<U> = euclid::Vector2D<U, CSSPixel>;
pub type PhysicalRect<U> = euclid::Rect<U, CSSPixel>;
pub type PhysicalSides<U> = euclid::SideOffsets2D<U, CSSPixel>;
pub type AuOrAuto = AutoOr<Au>;
pub type LengthPercentageOrAuto<'a> = AutoOr<&'a LengthPercentage>;

#[derive(Clone, Copy, MallocSizeOf, PartialEq)]
pub struct LogicalVec2<T> {
    pub inline: T,
    pub block: T,
}

#[derive(Clone, Copy, MallocSizeOf)]
pub struct LogicalRect<T> {
    pub start_corner: LogicalVec2<T>,
    pub size: LogicalVec2<T>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LogicalSides<T> {
    pub inline_start: T,
    pub inline_end: T,
    pub block_start: T,
    pub block_end: T,
}

#[derive(Clone, Copy, Debug, Default, MallocSizeOf, PartialEq)]
pub(crate) struct LogicalSides1D<T> {
    pub start: T,
    pub end: T,
}

impl<T> LogicalSides1D<T> {
    pub(crate) fn map<U>(&self, f: impl Fn(&T) -> U) -> LogicalSides1D<U> {
        LogicalSides1D {
            start: f(&self.start),
            end: f(&self.end),
        }
    }
}

impl LogicalSides1D<AutoOr<&LengthPercentage>> {
    pub(crate) fn percentages_relative_to(&self, basis: Au) -> LogicalSides1D<AutoOr<Au>> {
        self.map(|value| value.map(|length_percentage| length_percentage.to_used_value(basis)))
    }
}

impl<T: fmt::Debug> fmt::Debug for LogicalVec2<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Not using f.debug_struct on purpose here, to keep {:?} output somewhat compact
        f.write_str("Vec2 { i: ")?;
        self.inline.fmt(f)?;
        f.write_str(", b: ")?;
        self.block.fmt(f)?;
        f.write_str(" }")
    }
}

impl<T: Default> Default for LogicalVec2<T> {
    fn default() -> Self {
        Self {
            inline: T::default(),
            block: T::default(),
        }
    }
}

impl<T: Copy> From<T> for LogicalVec2<T> {
    fn from(value: T) -> Self {
        Self {
            inline: value,
            block: value,
        }
    }
}

impl<T> LogicalVec2<T> {
    pub fn map_inline_and_block_axes<U>(
        &self,
        inline_f: impl FnOnce(&T) -> U,
        block_f: impl FnOnce(&T) -> U,
    ) -> LogicalVec2<U> {
        LogicalVec2 {
            inline: inline_f(&self.inline),
            block: block_f(&self.block),
        }
    }
}

impl<T: Clone> LogicalVec2<Size<T>> {
    pub fn map_inline_and_block_sizes<U>(
        &self,
        inline_f: impl FnOnce(T) -> U,
        block_f: impl FnOnce(T) -> U,
    ) -> LogicalVec2<Size<U>> {
        self.map_inline_and_block_axes(|size| size.map(inline_f), |size| size.map(block_f))
    }
}

impl<T: Clone> LogicalVec2<T> {
    pub fn from_physical_size(physical_size: &PhysicalSize<T>, mode: WritingMode) -> Self {
        // https://drafts.csswg.org/css-writing-modes/#logical-to-physical
        let (i, b) = if mode.is_horizontal() {
            (&physical_size.width, &physical_size.height)
        } else {
            (&physical_size.height, &physical_size.width)
        };
        LogicalVec2 {
            inline: i.clone(),
            block: b.clone(),
        }
    }

    pub fn map<U>(&self, f: impl Fn(&T) -> U) -> LogicalVec2<U> {
        LogicalVec2 {
            inline: f(&self.inline),
            block: f(&self.block),
        }
    }

    pub(crate) fn map_with<U, V>(
        &self,
        other: &LogicalVec2<U>,
        f: impl Fn(&T, &U) -> V,
    ) -> LogicalVec2<V> {
        LogicalVec2 {
            inline: f(&self.inline, &other.inline),
            block: f(&self.block, &other.block),
        }
    }
}

impl<T: Add<Output = T> + Copy> Add<LogicalVec2<T>> for LogicalVec2<T> {
    type Output = LogicalVec2<T>;
    fn add(self, other: Self) -> Self::Output {
        LogicalVec2 {
            inline: self.inline + other.inline,
            block: self.block + other.block,
        }
    }
}

impl<T: Sub<Output = T> + Copy> Sub<LogicalVec2<T>> for LogicalVec2<T> {
    type Output = LogicalVec2<T>;
    fn sub(self, other: Self) -> Self::Output {
        LogicalVec2 {
            inline: self.inline - other.inline,
            block: self.block - other.block,
        }
    }
}

impl<T: AddAssign<T> + Copy> AddAssign<LogicalVec2<T>> for LogicalVec2<T> {
    fn add_assign(&mut self, other: LogicalVec2<T>) {
        self.inline += other.inline;
        self.block += other.block;
    }
}

impl<T: SubAssign<T> + Copy> SubAssign<LogicalVec2<T>> for LogicalVec2<T> {
    fn sub_assign(&mut self, other: LogicalVec2<T>) {
        self.inline -= other.inline;
        self.block -= other.block;
    }
}

impl<T: Neg<Output = T> + Copy> Neg for LogicalVec2<T> {
    type Output = LogicalVec2<T>;
    fn neg(self) -> Self::Output {
        Self {
            inline: -self.inline,
            block: -self.block,
        }
    }
}

impl<T: Zero> LogicalVec2<T> {
    pub fn zero() -> Self {
        Self {
            inline: T::zero(),
            block: T::zero(),
        }
    }
}

impl<T: Clone> LogicalVec2<AutoOr<T>> {
    pub fn auto_is(&self, f: impl Fn() -> T) -> LogicalVec2<T> {
        self.map(|t| t.auto_is(&f))
    }
}

impl<T: Zero> LogicalRect<T> {
    pub fn zero() -> Self {
        Self {
            start_corner: LogicalVec2::zero(),
            size: LogicalVec2::zero(),
        }
    }
}

impl fmt::Debug for LogicalRect<Au> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Rect(i{}×b{} @ (i{},b{}))",
            self.size.inline.to_f32_px(),
            self.size.block.to_f32_px(),
            self.start_corner.inline.to_f32_px(),
            self.start_corner.block.to_f32_px(),
        )
    }
}

impl<T: Clone> LogicalVec2<T> {
    /// The physical size of a logical size. Sizes never change sign.
    // https://drafts.csswg.org/css-writing-modes/#logical-to-physical
    pub fn to_physical_size(&self, mode: WritingMode) -> PhysicalSize<T> {
        let (x, y) = if mode.is_horizontal() {
            (&self.inline, &self.block)
        } else {
            (&self.block, &self.inline)
        };
        PhysicalSize::new(x.clone(), y.clone())
    }
}

impl<T: Copy + Neg<Output = T>> LogicalVec2<T> {
    /// The physical translation of a logical offset. The sign of each component
    /// depends on the physical direction that the logical axis points to:
    ///
    ///  - The block component is negated in `vertical-rl` (blocks stack from the
    ///    right edge leftward) and in `sideways-lr`.
    ///  - The inline component is negated when the inline direction runs against
    ///    the physical axis (`direction: rtl` or a bottom-to-top vertical flow).
    ///
    /// Use this for offsets between boxes (for instance when translating child
    /// fragments); use [`Self::to_physical_size`] for sizes, which never flip.
    pub fn to_physical_translation(&self, mode: WritingMode) -> PhysicalVec<T> {
        if mode.is_horizontal() {
            if mode.is_bidi_ltr() {
                PhysicalVec::new(self.inline, self.block)
            } else {
                PhysicalVec::new(-self.inline, self.block)
            }
        } else if mode.is_vertical_lr() {
            // vertical-lr (and sideways-lr): blocks stack left to right.
            let block = if mode.is_inline_tb() {
                PhysicalVec::new(self.block, self.inline)
            } else {
                PhysicalVec::new(self.block, -self.inline)
            };
            block
        } else {
            // vertical-rl (and sideways-rl): blocks stack right to left.
            if mode.is_inline_tb() {
                PhysicalVec::new(-self.block, self.inline)
            } else {
                PhysicalVec::new(-self.block, -self.inline)
            }
        }
    }

    /// Legacy alias of [`Self::to_physical_translation`], kept for offsets that
    /// were historically converted with `to_physical_vector`.
    pub fn to_physical_vector(&self, mode: WritingMode) -> PhysicalVec<T> {
        self.to_physical_translation(mode)
    }
}

impl<T: Clone> LogicalSides<T> {
    pub fn from_physical(sides: &PhysicalSides<T>, mode: WritingMode) -> Self {
        // https://drafts.csswg.org/css-writing-modes/#logical-to-physical
        let block_flow = mode.block_flow_direction();
        let (bs, be) = match mode.block_flow_direction() {
            BlockFlowDirection::TopToBottom => (&sides.top, &sides.bottom),
            BlockFlowDirection::RightToLeft => (&sides.right, &sides.left),
            BlockFlowDirection::LeftToRight => (&sides.left, &sides.right),
        };
        use BlockFlowDirection::TopToBottom;
        let (is, ie) = match (block_flow, mode.inline_base_direction()) {
            (TopToBottom, InlineBaseDirection::LeftToRight) => (&sides.left, &sides.right),
            (TopToBottom, InlineBaseDirection::RightToLeft) => (&sides.right, &sides.left),
            (_, InlineBaseDirection::LeftToRight) => (&sides.top, &sides.bottom),
            (_, InlineBaseDirection::RightToLeft) => (&sides.bottom, &sides.top),
        };
        LogicalSides {
            inline_start: is.clone(),
            inline_end: ie.clone(),
            block_start: bs.clone(),
            block_end: be.clone(),
        }
    }
}

impl<T> LogicalSides<T> {
    pub fn map<U>(&self, f: impl Fn(&T) -> U) -> LogicalSides<U> {
        LogicalSides {
            inline_start: f(&self.inline_start),
            inline_end: f(&self.inline_end),
            block_start: f(&self.block_start),
            block_end: f(&self.block_end),
        }
    }

    pub fn map_inline_and_block_axes<U>(
        &self,
        inline_f: impl Fn(&T) -> U,
        block_f: impl Fn(&T) -> U,
    ) -> LogicalSides<U> {
        LogicalSides {
            inline_start: inline_f(&self.inline_start),
            inline_end: inline_f(&self.inline_end),
            block_start: block_f(&self.block_start),
            block_end: block_f(&self.block_end),
        }
    }

    pub fn inline_sum(&self) -> T::Output
    where
        T: Add + Copy,
    {
        self.inline_start + self.inline_end
    }

    pub fn block_sum(&self) -> T::Output
    where
        T: Add + Copy,
    {
        self.block_start + self.block_end
    }

    pub fn sum(&self) -> LogicalVec2<T::Output>
    where
        T: Add + Copy,
    {
        LogicalVec2 {
            inline: self.inline_sum(),
            block: self.block_sum(),
        }
    }

    pub fn to_physical(&self, mode: WritingMode) -> PhysicalSides<T>
    where
        T: Clone,
    {
        let top;
        let right;
        let bottom;
        let left;
        if mode.is_vertical() {
            if mode.is_vertical_lr() {
                left = self.block_start.clone();
                right = self.block_end.clone();
            } else {
                right = self.block_start.clone();
                left = self.block_end.clone();
            }

            if mode.is_inline_tb() {
                top = self.inline_start.clone();
                bottom = self.inline_end.clone();
            } else {
                bottom = self.inline_start.clone();
                top = self.inline_end.clone();
            }
        } else {
            top = self.block_start.clone();
            bottom = self.block_end.clone();
            if mode.is_bidi_ltr() {
                left = self.inline_start.clone();
                right = self.inline_end.clone();
            } else {
                right = self.inline_start.clone();
                left = self.inline_end.clone();
            }
        }
        PhysicalSides::new(top, right, bottom, left)
    }
}

impl<T: Copy> LogicalSides<T> {
    pub fn start_offset(&self) -> LogicalVec2<T> {
        LogicalVec2 {
            inline: self.inline_start,
            block: self.block_start,
        }
    }

    #[inline]
    pub(crate) fn inline_sides(&self) -> LogicalSides1D<T> {
        LogicalSides1D::new(self.inline_start, self.inline_end)
    }

    #[inline]
    pub(crate) fn block_sides(&self) -> LogicalSides1D<T> {
        LogicalSides1D::new(self.block_start, self.block_end)
    }
}

impl LogicalSides<LengthPercentage> {
    pub fn percentages_relative_to(&self, basis: Au) -> LogicalSides<Au> {
        self.map(|value| value.to_used_value(basis))
    }
}

impl LogicalSides<LengthPercentageOrAuto<'_>> {
    pub fn percentages_relative_to(&self, basis: Au) -> LogicalSides<AuOrAuto> {
        self.map(|value| value.map(|value| value.to_used_value(basis)))
    }
}

impl<T: Clone> LogicalSides<AutoOr<T>> {
    pub fn auto_is(&self, f: impl Fn() -> T) -> LogicalSides<T> {
        self.map(|s| s.auto_is(&f))
    }
}

impl<T: Add<Output = T> + Copy> Add<LogicalSides<T>> for LogicalSides<T> {
    type Output = LogicalSides<T>;

    fn add(self, other: Self) -> Self::Output {
        LogicalSides {
            inline_start: self.inline_start + other.inline_start,
            inline_end: self.inline_end + other.inline_end,
            block_start: self.block_start + other.block_start,
            block_end: self.block_end + other.block_end,
        }
    }
}

impl<T: Sub<Output = T> + Copy> Sub<LogicalSides<T>> for LogicalSides<T> {
    type Output = LogicalSides<T>;

    fn sub(self, other: Self) -> Self::Output {
        LogicalSides {
            inline_start: self.inline_start - other.inline_start,
            inline_end: self.inline_end - other.inline_end,
            block_start: self.block_start - other.block_start,
            block_end: self.block_end - other.block_end,
        }
    }
}

impl<T: Neg<Output = T> + Copy> Neg for LogicalSides<T> {
    type Output = LogicalSides<T>;
    fn neg(self) -> Self::Output {
        Self {
            inline_start: -self.inline_start,
            inline_end: -self.inline_end,
            block_start: -self.block_start,
            block_end: -self.block_end,
        }
    }
}

impl<T: Zero> LogicalSides<T> {
    pub(crate) fn zero() -> LogicalSides<T> {
        Self {
            inline_start: T::zero(),
            inline_end: T::zero(),
            block_start: T::zero(),
            block_end: T::zero(),
        }
    }
}

impl From<LogicalSides<CSSPixelLength>> for LogicalSides<Au> {
    fn from(value: LogicalSides<CSSPixelLength>) -> Self {
        Self {
            inline_start: value.inline_start.into(),
            inline_end: value.inline_end.into(),
            block_start: value.block_start.into(),
            block_end: value.block_end.into(),
        }
    }
}

impl From<LogicalSides<Au>> for LogicalSides<CSSPixelLength> {
    fn from(value: LogicalSides<Au>) -> Self {
        Self {
            inline_start: value.inline_start.into(),
            inline_end: value.inline_end.into(),
            block_start: value.block_start.into(),
            block_end: value.block_end.into(),
        }
    }
}

impl<T> LogicalSides1D<T> {
    #[inline]
    pub(crate) fn new(start: T, end: T) -> Self {
        Self { start, end }
    }
}

impl<T> LogicalSides1D<AutoOr<T>> {
    #[inline]
    pub(crate) fn either_specified(&self) -> bool {
        !self.start.is_auto() || !self.end.is_auto()
    }

    #[inline]
    pub(crate) fn either_auto(&self) -> bool {
        self.start.is_auto() || self.end.is_auto()
    }
}

impl<T> LogicalRect<T> {
    pub fn max_inline_position(&self) -> T
    where
        T: Add<Output = T> + Copy,
    {
        self.start_corner.inline + self.size.inline
    }

    pub fn max_block_position(&self) -> T
    where
        T: Add<Output = T> + Copy,
    {
        self.start_corner.block + self.size.block
    }

    pub fn inflate(&self, sides: &LogicalSides<T>) -> Self
    where
        T: Add<Output = T> + Copy,
        T: Sub<Output = T> + Copy,
    {
        Self {
            start_corner: LogicalVec2 {
                inline: self.start_corner.inline - sides.inline_start,
                block: self.start_corner.block - sides.block_start,
            },
            size: LogicalVec2 {
                inline: self.size.inline + sides.inline_sum(),
                block: self.size.block + sides.block_sum(),
            },
        }
    }

    pub fn deflate(&self, sides: &LogicalSides<T>) -> Self
    where
        T: Add<Output = T> + Copy,
        T: Sub<Output = T> + Copy,
    {
        LogicalRect {
            start_corner: LogicalVec2 {
                inline: self.start_corner.inline + sides.inline_start,
                block: self.start_corner.block + sides.block_start,
            },
            size: LogicalVec2 {
                inline: self.size.inline - sides.inline_sum(),
                block: self.size.block - sides.block_sum(),
            },
        }
    }
}

impl LogicalRect<Au> {
    /// The physical rectangle of this logical rectangle, relative to the top-left
    /// corner of the given containing block's content box.
    ///
    /// The mapping honors both the block flow direction (`vertical-rl` measures
    /// the block axis from the right edge) and the inline direction (`direction:
    /// rtl` in horizontal flows and bottom-to-top vertical flows measure the
    /// inline axis from the physical end edge). This requires knowing the size
    /// of the containing block in the flipped axis; when the containing block is
    /// `None` or its size is indefinite, the rectangle is measured from the
    /// physical start edge instead.
    pub(crate) fn as_physical(
        &self,
        containing_block: Option<&ContainingBlock<'_>>,
    ) -> PhysicalRect<Au> {
        let mode = containing_block.map_or_else(WritingMode::horizontal_tb, |containing_block| {
            containing_block.style.writing_mode
        });

        // Map one logical axis coordinate to a physical coordinate relative to
        // the top-left corner of the containing block. `inline_axis` selects the
        // logical axis to convert; the other axis is used as-is.
        let convert_axis = |start: Au, size: Au, inline_axis: bool| -> Au {
            // Whether this logical axis is measured against the physical end
            // edge of its axis, which requires flipping the coordinate.
            let measured_from_end = if mode.is_vertical() {
                // The block axis of a vertical flow runs right-to-left in
                // `vertical-rl`; the inline axis runs bottom-to-top when the
                // inline base direction is reversed.
                if inline_axis {
                    !mode.is_inline_tb()
                } else {
                    mode.is_vertical_rl()
                }
            } else {
                // The block axis of a horizontal flow always runs top-to-bottom;
                // the inline axis runs right-to-left for `direction: rtl`.
                inline_axis && !mode.is_bidi_ltr()
            };
            if !measured_from_end {
                return start;
            }

            // Flipping needs the size of the containing block along the
            // flipped physical axis. That is the inline size for the inline
            // axis and the block size for the block axis.
            let Some(containing_block) = containing_block else {
                return start;
            };
            let axis_size = if inline_axis {
                Some(containing_block.size.inline)
            } else {
                containing_block.size.block.to_definite()
            };
            match axis_size {
                Some(axis_size) => axis_size - (start + size),
                None => start,
            }
        };

        let (x, y, width, height) = if mode.is_vertical() {
            (
                convert_axis(self.start_corner.block, self.size.block, false),
                convert_axis(self.start_corner.inline, self.size.inline, true),
                self.size.block,
                self.size.inline,
            )
        } else {
            (
                convert_axis(self.start_corner.inline, self.size.inline, true),
                convert_axis(self.start_corner.block, self.size.block, false),
                self.size.inline,
                self.size.block,
            )
        };

        PhysicalRect::new(PhysicalPoint::new(x, y), PhysicalSize::new(width, height))
    }
}

impl From<LogicalVec2<CSSPixelLength>> for LogicalVec2<Au> {
    fn from(value: LogicalVec2<CSSPixelLength>) -> Self {
        LogicalVec2 {
            inline: value.inline.into(),
            block: value.block.into(),
        }
    }
}

impl From<LogicalVec2<Au>> for LogicalVec2<CSSPixelLength> {
    fn from(value: LogicalVec2<Au>) -> Self {
        LogicalVec2 {
            inline: value.inline.into(),
            block: value.block.into(),
        }
    }
}

impl From<LogicalRect<Au>> for LogicalRect<CSSPixelLength> {
    fn from(value: LogicalRect<Au>) -> Self {
        LogicalRect {
            start_corner: value.start_corner.into(),
            size: value.size.into(),
        }
    }
}

impl From<LogicalRect<CSSPixelLength>> for LogicalRect<Au> {
    fn from(value: LogicalRect<CSSPixelLength>) -> Self {
        LogicalRect {
            start_corner: value.start_corner.into(),
            size: value.size.into(),
        }
    }
}

pub(crate) trait ToLogical<Unit, LogicalType> {
    fn to_logical(&self, writing_mode: WritingMode) -> LogicalType;
}

impl<Unit: Copy> ToLogical<Unit, LogicalVec2<Unit>> for PhysicalSize<Unit> {
    fn to_logical(&self, writing_mode: WritingMode) -> LogicalVec2<Unit> {
        LogicalVec2::from_physical_size(self, writing_mode)
    }
}

impl<Unit: Copy> ToLogical<Unit, LogicalSides<Unit>> for PhysicalSides<Unit> {
    fn to_logical(&self, writing_mode: WritingMode) -> LogicalSides<Unit> {
        LogicalSides::from_physical(self, writing_mode)
    }
}

pub(crate) trait ToLogicalWithContainingBlock<LogicalType> {
    fn to_logical(&self, containing_block: &ContainingBlock) -> LogicalType;
}

impl ToLogicalWithContainingBlock<LogicalVec2<Au>> for PhysicalPoint<Au> {
    fn to_logical(&self, containing_block: &ContainingBlock) -> LogicalVec2<Au> {
        let writing_mode = containing_block.style.writing_mode;
        if writing_mode.is_vertical() {
            LogicalVec2 {
                // The inline axis of a vertical flow is the physical vertical axis;
                // it runs bottom-to-top when the inline base direction is reversed.
                inline: if writing_mode.is_inline_tb() {
                    self.y
                } else {
                    containing_block.size.inline - self.y
                },
                // The block axis of a vertical flow is the physical horizontal
                // axis; it runs right-to-left in `vertical-rl`.
                block: if writing_mode.is_vertical_lr() {
                    self.x
                } else {
                    containing_block
                        .size
                        .block
                        .to_definite()
                        .map(|block_size| block_size - self.x)
                        .unwrap_or(self.x)
                },
            }
        } else {
            LogicalVec2 {
                inline: if writing_mode.is_bidi_ltr() {
                    self.x
                } else {
                    containing_block.size.inline - self.x
                },
                block: self.y,
            }
        }
    }
}

impl ToLogicalWithContainingBlock<LogicalRect<Au>> for PhysicalRect<Au> {
    fn to_logical(&self, containing_block: &ContainingBlock) -> LogicalRect<Au> {
        let inline_start;
        let block_start;
        let inline;
        let block;

        let writing_mode = containing_block.style.writing_mode;
        if writing_mode.is_vertical() {
            inline = self.size.height;
            block = self.size.width;
            block_start = if writing_mode.is_vertical_lr() {
                self.origin.x
            } else {
                containing_block
                    .size
                    .block
                    .to_definite()
                    .map(|block_size| block_size - (self.origin.x + self.size.width))
                    .unwrap_or(self.origin.x)
            };
            inline_start = if writing_mode.is_inline_tb() {
                self.origin.y
            } else {
                containing_block.size.inline - (self.origin.y + self.size.height)
            };
        } else {
            inline = self.size.width;
            block = self.size.height;
            block_start = self.origin.y;
            if writing_mode.is_bidi_ltr() {
                inline_start = self.origin.x;
            } else {
                inline_start = containing_block.size.inline - (self.origin.x + self.size.width);
            }
        }
        LogicalRect {
            start_corner: LogicalVec2 {
                inline: inline_start,
                block: block_start,
            },
            size: LogicalVec2 { inline, block },
        }
    }
}

/// A `PhysicalRect<Au>` with shared mutablitiy
///
/// It is based on `AtomicI32` but is not atomic itself: there is no protection against tearing:
/// If multiple threads are calling `set()`, the rectangle may end up with some fields from one call
/// and some fields from another call.
///
/// Compared to `AtomicRefCell<PhysicalRect<Au>>`:
///
/// * Both are meant for uses where despite shared ownership, no concurrency is expected in practice.
/// * If concurrent access does unexpectedly happen `AtomicRefCell` will panic,
///   whereas `SyncPhysicalRectAu` can tear and result in invalid/inconsistent data.
/// * `SyncPhysicalRectAu` has no space overhead, `AtomicRefCell` stores an `AtomicUsize` borrow flag
/// * `SyncPhysicalRectAu` uses relaxed atomic access for each field separately
#[derive(MallocSizeOf, Default)]
pub(crate) struct SyncPhysicalRectAu(PhysicalRect<AtomicI32>);

impl SyncPhysicalRectAu {
    #[inline]
    pub(crate) fn new(rect: PhysicalRect<Au>) -> Self {
        Self(PhysicalRect::new(
            PhysicalPoint::new(rect.origin.x.0.into(), rect.origin.y.0.into()),
            PhysicalSize::new(rect.size.width.0.into(), rect.size.height.0.into()),
        ))
    }

    pub(crate) fn get(&self) -> PhysicalRect<Au> {
        PhysicalRect::new(self.origin(), self.size())
    }

    pub(crate) fn set(&self, new_rect: PhysicalRect<Au>) {
        self.set_origin(new_rect.origin);
        self.set_size(new_rect.size);
    }

    #[inline]
    pub(crate) fn origin(&self) -> PhysicalPoint<Au> {
        Point2D::new(
            // Bypass clamping in `Au::new`: the value originally came from another `Au`
            // so it’s expected to already be in range
            Au(self.0.origin.x.load(Ordering::Relaxed)),
            Au(self.0.origin.y.load(Ordering::Relaxed)),
        )
    }

    #[inline]
    pub(crate) fn size(&self) -> PhysicalSize<Au> {
        Size2D::new(
            // Bypass clamping in `Au::new`: the value originally came from another `Au`
            // so it’s expected to already be in range
            Au(self.0.size.width.load(Ordering::Relaxed)),
            Au(self.0.size.height.load(Ordering::Relaxed)),
        )
    }

    #[inline]
    pub(crate) fn set_origin(&self, new_origin: PhysicalPoint<Au>) {
        let origin = &self.0.origin;
        origin.x.store(new_origin.x.0, Ordering::Relaxed);
        origin.y.store(new_origin.y.0, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn set_size(&self, new_size: PhysicalSize<Au>) {
        let size = &self.0.size;
        size.width.store(new_size.width.0, Ordering::Relaxed);
        size.height.store(new_size.height.0, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn translate(&self, offset: PhysicalSize<Au>) {
        // This code explicitly does not use `AtomicI32::fetch_add`, as we rely on Au's
        // overflow detection to clamp the resulting value between `MAX_AU` and `MIN_AU`.
        let new_origin = self.origin() + offset;
        self.set_origin(new_origin);
    }
}

impl fmt::Debug for SyncPhysicalRectAu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}
