//! The arrangement of screens in the shared virtual space.

use mb_types::{Edge, GlobalScreenId, LogicalPoint, LogicalRect, Scale};
use serde::{Deserialize, Serialize};

/// One screen, placed in the shared virtual space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedScreen {
    /// Which device's screen this is.
    pub id: GlobalScreenId,
    /// Position and size in the shared virtual space.
    pub bounds: LogicalRect,
    /// Backing scale factor, carried for the injecting side's benefit.
    pub scale: Scale,
}

/// Why a layout was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// The layout contained no screens.
    ///
    /// Reachable while every display is asleep. The caller must suspend edge
    /// detection rather than divide by a zero-sized desktop.
    #[error("a layout must contain at least one screen")]
    Empty,
    /// Two screens claimed the same identifier.
    #[error("screen {id} appears more than once")]
    Duplicate {
        /// The repeated identifier.
        id: String,
    },
    /// Two screens overlap in area.
    ///
    /// Rejected because a cursor position inside the overlap would belong to two
    /// screens at once, and which one wins would depend on iteration order.
    #[error("screens {a} and {b} overlap; screens may touch but not overlap")]
    Overlap {
        /// First screen.
        a: String,
        /// Second screen.
        b: String,
    },
}

/// A validated arrangement of screens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    screens: Vec<PlacedScreen>,
}

impl Layout {
    /// Validates and builds a layout.
    ///
    /// # Errors
    ///
    /// See [`LayoutError`]. Validation happens once, here, so the hot path can
    /// assume a coherent arrangement and skip the checks entirely.
    pub fn new(screens: Vec<PlacedScreen>) -> Result<Self, LayoutError> {
        if screens.is_empty() {
            return Err(LayoutError::Empty);
        }

        for (index, screen) in screens.iter().enumerate() {
            for other in &screens[index + 1..] {
                if screen.id == other.id {
                    return Err(LayoutError::Duplicate {
                        id: screen.id.to_string(),
                    });
                }
                if screen.bounds.intersects(&other.bounds) {
                    return Err(LayoutError::Overlap {
                        a: screen.id.to_string(),
                        b: other.id.to_string(),
                    });
                }
            }
        }

        Ok(Self { screens })
    }

    /// The screens, in the order given.
    #[must_use]
    pub fn screens(&self) -> &[PlacedScreen] {
        &self.screens
    }

    /// Looks up a screen.
    #[must_use]
    pub fn get(&self, id: GlobalScreenId) -> Option<&PlacedScreen> {
        self.screens.iter().find(|s| s.id == id)
    }

    /// Finds the screen containing a point.
    ///
    /// Containment is half-open, so a point on a shared seam belongs to exactly
    /// one screen — which is what stops the cursor oscillating there.
    #[must_use]
    pub fn screen_at(&self, point: LogicalPoint) -> Option<&PlacedScreen> {
        self.screens.iter().find(|s| s.bounds.contains(point))
    }

    /// Finds the first screen other than `from` that the segment enters.
    ///
    /// Used when the destination point lands in a gap between screens: a
    /// straight-line move from inside one screen to outside every screen may
    /// still have passed through a neighbour. Returns the screen entered
    /// earliest along the path, so a fast diagonal movement lands on the screen
    /// it actually crossed into rather than one further along.
    #[must_use]
    pub fn first_screen_along(
        &self,
        from_point: LogicalPoint,
        to_point: LogicalPoint,
        exclude: GlobalScreenId,
    ) -> Option<(&PlacedScreen, f64)> {
        let dx = to_point.x - from_point.x;
        let dy = to_point.y - from_point.y;

        let mut best: Option<(&PlacedScreen, f64)> = None;
        for screen in &self.screens {
            if screen.id == exclude {
                continue;
            }
            if let Some(t) = segment_enters(from_point, dx, dy, &screen.bounds)
                && best.is_none_or(|(_, best_t)| t < best_t)
            {
                best = Some((screen, t));
            }
        }
        best
    }

    /// The smallest rectangle covering every screen.
    #[must_use]
    pub fn bounding_box(&self) -> LogicalRect {
        let mut iter = self.screens.iter();
        let first = iter
            .next()
            .map_or_else(|| unreachable!("a layout is never empty"), |s| s.bounds);
        iter.fold(first, |acc, s| acc.union(&s.bounds))
    }

