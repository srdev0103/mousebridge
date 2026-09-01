//! Geometry in logical points.
//!
//! Every rectangle here lives in the *virtual desktop space*: one shared 2D plane
//! into which every screen of every device is placed. Working in one plane is what
//! lets multi-monitor, mismatched resolutions, Retina and Windows fractional
//! scaling all reduce to the same problem.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Backing scale factor of a display (1.0 at 100%, 2.0 for a Retina panel).
///
/// Carried alongside geometry rather than baked into it: the topology engine works
/// in logical points, but the platform layer needs the factor to convert back to
/// physical pixels when it injects a cursor position.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Scale(f64);

impl Scale {
    /// Scale factor of 1.0.
    pub const ONE: Self = Self(1.0);

    /// Wraps a scale factor, rejecting values that would produce degenerate
    /// geometry.
    ///
    /// # Errors
    ///
    /// Returns `None` if `factor` is not finite or is not greater than zero.
    #[must_use]
    pub fn new(factor: f64) -> Option<Self> {
        (factor.is_finite() && factor > 0.0).then_some(Self(factor))
    }

    /// Returns the scale factor.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Default for Scale {
    fn default() -> Self {
        Self::ONE
    }
}

/// A point in the virtual desktop space, in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalPoint {
    /// Horizontal coordinate; increases to the right.
    pub x: f64,
    /// Vertical coordinate; increases downward, matching both macOS global display
    /// space and the Windows virtual desktop.
    pub y: f64,
}

impl LogicalPoint {
    /// The origin.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Builds a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Returns this point offset by `dx`, `dy`.
    #[must_use]
    pub const fn offset(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    /// Returns true if both coordinates are finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl fmt::Display for LogicalPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:.1}, {:.1})", self.x, self.y)
    }
}

/// A size in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalSize {
    /// Width in logical points.
    pub width: f64,
    /// Height in logical points.
    pub height: f64,
}

impl LogicalSize {
    /// Builds a size, rejecting non-positive or non-finite dimensions.
    ///
    /// # Errors
    ///
    /// Returns `None` for zero, negative, infinite or NaN dimensions. A
    /// zero-area screen is never legitimate and silently accepting one produces
    /// division-by-zero deep inside the edge-crossing maths.
    #[must_use]
    pub fn new(width: f64, height: f64) -> Option<Self> {
        (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
            .then_some(Self { width, height })
    }
}

/// The four sides of a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    /// The left (minimum X) side.
    Left,
    /// The right (maximum X) side.
    Right,
    /// The top (minimum Y) side.
    Top,
    /// The bottom (maximum Y) side.
    Bottom,
}

impl Edge {
    /// All four edges, in a stable order.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    /// Returns the edge a cursor arrives at when it leaves through this one.
    ///
    /// Leaving the right edge of one screen means entering the left edge of the
    /// screen beyond it.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    /// Returns true for the left and right edges.
    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// An axis-aligned rectangle in the virtual desktop space.
///
/// Containment is deliberately **half-open**: a rectangle owns its minimum edges
/// and not its maximum ones. Screens placed edge to edge therefore never both
/// claim the same coordinate, which is what stops the cursor oscillating between
/// two devices at a shared seam.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    /// Top-left corner.
    pub origin: LogicalPoint,
    /// Extent from the origin.
    pub size: LogicalSize,
}

impl LogicalRect {
    /// Builds a rectangle from an origin and a size.
    #[must_use]
    pub const fn new(origin: LogicalPoint, size: LogicalSize) -> Self {
        Self { origin, size }
    }

    /// Builds a rectangle from raw components, validating the size.
    ///
    /// # Errors
    ///
    /// Returns `None` if the size is degenerate; see [`LogicalSize::new`].
    #[must_use]
    pub fn from_parts(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        Some(Self::new(
            LogicalPoint::new(x, y),
            LogicalSize::new(width, height)?,
        ))
    }

    /// Minimum X coordinate (left edge).
    #[must_use]
    pub const fn min_x(&self) -> f64 {
        self.origin.x
    }

