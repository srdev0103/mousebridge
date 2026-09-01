//! Converting between a device's own cursor space and the shared virtual space.
//!
//! # The problem this solves
//!
//! The two platforms describe a screen in different units, and neither can be
//! used directly as a shared one:
//!
//! * **macOS** reports bounds in logical points that already account for the
//!   user's display-scaling choice. A 5K panel set to "looks like 2560×1440"
//!   reports 2560×1440, with a backing scale factor of 2.0 describing the
//!   *pixels behind* those points.
//! * **Windows** reports bounds in physical pixels of the virtual desktop, with
//!   a separate effective DPI. A 3840-wide monitor at 150% reports 3840, and the
//!   user sees the equivalent of 2560.
//!
//! So a "1920" from one machine and a "1920" from the other mean different
//! amounts of screen. Placing them side by side without conversion produces a
//! layout where one machine's screens are visibly the wrong size relative to the
//! other's, and where the cursor changes speed as it crosses.
//!
//! # The shared unit
//!
//! Screens are placed in the shared space in **logical points at 96 DPI**: what
//! the user perceives, normalised. macOS bounds are already that unit;
//! Windows bounds are divided by the effective scale factor.
//!
//! # Why the native space is kept
//!
//! Injection needs it. `SendInput` normalises against the virtual desktop in
//! physical pixels, and `CGWarpMouseCursorPosition` takes global display points.
//! Discarding the native space and reconstructing it would introduce rounding
//! error at exactly the place where a one-pixel mistake means the cursor cannot
//! reach the last column — the column an edge crossing happens in.
//!
//! See `docs/adr/0001-display-coordinate-space.md`.

use mb_types::{GlobalScreenId, LogicalPoint, LogicalRect, LogicalSize, OsKind, Scale};
use serde::{Deserialize, Serialize};

/// One screen, in both the unit its device uses and the shared unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenMapping {
    /// Which device's screen this is.
    pub id: GlobalScreenId,
    /// Bounds in the device's own cursor space, exactly as the OS reports them.
    ///
    /// What injection uses. Never converted, never rounded.
    pub native: LogicalRect,
    /// Size in shared units: logical points at 96 DPI.
    pub logical_size: LogicalSize,
    /// The device's own scale factor for this screen.
    pub scale: Scale,
}

impl ScreenMapping {
    /// Builds a mapping from what a platform backend reported.
    ///
    /// The conversion is per-platform because the platforms disagree about what
    /// their bounds mean — see the module documentation.
    #[must_use]
    pub fn from_native(id: GlobalScreenId, native: LogicalRect, scale: Scale, os: OsKind) -> Self {
        let logical_size = match os {
            // Already logical points; the scale factor describes the backing
            // store, not the size the user perceives.
            OsKind::MacOs => native.size,
            // Physical pixels; divide by the effective scale to get what the
            // user perceives.
            OsKind::Windows | OsKind::Unknown => LogicalSize::new(
                native.size.width / scale.get(),
                native.size.height / scale.get(),
            )
            .unwrap_or(native.size),
        };

        Self {
            id,
            native,
            logical_size,
            scale,
        }
    }

    /// Converts a point in the shared space to this screen's native space.
    ///
    /// `placed` is where this screen sits in the shared layout. The conversion is
    /// proportional rather than a fixed offset, because the shared rectangle and
    /// the native rectangle can be different sizes.
    #[must_use]
    pub fn to_native(&self, placed: &LogicalRect, shared_point: LogicalPoint) -> LogicalPoint {
        let (nx, ny) = placed.normalize(shared_point);
        let point = self.native.denormalize(nx, ny);
        // Clamped so a point on the shared rectangle's exclusive far edge lands
        // on the last addressable native coordinate rather than one past it.
        self.native.clamp_point(point, 0.5)
    }

    /// Converts a point in this screen's native space to the shared space.
    #[must_use]
    pub fn to_shared(&self, placed: &LogicalRect, native_point: LogicalPoint) -> LogicalPoint {
        let (nx, ny) = self.native.normalize(native_point);
        placed.clamp_point(placed.denormalize(nx, ny), 0.5)
    }
}