    /// Whether the two screens share an edge long enough to cross through.
    ///
    /// Screens that merely touch at a corner are not crossable: the shared span
    /// is a single point, and a cursor could only pass through it by landing on
    /// one exact coordinate.
    #[must_use]
    pub fn are_adjacent(&self, a: GlobalScreenId, b: GlobalScreenId) -> bool {
        let (Some(a), Some(b)) = (self.get(a), self.get(b)) else {
            return false;
        };
        let horizontal_touch = (a.bounds.max_x() - b.bounds.min_x()).abs() < f64::EPSILON
            || (b.bounds.max_x() - a.bounds.min_x()).abs() < f64::EPSILON;
        let vertical_touch = (a.bounds.max_y() - b.bounds.min_y()).abs() < f64::EPSILON
            || (b.bounds.max_y() - a.bounds.min_y()).abs() < f64::EPSILON;

        (horizontal_touch && a.bounds.overlap_span(&b.bounds, false).is_some())
            || (vertical_touch && a.bounds.overlap_span(&b.bounds, true).is_some())
    }
}

/// Returns the fraction along a segment at which it enters a rectangle.
///
/// The slab method: clip the segment against each axis in turn and see whether
/// an interval survives. Returns `None` when the segment misses the rectangle
/// entirely, or when it starts inside it.
fn segment_enters(from: LogicalPoint, dx: f64, dy: f64, rect: &LogicalRect) -> Option<f64> {
    if rect.contains(from) {
        return None;
    }

    let mut enter = 0.0_f64;
    let mut exit = 1.0_f64;

    for (origin, delta, min, max) in [
        (from.x, dx, rect.min_x(), rect.max_x()),
        (from.y, dy, rect.min_y(), rect.max_y()),
    ] {
        if delta.abs() < f64::EPSILON {
            // Parallel to this axis: the segment can only hit the rectangle if
            // it already lies within the slab.
            if origin < min || origin >= max {
                return None;
            }
            continue;
        }
        let t1 = (min - origin) / delta;
        let t2 = (max - origin) / delta;
        let (near, far) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
        enter = enter.max(near);
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }

    (enter <= exit && (0.0..=1.0).contains(&enter)).then_some(enter)
}

