//! Snapping screens together while the user drags them.
//!
//! # Why snapping is not cosmetic
//!
//! A layout only works if screens actually touch. A one-point gap between two
//! rectangles is invisible on a scaled-down editor canvas and completely breaks
//! crossing: the cursor reaches the edge, finds nothing adjacent, and stops. The
//! user sees a layout that looks right and a pointer that will not move between
//! machines, with nothing on screen to explain the difference.
//!
//! So the editor does not let a near-miss happen. A block dragged within
//! [`SNAP_DISTANCE`] of another is placed flush against it, and edges that nearly
//! line up are aligned exactly.
//!
//! # Why this is here and not in the interface
//!
//! It is geometry, and geometry belongs where it can be tested. The editor
//! reports where the user dropped something; this decides where it goes.

use mb_types::{LogicalPoint, LogicalRect};

/// How close a dragged block must come before it snaps, in shared points.
///
/// Generous, because the editor canvas is scaled down — often by a factor of
/// twenty — so a few pixels of mouse movement is a large distance in shared
/// space. Too small a threshold makes snapping feel unreliable, which teaches
/// users to nudge repeatedly rather than trust it.
pub const SNAP_DISTANCE: f64 = 120.0;

/// Where a dragged block should come to rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapResult {
    /// The position to use.
    pub origin: LogicalPoint,
    /// Whether the block snapped to something rather than staying where dropped.
    pub snapped: bool,
}

/// Places a dragged block, snapping it flush to a neighbour where possible.
///
/// `dragged` is where the user has it now; `others` are the blocks already
/// placed. The nearest valid snap wins, so dragging between two screens attaches
/// to whichever the user is closer to rather than to whichever happens to be
/// first in the list.
#[must_use]
pub fn snap(dragged: LogicalRect, others: &[LogicalRect]) -> SnapResult {
    let mut best: Option<(f64, LogicalPoint)> = None;

    for other in others {
        for candidate in candidates(dragged, *other) {
            // Snapping onto a position that overlaps an existing screen would
            // produce a layout the topology refuses to build.
            let placed = LogicalRect::new(candidate, dragged.size);
            if others.iter().any(|o| placed.intersects(o)) {
                continue;
            }

            let distance = (candidate.x - dragged.origin.x).hypot(candidate.y - dragged.origin.y);
            if distance <= SNAP_DISTANCE && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, candidate));
            }
        }
    }

    best.map_or(
        SnapResult {
            origin: dragged.origin,
            snapped: false,
        },
        |(_, origin)| SnapResult {
            origin,
            snapped: true,
        },
    )
}

/// The positions at which `dragged` would sit flush against `other`.
///
/// Four sides, each with three alignments: leading edges level, trailing edges
/// level, and centres level. Offering all three is what lets a small screen be
/// placed against a large one wherever the user actually means.
fn candidates(dragged: LogicalRect, other: LogicalRect) -> Vec<LogicalPoint> {
    let (w, h) = (dragged.size.width, dragged.size.height);

    let vertical_alignments = [
        other.min_y(),
        other.max_y() - h,
        other.min_y() + (other.size.height - h) / 2.0,
    ];
    let horizontal_alignments = [
        other.min_x(),
        other.max_x() - w,
        other.min_x() + (other.size.width - w) / 2.0,
    ];

    let mut out = Vec::with_capacity(12);
    for y in vertical_alignments {
        out.push(LogicalPoint::new(other.max_x(), y)); // to the right
        out.push(LogicalPoint::new(other.min_x() - w, y)); // to the left
    }
    for x in horizontal_alignments {
        out.push(LogicalPoint::new(x, other.max_y())); // below
        out.push(LogicalPoint::new(x, other.min_y() - h)); // above
    }
    out
}

/// True when a block overlaps any of the others.
///
/// [`snap`] never *produces* an overlap, but it does not prevent one either: a
/// block dropped far from everything stays where it was put, and that may be on
/// top of something. The editor needs to know, so it can show the arrangement as
/// invalid rather than letting the user save a layout the engine will refuse.
#[must_use]
pub fn overlaps_any(block: LogicalRect, others: &[LogicalRect]) -> bool {
    others.iter().any(|other| block.intersects(other))
}