/// All of one device's screens, in that device's own arrangement.
///
/// Kept as a group because a device's monitors have a fixed relationship to each
/// other that the user set up in their OS. The shared layout may move the group,
/// but must never rearrange within it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceScreens {
    /// The screens, in the platform layer's stable order.
    pub screens: Vec<ScreenMapping>,
}

impl DeviceScreens {
    /// Builds a group from a device's screens.
    #[must_use]
    pub const fn new(screens: Vec<ScreenMapping>) -> Self {
        Self { screens }
    }

    /// True when the device reported no screens.
    ///
    /// Reachable while every display is asleep, and the caller must treat it as
    /// a transient state rather than a device with nowhere to put the cursor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.screens.is_empty()
    }

    /// The bounding box of this device's screens, in shared units.
    ///
    /// Computed from the *native* arrangement scaled per screen, so a mixed-DPI
    /// setup keeps its relative geometry.
    #[must_use]
    pub fn shared_extent(&self) -> Option<LogicalSize> {
        let rects = self.shared_rects()?;
        let mut iter = rects.iter();
        let first = *iter.next()?;
        let union = iter.fold(first, |acc, r| acc.union(r));
        Some(union.size)
    }

    /// Places this device's screens as a block at `origin` in the shared space.
    ///
    /// Relative positions are preserved: the block is translated, never reflowed.
    /// A user who put their laptop below their monitor sees that arrangement on
    /// every machine.
    #[must_use]
    pub fn place_at(&self, origin: LogicalPoint) -> Vec<crate::layout::PlacedScreen> {
        let Some(rects) = self.shared_rects() else {
            return Vec::new();
        };

        rects
            .iter()
            .zip(&self.screens)
            .map(|(rect, mapping)| crate::layout::PlacedScreen {
                id: mapping.id,
                bounds: LogicalRect::new(
                    LogicalPoint::new(origin.x + rect.origin.x, origin.y + rect.origin.y),
                    rect.size,
                ),
                scale: mapping.scale,
            })
            .collect()
    }

    /// The device's screens converted to shared units, normalised so the
    /// top-left of the arrangement sits at the origin.
    ///
    /// # Why the arrangement is rebuilt rather than transformed
    ///
    /// The obvious approach — scale each screen's origin by its own factor,
    /// alongside its size — does not work, and fails in exactly the way ADR 0001
    /// predicts. Consider a 150% laptop panel at native `0..2880` beside a 100%
    /// monitor at native `2880..4800`. They touch. Converting independently gives
    /// `0..1920` and `2880..4800`: a 960-point gap opens between screens that are
    /// physically adjacent, because the second screen's origin was scaled by its
    /// own ratio when the distance it describes was spanned by the *first*
    /// screen's pixels.
    ///
    /// So the arrangement is reconstructed instead. Starting from one screen,
    /// each neighbour is placed flush against an already-placed screen using the
    /// converted sizes. Adjacency in the native arrangement becomes adjacency in
    /// the shared one by construction, whatever the scale factors involved.
    fn shared_rects(&self) -> Option<Vec<LogicalRect>> {
        if self.screens.is_empty() {
            return None;
        }

        let mut placed: Vec<Option<LogicalRect>> = vec![None; self.screens.len()];

        // Anchor the first screen at the origin. Which screen anchors the block
        // does not matter: the whole thing is translated to the corner afterwards.
        placed[0] = Some(LogicalRect::new(
            LogicalPoint::ZERO,
            self.screens[0].logical_size,
        ));

        // Breadth-first over native adjacency, so every screen is positioned
        // relative to a neighbour rather than to a global origin.
        let mut frontier = vec![0usize];
        while let Some(anchor) = frontier.pop() {
            let Some(anchor_rect) = placed[anchor] else {
                continue;
            };
            let anchor_native = &self.screens[anchor].native;

            for (index, candidate) in self.screens.iter().enumerate() {
                if placed[index].is_some() {
                    continue;
                }
                let Some(rect) = place_against(
                    anchor_native,
                    &anchor_rect,
                    &candidate.native,
                    candidate.logical_size,
                ) else {
                    continue;
                };
                placed[index] = Some(rect);
                frontier.push(index);
            }
        }

        // Screens the walk never reached are not adjacent to anything — a
        // disconnected arrangement, which the OS should not produce but which a
        // stale saved layout can. Appending them keeps them reachable instead of
        // dropping them silently.
        let mut next_free_x = placed
            .iter()
            .flatten()
            .map(LogicalRect::max_x)
            .fold(0.0_f64, f64::max);
        for (index, slot) in placed.iter_mut().enumerate() {
            if slot.is_none() {
                let size = self.screens[index].logical_size;
                *slot = Some(LogicalRect::new(LogicalPoint::new(next_free_x, 0.0), size));
                next_free_x += size.width;
            }
        }

        let rects: Vec<LogicalRect> = placed.into_iter().flatten().collect();
        let min_x = rects
            .iter()
            .map(LogicalRect::min_x)
            .fold(f64::MAX, f64::min);
        let min_y = rects
            .iter()
            .map(LogicalRect::min_y)
            .fold(f64::MAX, f64::min);

        Some(
            rects
                .into_iter()
                .map(|r| {
                    LogicalRect::new(
                        LogicalPoint::new(r.origin.x - min_x, r.origin.y - min_y),
                        r.size,
                    )
                })
                .collect(),
        )
    }
}

