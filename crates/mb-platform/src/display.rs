//! Display enumeration.

use mb_types::{LogicalRect, Scale, ScreenId};

/// One attached display, described in its own device's native cursor space.
///
/// `bounds` is expressed in the coordinate space in which a cursor position is
/// meaningful *on that device*: logical points on macOS, virtual-screen physical
/// pixels on Windows. `scale` relates that space to physical pixels. The two are
/// deliberately not pre-combined here — see
/// `docs/adr/0001-display-coordinate-space.md`.
///
/// Placement into the shared virtual desktop space that spans every device is the
/// topology engine's job, not the platform layer's.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// Identifier assigned by the platform layer.
    ///
    /// Stable only while the display stays attached. Not persisted: display IDs
    /// are recycled across reboots and dock connections, so keying a saved
    /// layout on one would silently reassign the user's screens.
    pub id: ScreenId,
    /// Position and size in the device's native cursor coordinate space.
    pub bounds: LogicalRect,
    /// Backing scale factor: 2.0 for a Retina panel, 1.5 for Windows at 150%.
    pub scale: Scale,
    /// Whether this is the primary display, which owns the local origin.
    pub is_primary: bool,
    /// Human-readable name where the OS provides one.
    pub name: Option<String>,
}

/// The set of displays currently attached to this device.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DisplayLayout {
    displays: Vec<DisplayInfo>,
}

impl DisplayLayout {
    /// Builds a layout from an enumeration result.
    ///
    /// Displays are sorted by position — top to bottom, then left to right — so
    /// that the order is stable across enumerations. The OS does not guarantee a
    /// stable order, and an unstable one would make the topology editor reshuffle
    /// the user's screens whenever a display woke up.
    #[must_use]
    pub fn new(mut displays: Vec<DisplayInfo>) -> Self {
        displays.sort_by(|a, b| {
            a.bounds
                .min_y()
                .total_cmp(&b.bounds.min_y())
                .then_with(|| a.bounds.min_x().total_cmp(&b.bounds.min_x()))
                .then_with(|| a.id.cmp(&b.id))
        });
        Self { displays }
    }

    /// Returns the displays.
    #[must_use]
    pub fn displays(&self) -> &[DisplayInfo] {
        &self.displays
    }

    /// Returns the number of attached displays.
    #[must_use]
    pub fn len(&self) -> usize {
        self.displays.len()
    }

    /// Returns true when no displays are attached.
    ///
    /// Reachable in practice: a closed clamshell, a display asleep, or a remote
    /// session with no console. Callers must treat it as a transient state and
    /// suspend edge detection rather than dividing by a zero-sized desktop.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.displays.is_empty()
    }

    /// Returns the primary display, if one is marked.
    #[must_use]
    pub fn primary(&self) -> Option<&DisplayInfo> {
        self.displays
            .iter()
            .find(|d| d.is_primary)
            .or_else(|| self.displays.first())
    }

    /// Finds the display by identifier.
    #[must_use]
    pub fn get(&self, id: ScreenId) -> Option<&DisplayInfo> {
        self.displays.iter().find(|d| d.id == id)
    }

    /// Returns the smallest rectangle covering every display.
    #[must_use]
    pub fn bounding_box(&self) -> Option<LogicalRect> {
        let mut iter = self.displays.iter();
        let first = iter.next()?.bounds;
        Some(iter.fold(first, |acc, d| acc.union(&d.bounds)))
    }

    /// Finds the display containing a point in the device's native cursor space.
    #[must_use]
    pub fn display_at(&self, point: mb_types::LogicalPoint) -> Option<&DisplayInfo> {
        self.displays.iter().find(|d| d.bounds.contains(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_types::LogicalPoint;

    fn display(id: u32, x: f64, y: f64, w: f64, h: f64, primary: bool) -> DisplayInfo {
        DisplayInfo {
            id: ScreenId(id),
            bounds: LogicalRect::from_parts(x, y, w, h).unwrap(),
            scale: Scale::ONE,
            is_primary: primary,
            name: None,
        }
    }

    #[test]
    fn ordering_is_stable_regardless_of_enumeration_order() {
        let a = display(3, 1920.0, 0.0, 1920.0, 1080.0, false);
        let b = display(1, 0.0, 0.0, 1920.0, 1080.0, true);
        let c = display(2, 0.0, 1080.0, 1920.0, 1080.0, false);

        let one = DisplayLayout::new(vec![a.clone(), b.clone(), c.clone()]);
        let two = DisplayLayout::new(vec![c, b, a]);
        assert_eq!(one, two, "enumeration order must not affect the layout");
        assert_eq!(
            one.displays().iter().map(|d| d.id.0).collect::<Vec<_>>(),
            vec![1, 3, 2]
        );
    }

    #[test]
    fn primary_falls_back_to_the_first_display() {
        // Not every platform marks a primary; the topology still needs an origin.
        let layout = DisplayLayout::new(vec![display(7, 0.0, 0.0, 800.0, 600.0, false)]);
        assert_eq!(layout.primary().map(|d| d.id), Some(ScreenId(7)));
    }

    #[test]
    fn bounding_box_spans_every_display() {
        let layout = DisplayLayout::new(vec![
            display(1, 0.0, 0.0, 1920.0, 1080.0, true),
            display(2, 1920.0, -200.0, 2560.0, 1440.0, false),
        ]);
        let bb = layout.bounding_box().unwrap();
        assert_eq!(bb.min_x(), 0.0);
        assert_eq!(bb.min_y(), -200.0);
        assert_eq!(bb.max_x(), 4480.0);
        assert_eq!(bb.max_y(), 1240.0);
    }

    #[test]
    fn empty_layout_is_handled_not_assumed_away() {
        let layout = DisplayLayout::default();
        assert!(layout.is_empty());
        assert!(layout.primary().is_none());
        assert!(layout.bounding_box().is_none());
        assert!(layout.display_at(LogicalPoint::ZERO).is_none());
    }

    #[test]
    fn display_at_respects_half_open_bounds() {
        let layout = DisplayLayout::new(vec![
            display(1, 0.0, 0.0, 1920.0, 1080.0, true),
            display(2, 1920.0, 0.0, 1920.0, 1080.0, false),
        ]);
        // The seam belongs to exactly one display, never both.
        assert_eq!(
            layout
                .display_at(LogicalPoint::new(1920.0, 500.0))
                .map(|d| d.id),
            Some(ScreenId(2))
        );
        assert_eq!(
            layout
                .display_at(LogicalPoint::new(1919.9, 500.0))
                .map(|d| d.id),
            Some(ScreenId(1))
        );
    }
}