/// Shifts every block so the arrangement's top-left corner sits at the origin.
///
/// Keeps coordinates from drifting ever further from zero as the user rearranges
/// screens, which would eventually make the saved layout hard to read and the
/// canvas scaling awkward.
#[must_use]
pub fn normalise(blocks: &[LogicalRect]) -> Vec<LogicalRect> {
    if blocks.is_empty() {
        return Vec::new();
    }
    let min_x = blocks
        .iter()
        .map(LogicalRect::min_x)
        .fold(f64::MAX, f64::min);
    let min_y = blocks
        .iter()
        .map(LogicalRect::min_y)
        .fold(f64::MAX, f64::min);

    blocks
        .iter()
        .map(|b| {
            LogicalRect::new(
                LogicalPoint::new(b.origin.x - min_x, b.origin.y - min_y),
                b.size,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
        LogicalRect::from_parts(x, y, w, h).expect("valid")
    }

    #[test]
    fn a_near_miss_snaps_flush() {
        // The whole point. A gap this small is invisible on a scaled canvas and
        // completely breaks crossing.
        let anchor = rect(0.0, 0.0, 1920.0, 1080.0);
        let dragged = rect(1923.0, 4.0, 1920.0, 1080.0);

        let result = snap(dragged, &[anchor]);
        assert!(result.snapped);
        assert_eq!(result.origin, LogicalPoint::new(1920.0, 0.0));
    }

    #[test]
    fn a_flush_position_is_left_alone() {
        let anchor = rect(0.0, 0.0, 1920.0, 1080.0);
        let dragged = rect(1920.0, 0.0, 1920.0, 1080.0);

        let result = snap(dragged, &[anchor]);
        assert_eq!(result.origin, LogicalPoint::new(1920.0, 0.0));
    }

    #[test]
    fn a_distant_block_is_left_where_it_was_dropped() {
        // Snapping from across the canvas would fight the user.
        let anchor = rect(0.0, 0.0, 1920.0, 1080.0);
        let dragged = rect(9000.0, 9000.0, 1920.0, 1080.0);

        let result = snap(dragged, &[anchor]);
        assert!(!result.snapped);
        assert_eq!(result.origin, dragged.origin);
    }

    #[test]
    fn snapping_works_on_every_side() {
        let anchor = rect(0.0, 0.0, 1000.0, 1000.0);

        for (dropped, expected) in [
            (
                rect(1020.0, 10.0, 500.0, 500.0),
                LogicalPoint::new(1000.0, 0.0),
            ),
            (
                rect(-520.0, 10.0, 500.0, 500.0),
                LogicalPoint::new(-500.0, 0.0),
            ),
            (
                rect(10.0, 1020.0, 500.0, 500.0),
                LogicalPoint::new(0.0, 1000.0),
            ),
            (
                rect(10.0, -520.0, 500.0, 500.0),
                LogicalPoint::new(0.0, -500.0),
            ),
        ] {
            let result = snap(dropped, &[anchor]);
            assert!(result.snapped, "did not snap from {dropped:?}");
            assert_eq!(result.origin, expected, "from {dropped:?}");
        }
    }

    #[test]
    fn a_small_screen_can_align_to_either_end_or_the_centre() {
        // Offering only one alignment would make it impossible to place a laptop
        // panel where the user actually means it against a large monitor.
        let anchor = rect(0.0, 0.0, 2000.0, 2000.0);

        let top = snap(rect(2010.0, 20.0, 500.0, 500.0), &[anchor]);
        assert_eq!(top.origin, LogicalPoint::new(2000.0, 0.0));

        let bottom = snap(rect(2010.0, 1480.0, 500.0, 500.0), &[anchor]);
        assert_eq!(bottom.origin, LogicalPoint::new(2000.0, 1500.0));

        let centre = snap(rect(2010.0, 760.0, 500.0, 500.0), &[anchor]);
        assert_eq!(centre.origin, LogicalPoint::new(2000.0, 750.0));
    }

    #[test]
    fn the_nearest_snap_wins() {
        // Dragging between two screens must attach to whichever the user is
        // closer to, not to whichever happens to be first in the list.
        let left = rect(0.0, 0.0, 1000.0, 1000.0);
        let right = rect(2000.0, 0.0, 1000.0, 1000.0);
        // Sixty points from resting against `right`, five hundred from `left`.
        let dragged = rect(1560.0, 0.0, 500.0, 1000.0);

        let result = snap(dragged, &[left, right]);
        assert!(result.snapped);
        assert_eq!(result.origin, LogicalPoint::new(1500.0, 0.0));
    }

    #[test]
    fn snapping_never_produces_an_overlap() {
        // Overlapping screens produce a layout the topology refuses to build.
        let left = rect(0.0, 0.0, 1000.0, 1000.0);
        let right = rect(1000.0, 0.0, 1000.0, 1000.0);
        // Dropped just past the seam, where snapping right from `left` would
        // land on top of `right`.
        let dragged = rect(1010.0, 10.0, 1000.0, 1000.0);

        let result = snap(dragged, &[left, right]);
        if result.snapped {
            let placed = LogicalRect::new(result.origin, dragged.size);
            assert!(
                !overlaps_any(placed, &[left, right]),
                "snapped into an overlap at {:?}",
                result.origin
            );
        }
    }

    #[test]
    fn a_drop_with_nowhere_to_snap_stays_put_and_is_flagged() {
        // `snap` does not rescue a bad drop; it reports one. The editor shows the
        // arrangement as invalid rather than letting the user save a layout the
        // engine will refuse.
        let left = rect(0.0, 0.0, 1000.0, 1000.0);
        let dragged = rect(500.0, 500.0, 1000.0, 1000.0);

        let result = snap(dragged, &[left]);
        assert!(!result.snapped, "found a snap that does not exist");

        let placed = LogicalRect::new(result.origin, dragged.size);
        assert!(
            overlaps_any(placed, &[left]),
            "the overlap must be detectable by the caller"
        );
    }

    #[test]
    fn a_valid_arrangement_reports_no_overlap() {
        let left = rect(0.0, 0.0, 1000.0, 1000.0);
        let right = rect(1000.0, 0.0, 1000.0, 1000.0);
        assert!(!overlaps_any(right, &[left]), "touching is not overlapping");
    }

    #[test]
    fn the_first_screen_has_nothing_to_snap_to() {
        let dragged = rect(500.0, 500.0, 1920.0, 1080.0);
        let result = snap(dragged, &[]);
        assert!(!result.snapped);
        assert_eq!(result.origin, dragged.origin);
    }

    #[test]
    fn normalising_moves_the_arrangement_to_the_origin() {
        // Otherwise coordinates drift further from zero on every rearrangement.
        let blocks = vec![
            rect(-500.0, 300.0, 100.0, 100.0),
            rect(700.0, -200.0, 100.0, 100.0),
        ];
        let normalised = normalise(&blocks);

        assert_eq!(normalised[0].origin, LogicalPoint::new(0.0, 500.0));
        assert_eq!(normalised[1].origin, LogicalPoint::new(1200.0, 0.0));
    }

    #[test]
    fn normalising_preserves_relative_positions() {
        let blocks = vec![
            rect(100.0, 100.0, 50.0, 50.0),
            rect(300.0, 400.0, 50.0, 50.0),
        ];
        let normalised = normalise(&blocks);

        let before = (
            blocks[1].min_x() - blocks[0].min_x(),
            blocks[1].min_y() - blocks[0].min_y(),
        );
        let after = (
            normalised[1].min_x() - normalised[0].min_x(),
            normalised[1].min_y() - normalised[0].min_y(),
        );
        assert_eq!(before, after);
    }

    #[test]
    fn normalising_nothing_is_harmless() {
        assert!(normalise(&[]).is_empty());
    }

    #[test]
    fn a_snapped_arrangement_builds_a_valid_layout() {
        // The end-to-end statement: what the editor produces must be something
        // the topology engine accepts.
        use crate::layout::{Layout, PlacedScreen};
        use mb_types::{DeviceId, GlobalScreenId, Scale, ScreenId};

        let anchor = rect(0.0, 0.0, 1920.0, 1080.0);
        let dropped = rect(1917.0, 6.0, 1280.0, 800.0);
        let result = snap(dropped, &[anchor]);

        let blocks = normalise(&[anchor, LogicalRect::new(result.origin, dropped.size)]);
        let screens: Vec<PlacedScreen> = blocks
            .iter()
            .enumerate()
            .map(|(index, bounds)| PlacedScreen {
                id: GlobalScreenId::new(
                    DeviceId::from_bytes([u8::try_from(index).unwrap_or(0); 32]),
                    ScreenId(0),
                ),
                bounds: *bounds,
                scale: Scale::ONE,
            })
            .collect();

        let layout = Layout::new(screens).expect("the editor produced an invalid layout");
        assert!(
            layout.are_adjacent(layout.screens()[0].id, layout.screens()[1].id),
            "the snapped screens are not adjacent, so the cursor cannot cross"
        );
    }
}