/// Places `candidate` flush against `anchor`, if they touch in native space.
///
/// The offset *along* the shared edge is scaled by the anchor's ratio, which
/// keeps the alignment the user set up: two monitors whose tops line up natively
/// still line up afterwards.
fn place_against(
    anchor_native: &LogicalRect,
    anchor_shared: &LogicalRect,
    candidate_native: &LogicalRect,
    candidate_size: LogicalSize,
) -> Option<LogicalRect> {
    const TOUCH: f64 = 1.0;

    let ratio_x = anchor_shared.size.width / anchor_native.size.width;
    let ratio_y = anchor_shared.size.height / anchor_native.size.height;

    let overlaps_vertically = anchor_native
        .overlap_span(candidate_native, false)
        .is_some();
    let overlaps_horizontally = anchor_native.overlap_span(candidate_native, true).is_some();

    // To the right of the anchor.
    if overlaps_vertically && (candidate_native.min_x() - anchor_native.max_x()).abs() < TOUCH {
        let dy = (candidate_native.min_y() - anchor_native.min_y()) * ratio_y;
        return Some(LogicalRect::new(
            LogicalPoint::new(anchor_shared.max_x(), anchor_shared.min_y() + dy),
            candidate_size,
        ));
    }
    // To the left.
    if overlaps_vertically && (anchor_native.min_x() - candidate_native.max_x()).abs() < TOUCH {
        let dy = (candidate_native.min_y() - anchor_native.min_y()) * ratio_y;
        return Some(LogicalRect::new(
            LogicalPoint::new(
                anchor_shared.min_x() - candidate_size.width,
                anchor_shared.min_y() + dy,
            ),
            candidate_size,
        ));
    }
    // Below.
    if overlaps_horizontally && (candidate_native.min_y() - anchor_native.max_y()).abs() < TOUCH {
        let dx = (candidate_native.min_x() - anchor_native.min_x()) * ratio_x;
        return Some(LogicalRect::new(
            LogicalPoint::new(anchor_shared.min_x() + dx, anchor_shared.max_y()),
            candidate_size,
        ));
    }
    // Above.
    if overlaps_horizontally && (anchor_native.min_y() - candidate_native.max_y()).abs() < TOUCH {
        let dx = (candidate_native.min_x() - anchor_native.min_x()) * ratio_x;
        return Some(LogicalRect::new(
            LogicalPoint::new(
                anchor_shared.min_x() + dx,
                anchor_shared.min_y() - candidate_size.height,
            ),
            candidate_size,
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_types::{DeviceId, ScreenId};

    fn id(device: u8, screen: u32) -> GlobalScreenId {
        GlobalScreenId::new(DeviceId::from_bytes([device; 32]), ScreenId(screen))
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
        LogicalRect::from_parts(x, y, w, h).expect("valid")
    }

    fn scale(factor: f64) -> Scale {
        Scale::new(factor).expect("positive")
    }

    #[test]
    fn macos_bounds_are_already_the_shared_unit() {
        // CGDisplayBounds already accounts for the user's scaling choice; the
        // backing factor describes the pixels behind the points, not the size.
        let mapping = ScreenMapping::from_native(
            id(1, 0),
            rect(0.0, 0.0, 2560.0, 1440.0),
            scale(2.0),
            OsKind::MacOs,
        );
        assert_eq!(mapping.logical_size.width, 2560.0);
        assert_eq!(mapping.logical_size.height, 1440.0);
    }

    #[test]
    fn windows_bounds_are_divided_by_the_effective_scale() {
        // A 3840-wide monitor at 150% shows the user the equivalent of 2560.
        let mapping = ScreenMapping::from_native(
            id(2, 0),
            rect(0.0, 0.0, 3840.0, 2160.0),
            scale(1.5),
            OsKind::Windows,
        );
        assert_eq!(mapping.logical_size.width, 2560.0);
        assert_eq!(mapping.logical_size.height, 1440.0);
    }

    #[test]
    fn a_retina_mac_and_a_scaled_pc_end_up_comparable() {
        // The point of the whole conversion: two screens the user perceives as
        // the same size must be the same size in the shared space, even though
        // the numbers their operating systems report differ by a factor of 1.5.
        let mac = ScreenMapping::from_native(
            id(1, 0),
            rect(0.0, 0.0, 2560.0, 1440.0),
            scale(2.0),
            OsKind::MacOs,
        );
        let pc = ScreenMapping::from_native(
            id(2, 0),
            rect(0.0, 0.0, 3840.0, 2160.0),
            scale(1.5),
            OsKind::Windows,
        );
        assert_eq!(mac.logical_size, pc.logical_size);
    }

    #[test]
    fn the_native_space_is_preserved_exactly() {
        // Injection depends on it. A rounding error here means the cursor cannot
        // reach the last pixel column, which is where crossings happen.
        let native = rect(-1920.0, 0.0, 3840.0, 2160.0);
        let mapping = ScreenMapping::from_native(id(2, 0), native, scale(1.5), OsKind::Windows);
        assert_eq!(mapping.native, native);
    }

    #[test]
    fn conversion_round_trips_through_the_shared_space() {
        let mapping = ScreenMapping::from_native(
            id(2, 0),
            rect(0.0, 0.0, 3840.0, 2160.0),
            scale(1.5),
            OsKind::Windows,
        );
        let placed = rect(1000.0, 500.0, 2560.0, 1440.0);

        for native_point in [
            LogicalPoint::new(0.0, 0.0),
            LogicalPoint::new(1920.0, 1080.0),
            LogicalPoint::new(3839.0, 2159.0),
        ] {
            let shared = mapping.to_shared(&placed, native_point);
            let back = mapping.to_native(&placed, shared);
            assert!(
                (back.x - native_point.x).abs() < 2.0 && (back.y - native_point.y).abs() < 2.0,
                "{native_point:?} -> {shared:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn conversion_stays_inside_both_rectangles() {
        let mapping = ScreenMapping::from_native(
            id(1, 0),
            rect(0.0, 0.0, 1920.0, 1080.0),
            scale(1.0),
            OsKind::MacOs,
        );
        let placed = rect(500.0, 500.0, 1920.0, 1080.0);

        // A point on the shared rectangle's exclusive far edge must land on the
        // last addressable native coordinate, not one past it.
        let far = LogicalPoint::new(placed.max_x(), placed.max_y());
        let native = mapping.to_native(&placed, far);
        assert!(mapping.native.contains(native), "{native:?} escaped");
    }

    #[test]
    fn a_single_screen_places_at_the_origin() {
        let device = DeviceScreens::new(vec![ScreenMapping::from_native(
            id(1, 0),
            rect(0.0, 0.0, 1920.0, 1080.0),
            scale(1.0),
            OsKind::MacOs,
        )]);
        let placed = device.place_at(LogicalPoint::new(100.0, 200.0));
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].bounds.min_x(), 100.0);
        assert_eq!(placed[0].bounds.min_y(), 200.0);
    }

    #[test]
    fn a_multi_monitor_device_keeps_its_own_arrangement() {
        // The user set this up in their OS. The shared layout may move the group,
        // but rearranging within it would show them something they did not choose.
        let device = DeviceScreens::new(vec![
            ScreenMapping::from_native(
                id(1, 0),
                rect(0.0, 0.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::MacOs,
            ),
            // A second monitor placed above the first.
            ScreenMapping::from_native(
                id(1, 1),
                rect(0.0, -1080.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::MacOs,
            ),
        ]);

        let placed = device.place_at(LogicalPoint::new(0.0, 0.0));
        assert_eq!(placed.len(), 2);

        let upper = placed.iter().find(|s| s.id == id(1, 1)).expect("present");
        let lower = placed.iter().find(|s| s.id == id(1, 0)).expect("present");
        assert!(
            upper.bounds.min_y() < lower.bounds.min_y(),
            "arrangement flipped"
        );
        assert_eq!(upper.bounds.max_y(), lower.bounds.min_y(), "gap opened up");
    }

    #[test]
    fn mixed_dpi_screens_on_one_device_stay_adjacent() {
        // The failure ADR 0001 describes: scaling only the sizes leaves screens
        // that physically touch either overlapping or gapped in the shared space.
        // A 150% laptop panel beside a 100% external monitor.
        let device = DeviceScreens::new(vec![
            ScreenMapping::from_native(
                id(2, 0),
                rect(0.0, 0.0, 2880.0, 1800.0),
                scale(1.5),
                OsKind::Windows,
            ),
            ScreenMapping::from_native(
                id(2, 1),
                rect(2880.0, 0.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::Windows,
            ),
        ]);

        let placed = device.place_at(LogicalPoint::ZERO);
        let left = placed.iter().find(|s| s.id == id(2, 0)).expect("present");
        let right = placed.iter().find(|s| s.id == id(2, 1)).expect("present");

        assert!(
            (left.bounds.max_x() - right.bounds.min_x()).abs() < 1e-6,
            "screens that touch natively ended up {} apart in shared space",
            right.bounds.min_x() - left.bounds.max_x()
        );
        assert!(!left.bounds.intersects(&right.bounds), "screens overlap");

        // And the sizes reflect what the user actually perceives.
        assert_eq!(left.bounds.size.width, 1920.0);
        assert_eq!(right.bounds.size.width, 1920.0);
    }

    #[test]
    fn a_negative_native_origin_is_normalised_to_the_block_corner() {
        // Windows puts a monitor left of the primary at a negative coordinate.
        let device = DeviceScreens::new(vec![
            ScreenMapping::from_native(
                id(2, 0),
                rect(-1920.0, 0.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::Windows,
            ),
            ScreenMapping::from_native(
                id(2, 1),
                rect(0.0, 0.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::Windows,
            ),
        ]);

        let placed = device.place_at(LogicalPoint::new(5000.0, 0.0));
        let min_x = placed
            .iter()
            .map(|s| s.bounds.min_x())
            .fold(f64::MAX, f64::min);
        assert_eq!(min_x, 5000.0, "the block did not start where it was placed");
    }

    #[test]
    fn a_device_with_no_screens_places_nothing() {
        // Reachable while every display is asleep.
        let device = DeviceScreens::new(vec![]);
        assert!(device.is_empty());
        assert!(device.place_at(LogicalPoint::ZERO).is_empty());
        assert!(device.shared_extent().is_none());
    }

    #[test]
    fn the_shared_extent_covers_the_whole_arrangement() {
        let device = DeviceScreens::new(vec![
            ScreenMapping::from_native(
                id(1, 0),
                rect(0.0, 0.0, 1920.0, 1080.0),
                scale(1.0),
                OsKind::MacOs,
            ),
            ScreenMapping::from_native(
                id(1, 1),
                rect(1920.0, 0.0, 1280.0, 800.0),
                scale(1.0),
                OsKind::MacOs,
            ),
        ]);
        let extent = device.shared_extent().expect("has screens");
        assert_eq!(extent.width, 3200.0);
        assert_eq!(extent.height, 1080.0);
    }
}