/// Which edge of `rect` a point lies beyond, if any.
///
/// When a point is outside on two axes at once — past a corner — the axis it
/// exceeded by more is reported, because that is the direction the user was
/// predominantly moving.
#[must_use]
pub fn exited_edge(rect: &LogicalRect, point: LogicalPoint) -> Option<Edge> {
    let left = rect.min_x() - point.x;
    let right = point.x - rect.max_x();
    let top = rect.min_y() - point.y;
    let bottom = point.y - rect.max_y();

    let candidates = [
        (Edge::Left, left),
        (Edge::Right, right),
        (Edge::Top, top),
        (Edge::Bottom, bottom),
    ];

    candidates
        .into_iter()
        .filter(|(_, overshoot)| *overshoot > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _)| edge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_types::{DeviceId, ScreenId};

    fn id(device: u8, screen: u32) -> GlobalScreenId {
        GlobalScreenId::new(DeviceId::from_bytes([device; 32]), ScreenId(screen))
    }

    fn screen(device: u8, index: u32, x: f64, y: f64, w: f64, h: f64) -> PlacedScreen {
        PlacedScreen {
            id: id(device, index),
            bounds: LogicalRect::from_parts(x, y, w, h).expect("valid"),
            scale: Scale::ONE,
        }
    }

    /// A Mac on the left, a PC on the right, sharing a full edge.
    fn side_by_side() -> Layout {
        Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 0, 1920.0, 0.0, 1920.0, 1080.0),
        ])
        .expect("valid layout")
    }

    #[test]
    fn an_empty_layout_is_rejected() {
        // Reachable while every display is asleep; the caller must suspend edge
        // detection rather than work with a zero-sized desktop.
        assert_eq!(Layout::new(vec![]), Err(LayoutError::Empty));
    }

    #[test]
    fn duplicate_screens_are_rejected() {
        let layout = Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 100.0, 100.0),
            screen(1, 0, 500.0, 0.0, 100.0, 100.0),
        ]);
        assert!(matches!(layout, Err(LayoutError::Duplicate { .. })));
    }

    #[test]
    fn overlapping_screens_are_rejected() {
        // A point inside the overlap would belong to two screens, and which won
        // would depend on iteration order.
        let layout = Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 0, 1000.0, 0.0, 1920.0, 1080.0),
        ]);
        assert!(matches!(layout, Err(LayoutError::Overlap { .. })));
    }

    #[test]
    fn touching_screens_are_allowed() {
        // The normal arrangement. Touching is not overlapping.
        assert!(
            Layout::new(vec![
                screen(1, 0, 0.0, 0.0, 1920.0, 1080.0),
                screen(2, 0, 1920.0, 0.0, 1920.0, 1080.0),
            ])
            .is_ok()
        );
    }

    #[test]
    fn a_seam_belongs_to_exactly_one_screen() {
        let layout = side_by_side();
        let seam = LogicalPoint::new(1920.0, 500.0);
        assert_eq!(layout.screen_at(seam).map(|s| s.id), Some(id(2, 0)));
        assert_eq!(
            layout
                .screen_at(LogicalPoint::new(1919.99, 500.0))
                .map(|s| s.id),
            Some(id(1, 0))
        );
    }

    #[test]
    fn adjacency_requires_a_shared_span_not_a_shared_corner() {
        let layout = side_by_side();
        assert!(layout.are_adjacent(id(1, 0), id(2, 0)));

        // Diagonally placed: they touch at exactly one point, which a cursor
        // could only pass through by landing on one exact coordinate.
        let corner = Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 100.0, 100.0),
            screen(2, 0, 100.0, 100.0, 100.0, 100.0),
        ])
        .expect("valid");
        assert!(!corner.are_adjacent(id(1, 0), id(2, 0)));
    }

    #[test]
    fn a_segment_into_a_neighbour_is_detected() {
        let layout = side_by_side();
        let entered = layout.first_screen_along(
            LogicalPoint::new(1900.0, 500.0),
            LogicalPoint::new(1950.0, 500.0),
            id(1, 0),
        );
        assert_eq!(entered.map(|(s, _)| s.id), Some(id(2, 0)));
    }

    #[test]
    fn a_segment_that_misses_every_screen_finds_nothing() {
        let layout = side_by_side();
        let entered = layout.first_screen_along(
            LogicalPoint::new(100.0, 100.0),
            LogicalPoint::new(100.0, -500.0),
            id(1, 0),
        );
        assert!(entered.is_none());
    }

    #[test]
    fn the_nearest_screen_along_the_path_wins() {
        // A fast diagonal must land on the screen it actually crossed into, not
        // one further along the same line.
        let layout = Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 100.0, 100.0),
            screen(2, 0, 100.0, 0.0, 100.0, 100.0),
            screen(3, 0, 200.0, 0.0, 100.0, 100.0),
        ])
        .expect("valid");

        let entered = layout.first_screen_along(
            LogicalPoint::new(50.0, 50.0),
            LogicalPoint::new(250.0, 50.0),
            id(1, 0),
        );
        assert_eq!(entered.map(|(s, _)| s.id), Some(id(2, 0)));
    }

    #[test]
    fn a_gap_between_screens_is_still_crossable_in_one_move() {
        // Users do not align screens perfectly in the editor. A movement long
        // enough to span a small gap should still land.
        let layout = Layout::new(vec![
            screen(1, 0, 0.0, 0.0, 100.0, 100.0),
            screen(2, 0, 110.0, 0.0, 100.0, 100.0),
        ])
        .expect("valid");

        let entered = layout.first_screen_along(
            LogicalPoint::new(95.0, 50.0),
            LogicalPoint::new(130.0, 50.0),
            id(1, 0),
        );
        assert_eq!(entered.map(|(s, _)| s.id), Some(id(2, 0)));
    }

    #[test]
    fn exited_edge_reports_the_dominant_axis_past_a_corner() {
        let rect = LogicalRect::from_parts(0.0, 0.0, 100.0, 100.0).expect("valid");
        assert_eq!(
            exited_edge(&rect, LogicalPoint::new(150.0, 50.0)),
            Some(Edge::Right)
        );
        assert_eq!(
            exited_edge(&rect, LogicalPoint::new(-10.0, 50.0)),
            Some(Edge::Left)
        );
        assert_eq!(
            exited_edge(&rect, LogicalPoint::new(50.0, -30.0)),
            Some(Edge::Top)
        );
        assert_eq!(
            exited_edge(&rect, LogicalPoint::new(50.0, 130.0)),
            Some(Edge::Bottom)
        );

        // Past the bottom-right corner, further out horizontally.
        assert_eq!(
            exited_edge(&rect, LogicalPoint::new(200.0, 110.0)),
            Some(Edge::Right)
        );
        // Inside: no edge.
        assert_eq!(exited_edge(&rect, LogicalPoint::new(50.0, 50.0)), None);
    }

    #[test]
    fn bounding_box_spans_every_screen() {
        let bb = side_by_side().bounding_box();
        assert_eq!(bb.min_x(), 0.0);
        assert_eq!(bb.max_x(), 3840.0);
    }
}