    /// Maximum X coordinate (right edge, exclusive).
    #[must_use]
    pub const fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }

    /// Minimum Y coordinate (top edge).
    #[must_use]
    pub const fn min_y(&self) -> f64 {
        self.origin.y
    }

    /// Maximum Y coordinate (bottom edge, exclusive).
    #[must_use]
    pub const fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }

    /// Returns the centre point.
    #[must_use]
    pub fn center(&self) -> LogicalPoint {
        LogicalPoint::new(
            self.origin.x + self.size.width / 2.0,
            self.origin.y + self.size.height / 2.0,
        )
    }

    /// Returns the coordinate of one edge.
    #[must_use]
    pub const fn edge_coord(&self, edge: Edge) -> f64 {
        match edge {
            Edge::Left => self.min_x(),
            Edge::Right => self.max_x(),
            Edge::Top => self.min_y(),
            Edge::Bottom => self.max_y(),
        }
    }

    /// Returns true if the point lies inside, using half-open bounds.
    #[must_use]
    pub fn contains(&self, p: LogicalPoint) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    /// Returns true if the two rectangles overlap in area.
    ///
    /// Rectangles that merely touch along an edge do not overlap, which is the
    /// normal arrangement for adjacent screens.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min_x() < other.max_x()
            && other.min_x() < self.max_x()
            && self.min_y() < other.max_y()
            && other.min_y() < self.max_y()
    }

    /// Returns the overlapping span of two rectangles along one axis.
    ///
    /// Used to decide whether two screens share enough of an edge to be worth
    /// crossing between. Returns `None` when they do not overlap on that axis.
    #[must_use]
    pub fn overlap_span(&self, other: &Self, horizontal: bool) -> Option<(f64, f64)> {
        let (a_min, a_max, b_min, b_max) = if horizontal {
            (self.min_x(), self.max_x(), other.min_x(), other.max_x())
        } else {
            (self.min_y(), self.max_y(), other.min_y(), other.max_y())
        };
        let lo = a_min.max(b_min);
        let hi = a_max.min(b_max);
        (lo < hi).then_some((lo, hi))
    }

    /// Expresses a point as a fraction of this rectangle, each axis in `0.0..=1.0`.
    ///
    /// This is how an edge crossing keeps its position when the two screens differ
    /// in size: the sender describes *where along the edge* the cursor left, and
    /// the receiver resolves that against its own geometry.
    #[must_use]
    pub fn normalize(&self, p: LogicalPoint) -> (f64, f64) {
        (
            ((p.x - self.min_x()) / self.size.width).clamp(0.0, 1.0),
            ((p.y - self.min_y()) / self.size.height).clamp(0.0, 1.0),
        )
    }

    /// Inverse of [`LogicalRect::normalize`].
    #[must_use]
    pub fn denormalize(&self, nx: f64, ny: f64) -> LogicalPoint {
        LogicalPoint::new(
            self.min_x() + nx.clamp(0.0, 1.0) * self.size.width,
            self.min_y() + ny.clamp(0.0, 1.0) * self.size.height,
        )
    }

    /// Clamps a point so that it lies strictly inside the rectangle.
    ///
    /// `inset` keeps the result off the exclusive maximum edges. Landing exactly
    /// on `max_x` would place the cursor in the *next* screen under half-open
    /// containment, which is precisely the off-by-one that causes a handoff to
    /// bounce straight back.
    #[must_use]
    pub fn clamp_point(&self, p: LogicalPoint, inset: f64) -> LogicalPoint {
        let inset = inset.max(f64::EPSILON);
        let max_x = (self.max_x() - inset).max(self.min_x());
        let max_y = (self.max_y() - inset).max(self.min_y());
        LogicalPoint::new(
            p.x.clamp(self.min_x(), max_x),
            p.y.clamp(self.min_y(), max_y),
        )
    }

    /// Returns the smallest rectangle containing both inputs.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let min_x = self.min_x().min(other.min_x());
        let min_y = self.min_y().min(other.min_y());
        let max_x = self.max_x().max(other.max_x());
        let max_y = self.max_y().max(other.max_y());
        Self::new(
            LogicalPoint::new(min_x, min_y),
            LogicalSize {
                width: max_x - min_x,
                height: max_y - min_y,
            },
        )
    }
}

impl fmt::Display for LogicalRect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:.0},{:.0} {:.0}x{:.0}]",
            self.origin.x, self.origin.y, self.size.width, self.size.height
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
        LogicalRect::from_parts(x, y, w, h).unwrap()
    }

    #[test]
    fn degenerate_sizes_are_rejected() {
        assert!(LogicalSize::new(0.0, 100.0).is_none());
        assert!(LogicalSize::new(-1.0, 100.0).is_none());
        assert!(LogicalSize::new(f64::NAN, 100.0).is_none());
        assert!(LogicalSize::new(f64::INFINITY, 100.0).is_none());
        assert!(LogicalSize::new(1920.0, 1080.0).is_some());
    }

    #[test]
    fn scale_rejects_degenerate_factors() {
        assert!(Scale::new(0.0).is_none());
        assert!(Scale::new(-2.0).is_none());
        assert!(Scale::new(f64::NAN).is_none());
        assert_eq!(Scale::new(2.0).unwrap().get(), 2.0);
    }

    #[test]
    fn containment_is_half_open() {
        let r = rect(0.0, 0.0, 100.0, 100.0);
        assert!(
            r.contains(LogicalPoint::new(0.0, 0.0)),
            "owns its minimum corner"
        );
        assert!(r.contains(LogicalPoint::new(99.999, 99.999)));
        assert!(
            !r.contains(LogicalPoint::new(100.0, 50.0)),
            "excludes max_x"
        );
        assert!(
            !r.contains(LogicalPoint::new(50.0, 100.0)),
            "excludes max_y"
        );
    }

    #[test]
    fn adjacent_screens_never_both_claim_a_point() {
        // The whole point of half-open containment: a shared seam belongs to
        // exactly one screen, so the cursor cannot oscillate across it.
        let left = rect(0.0, 0.0, 1920.0, 1080.0);
        let right = rect(1920.0, 0.0, 1920.0, 1080.0);
        let seam = LogicalPoint::new(1920.0, 500.0);
        assert!(!left.contains(seam));
        assert!(right.contains(seam));
        assert!(!left.intersects(&right), "touching is not overlapping");
    }

    #[test]
    fn normalize_round_trips() {
        let r = rect(100.0, 200.0, 800.0, 600.0);
        let p = LogicalPoint::new(500.0, 350.0);
        let (nx, ny) = r.normalize(p);
        assert!((nx - 0.5).abs() < EPS);
        assert!((ny - 0.25).abs() < EPS);
        let back = r.denormalize(nx, ny);
        assert!((back.x - p.x).abs() < EPS);
        assert!((back.y - p.y).abs() < EPS);
    }

    #[test]
    fn normalize_maps_across_mismatched_screens() {
        // A 4K panel handing off to a small laptop must land proportionally,
        // not at a raw pixel coordinate that would be off-screen.
        let big = rect(0.0, 0.0, 3840.0, 2160.0);
        let small = rect(3840.0, 0.0, 1280.0, 800.0);
        let (_, ny) = big.normalize(LogicalPoint::new(3839.0, 1080.0));
        let landed = small.denormalize(0.0, ny);
        assert!((landed.y - 400.0).abs() < 1.0);
        assert!(small.contains(landed));
    }

    #[test]
    fn clamp_point_stays_inside_half_open_bounds() {
        let r = rect(0.0, 0.0, 100.0, 100.0);
        let clamped = r.clamp_point(LogicalPoint::new(500.0, 500.0), 0.5);
        assert!(r.contains(clamped), "clamped point must satisfy contains()");
        assert!(clamped.x < r.max_x());
        assert!(clamped.y < r.max_y());
    }

    #[test]
    fn overlap_span_detects_shared_edges() {
        let a = rect(0.0, 0.0, 1920.0, 1080.0);
        // Vertically offset neighbour: shares only part of the vertical edge.
        let b = rect(1920.0, 500.0, 1920.0, 1080.0);
        let span = a.overlap_span(&b, false).unwrap();
        assert!((span.0 - 500.0).abs() < EPS);
        assert!((span.1 - 1080.0).abs() < EPS);

        // Fully disjoint on the vertical axis: no shared edge to cross.
        let c = rect(1920.0, 5000.0, 1920.0, 1080.0);
        assert!(a.overlap_span(&c, false).is_none());
    }

    #[test]
    fn edge_opposite_is_involutive() {
        for e in Edge::ALL {
            assert_eq!(e.opposite().opposite(), e);
            assert_ne!(e.opposite(), e);
        }
    }

    #[test]
    fn union_covers_both() {
        let a = rect(0.0, 0.0, 100.0, 100.0);
        let b = rect(200.0, 50.0, 100.0, 100.0);
        let u = a.union(&b);
        assert_eq!(u.min_x(), 0.0);
        assert_eq!(u.max_x(), 300.0);
        assert_eq!(u.min_y(), 0.0);
        assert_eq!(u.max_y(), 150.0);
    }
}
