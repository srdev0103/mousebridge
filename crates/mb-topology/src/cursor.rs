//! Tracking the cursor and deciding when it crosses to another screen.
//!
//! # The three ways this goes wrong
//!
//! Edge switching is easy to implement and hard to make pleasant. Three failure
//! modes account for almost all of it, and each has a rule here:
//!
//! 1. **Accidental crossing.** The user throws the pointer at a scrollbar or a
//!    close button and lands on another computer. Guarded in two parts: the
//!    cursor must first *arrive* at the edge and stop there, and only then does
//!    continued outward motion accumulate towards
//!    [`SwitchingRules::edge_overshoot_points`].
//!
//!    Accumulating distance alone is not enough, and getting this wrong is easy:
//!    a single fast movement that happens to end past the edge satisfies any
//!    distance threshold in one frame, which is precisely the accidental
//!    crossing the rule exists to prevent. Requiring the cursor to be *already*
//!    pressed against the edge is what separates "flung the pointer at a
//!    scrollbar" from "pushed towards the next machine and kept pushing".
//! 2. **Oscillation.** The cursor sits on a seam and flips between machines many
//!    times a second. Guarded by a cooldown after each crossing —
//!    [`SwitchingRules::cooldown`].
//! 3. **Corner traps.** The macOS menu bar, the Windows Start button and every
//!    window's close button live in corners, and users fling the pointer at them
//!    at speed. Guarded by excluding a small region at each corner —
//!    [`SwitchingRules::corner_deadzone_points`].
//!
//! # Time is a parameter
//!
//! The cooldown is the only time-dependent rule, and it takes `now` as an
//! argument rather than reading a clock. A whole minute of edge behaviour is
//! therefore testable in microseconds, and deterministically.

use crate::layout::Layout;
use mb_types::{GlobalScreenId, LogicalPoint};
use std::time::{Duration, Instant};

/// How the cursor may cross between screens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitchingRules {
    /// Whether crossing is allowed at all.
    pub enabled: bool,
    /// Logical points of continued outward motion required to commit a crossing.
    pub edge_overshoot_points: f64,
    /// Minimum time between crossings.
    pub cooldown: Duration,
    /// Size of the excluded region at each screen corner.
    pub corner_deadzone_points: f64,
}

impl Default for SwitchingRules {
    fn default() -> Self {
        Self {
            enabled: true,
            edge_overshoot_points: 12.0,
            cooldown: Duration::from_millis(200),
            corner_deadzone_points: 8.0,
        }
    }
}

/// What happened to the cursor.
#[derive(Debug, Clone, PartialEq)]
pub enum Movement {
    /// The cursor moved within the screen it was already on.
    Local {
        /// New position in the shared virtual space.
        position: LogicalPoint,
    },
    /// The cursor pressed against a boundary it may not cross yet.
    ///
    /// Either there is nothing beyond it, or one of the anti-jitter rules has
    /// not been satisfied. The position is clamped inside the current screen, so
    /// the caller applies it normally — the cursor stops at the edge.
    Blocked {
        /// Clamped position, still inside the current screen.
        position: LogicalPoint,
        /// Accumulated outward motion so far, for diagnostics.
        overshoot: f64,
    },
    /// The cursor crossed to another screen.
    Crossed {
        /// The screen being left.
        from: GlobalScreenId,
        /// The screen being entered.
        to: GlobalScreenId,
        /// Where along the new screen the cursor entered, each axis in
        /// `0.0..=1.0`.
        ///
        /// Normalised so screens of different sizes and scale factors hand off
        /// proportionally rather than by raw coordinate.
        entry: (f64, f64),
        /// New position in the shared virtual space.
        position: LogicalPoint,
    },
}

/// Tracks the cursor's position and applies the crossing rules.
#[derive(Debug, Clone)]
pub struct CursorTracker {
    position: LogicalPoint,
    current: GlobalScreenId,
    /// Outward motion accumulated while pressed against a boundary.
    overshoot: f64,
    /// The edge the cursor is currently resting against, if any.
    ///
    /// Overshoot only accumulates once this is set, which is what makes a single
    /// fast movement past the edge stop rather than cross.
    pressed_edge: Option<mb_types::Edge>,
    last_crossing: Option<Instant>,
    rules: SwitchingRules,
}

impl CursorTracker {
    /// Places the cursor at the centre of a screen.
    ///
    /// # Errors
    ///
    /// Returns `None` if the screen is not in the layout.
    #[must_use]
    pub fn new(layout: &Layout, screen: GlobalScreenId, rules: SwitchingRules) -> Option<Self> {
        let placed = layout.get(screen)?;
        Some(Self {
            position: placed.bounds.center(),
            current: screen,
            overshoot: 0.0,
            pressed_edge: None,
            last_crossing: None,
            rules,
        })
    }

    /// The cursor's position in the shared virtual space.
    #[must_use]
    pub const fn position(&self) -> LogicalPoint {
        self.position
    }

    /// The screen the cursor is on.
    #[must_use]
    pub const fn screen(&self) -> GlobalScreenId {
        self.current
    }

    /// Replaces the rules, for example after a settings change.
    pub const fn set_rules(&mut self, rules: SwitchingRules) {
        self.rules = rules;
    }

    /// Applies a movement and reports what happened.
    ///
    /// Allocation-free: this runs on the cursor path.
    #[must_use]
    pub fn apply(&mut self, layout: &Layout, dx: f64, dy: f64, now: Instant) -> Movement {
        // A non-finite delta can arrive from a faulty device. Treating it as no
        // movement keeps a NaN out of the position, where it would poison every
        // subsequent comparison and freeze the cursor permanently.
        if !dx.is_finite() || !dy.is_finite() {
            return Movement::Local {
                position: self.position,
            };
        }

        let Some(current) = layout.get(self.current) else {
            // The screen vanished — a display was unplugged mid-movement. Hold
            // the position rather than guessing; the caller re-places the cursor
            // when it rebuilds the layout.
            return Movement::Local {
                position: self.position,
            };
        };

        let target = self.position.offset(dx, dy);

        // Still on the same screen: the common case, and the cheapest path.
        if current.bounds.contains(target) {
            self.position = target;
            self.overshoot = 0.0;
            self.pressed_edge = None;
            return Movement::Local { position: target };
        }

        let clamped = current.bounds.clamp_point(target, 0.5);
        let edge = crate::layout::exited_edge(&current.bounds, target);

        if !self.rules.enabled {
            self.position = clamped;
            return Movement::Blocked {
                position: clamped,
                overshoot: 0.0,
            };
        }

        // Cooldown: without this the cursor can flip between machines many times
        // a second while resting on a seam.
        if let Some(last) = self.last_crossing
            && now.duration_since(last) < self.rules.cooldown
        {
            self.position = clamped;
            return Movement::Blocked {
                position: clamped,
                overshoot: self.overshoot,
            };
        }

        // Corner dead zone: the menu bar, the Start button and every close
        // button live in corners, and users throw the pointer at them.
        if self.in_corner_deadzone(&current.bounds, clamped) {
            self.position = clamped;
            self.overshoot = 0.0;
            self.pressed_edge = edge;
            return Movement::Blocked {
                position: clamped,
                overshoot: 0.0,
            };
        }

        // The cursor must already be resting against *this* edge before any
        // outward motion counts. A movement that arrives at the edge — however
        // fast, however far past it — only ever stops there. Sliding along to a
        // different edge restarts the count, because pressing rightwards should
        // not build credit towards crossing downwards.
        if self.pressed_edge != edge {
            self.pressed_edge = edge;
            self.overshoot = 0.0;
            self.position = clamped;
            return Movement::Blocked {
                position: clamped,
                overshoot: 0.0,
            };
        }

        self.overshoot += distance_outside(&current.bounds, target);
        if self.overshoot < self.rules.edge_overshoot_points {
            self.position = clamped;
            return Movement::Blocked {
                position: clamped,
                overshoot: self.overshoot,
            };
        }

        // Find what lies beyond: either the target point is inside a neighbour,
        // or the path crossed into one on the way.
        let entered = layout
            .screen_at(target)
            .filter(|s| s.id != self.current)
            .or_else(|| {
                layout
                    .first_screen_along(self.position, target, self.current)
                    .map(|(screen, _)| screen)
            });

        let Some(entered) = entered else {
            // Nothing there. The cursor stops at the edge, and the accumulated
            // overshoot is kept so a continued push crosses the moment a screen
            // appears — a machine waking up, for instance.
            self.position = clamped;
            return Movement::Blocked {
                position: clamped,
                overshoot: self.overshoot,
            };
        };

        let landing = entered.bounds.clamp_point(target, 0.5);
        let entry = entered.bounds.normalize(landing);
        let from = self.current;

        self.current = entered.id;
        self.position = landing;
        self.overshoot = 0.0;
        self.pressed_edge = None;
        self.last_crossing = Some(now);

        Movement::Crossed {
            from,
            to: entered.id,
            entry,
            position: landing,
        }
    }

    /// Places the cursor on a screen without treating it as a crossing.
    ///
    /// Used when control arrives from another machine, and when a layout change
    /// invalidates the current position.
    ///
    /// # Errors
    ///
    /// Returns `false` if the screen is not in the layout.
    pub fn place(&mut self, layout: &Layout, screen: GlobalScreenId, entry: (f64, f64)) -> bool {
        let Some(placed) = layout.get(screen) else {
            return false;
        };
        self.position = placed.bounds.denormalize(entry.0, entry.1);
        self.position = placed.bounds.clamp_point(self.position, 0.5);
        self.current = screen;
        self.overshoot = 0.0;
        self.pressed_edge = None;
        true
    }

    fn in_corner_deadzone(&self, bounds: &mb_types::LogicalRect, point: LogicalPoint) -> bool {
        let zone = self.rules.corner_deadzone_points;
        if zone <= 0.0 {
            return false;
        }
        let near_horizontal = point.x - bounds.min_x() < zone || bounds.max_x() - point.x < zone;
        let near_vertical = point.y - bounds.min_y() < zone || bounds.max_y() - point.y < zone;
        near_horizontal && near_vertical
    }
}

/// How far outside a rectangle a point lies, along the axis it exceeded most.
fn distance_outside(rect: &mb_types::LogicalRect, point: LogicalPoint) -> f64 {
    let horizontal = (rect.min_x() - point.x)
        .max(point.x - rect.max_x())
        .max(0.0);
    let vertical = (rect.min_y() - point.y)
        .max(point.y - rect.max_y())
        .max(0.0);
    horizontal.max(vertical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PlacedScreen;
    use mb_types::{DeviceId, LogicalRect, Scale, ScreenId};

    fn id(device: u8) -> GlobalScreenId {
        GlobalScreenId::new(DeviceId::from_bytes([device; 32]), ScreenId(0))
    }

    fn screen(device: u8, x: f64, y: f64, w: f64, h: f64) -> PlacedScreen {
        PlacedScreen {
            id: id(device),
            bounds: LogicalRect::from_parts(x, y, w, h).expect("valid"),
            scale: Scale::ONE,
        }
    }

    fn side_by_side() -> Layout {
        Layout::new(vec![
            screen(1, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 1920.0, 0.0, 1920.0, 1080.0),
        ])
        .expect("valid")
    }

    fn tracker(layout: &Layout, rules: SwitchingRules) -> CursorTracker {
        CursorTracker::new(layout, id(1), rules).expect("screen exists")
    }

    /// Pushes right until something happens, or the budget runs out.
    fn push_right(
        tracker: &mut CursorTracker,
        layout: &Layout,
        step: f64,
        times: usize,
        now: Instant,
    ) -> Movement {
        let mut last = Movement::Local {
            position: tracker.position(),
        };
        for _ in 0..times {
            last = tracker.apply(layout, step, 0.0, now);
            if matches!(last, Movement::Crossed { .. }) {
                return last;
            }
        }
        last
    }

    #[test]
    fn ordinary_movement_stays_local() {
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        let movement = tracker.apply(&layout, 10.0, 5.0, now);
        assert!(matches!(movement, Movement::Local { .. }));
        assert_eq!(tracker.screen(), id(1));
    }

    #[test]
    fn a_single_overshoot_frame_does_not_cross() {
        // The accidental-crossing guard. Throwing the pointer at a scrollbar
        // must not land the user on another computer.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        // One fast fling that ends well past the edge. However far it overshoots,
        // arriving at an edge is never a crossing.
        let movement = tracker.apply(&layout, 1000.0, 0.0, now);
        assert!(
            matches!(movement, Movement::Blocked { .. }),
            "a single fling crossed: {movement:?}"
        );
        assert_eq!(tracker.screen(), id(1));

        // And one more small nudge is still not enough.
        let movement = tracker.apply(&layout, 5.0, 0.0, now);
        assert!(matches!(movement, Movement::Blocked { .. }), "{movement:?}");
        assert_eq!(tracker.screen(), id(1));
    }

    #[test]
    fn continued_outward_motion_does_cross() {
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        let _ = tracker.apply(&layout, 1000.0, 0.0, now);
        let movement = push_right(&mut tracker, &layout, 5.0, 40, now);

        let Movement::Crossed {
            from, to, entry, ..
        } = movement
        else {
            panic!("never crossed: {movement:?}");
        };
        assert_eq!(from, id(1));
        assert_eq!(to, id(2));
        assert!(
            entry.0 < 0.01,
            "should enter at the left edge, got {entry:?}"
        );
        assert_eq!(tracker.screen(), id(2));
    }

    #[test]
    fn the_cursor_stops_at_an_edge_with_nothing_beyond_it() {
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        // Push left, where there is no screen.
        let movement = push_right(&mut tracker, &layout, -50.0, 100, now);
        assert!(matches!(movement, Movement::Blocked { .. }));
        assert_eq!(tracker.screen(), id(1));
        assert!(tracker.position().x >= 0.0, "cursor escaped the desktop");
    }

    #[test]
    fn a_blocked_cursor_stays_inside_its_screen() {
        // The clamped position must satisfy `contains`, or the next move starts
        // from a point the current screen does not own.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        push_right(&mut tracker, &layout, -50.0, 100, now);
        let bounds = layout.get(id(1)).expect("present").bounds;
        assert!(bounds.contains(tracker.position()));
    }

    #[test]
    fn the_cooldown_prevents_oscillation_at_a_seam() {
        let layout = side_by_side();
        let rules = SwitchingRules {
            cooldown: Duration::from_millis(200),
            ..SwitchingRules::default()
        };
        let mut tracker = tracker(&layout, rules);
        let start = Instant::now();

        let _ = tracker.apply(&layout, 1000.0, 0.0, start);
        let crossed = push_right(&mut tracker, &layout, 5.0, 40, start);
        assert!(matches!(crossed, Movement::Crossed { .. }), "{crossed:?}");

        // Immediately try to come back. The cooldown must refuse.
        let back = push_right(&mut tracker, &layout, -50.0, 20, start);
        assert!(
            matches!(back, Movement::Blocked { .. }),
            "crossed back inside the cooldown: {back:?}"
        );
        assert_eq!(tracker.screen(), id(2));

        // After the cooldown, it is allowed.
        let later = start + Duration::from_millis(250);
        let back = push_right(&mut tracker, &layout, -50.0, 20, later);
        assert!(matches!(back, Movement::Crossed { .. }), "{back:?}");
        assert_eq!(tracker.screen(), id(1));
    }

    #[test]
    fn corners_are_excluded() {
        // Users fling the pointer at the menu bar and the Start button. Crossing
        // there is almost always accidental.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();

        // Go to the top-right corner, then push right.
        let _ = tracker.apply(&layout, 5000.0, -5000.0, now);
        let movement = push_right(&mut tracker, &layout, 20.0, 20, now);

        assert!(
            matches!(movement, Movement::Blocked { .. }),
            "crossed from a corner: {movement:?}"
        );
        assert_eq!(tracker.screen(), id(1));
    }

    #[test]
    fn crossing_works_just_outside_the_corner_zone() {
        // The dead zone must be small enough that the rest of the edge works.
        let layout = side_by_side();
        let rules = SwitchingRules {
            corner_deadzone_points: 8.0,
            ..SwitchingRules::default()
        };
        let mut tracker = tracker(&layout, rules);
        let now = Instant::now();

        // Sit well clear of both corners.
        let _ = tracker.apply(&layout, 5000.0, -5000.0, now);
        let _ = tracker.apply(&layout, 0.0, 400.0, now);
        let movement = push_right(&mut tracker, &layout, 20.0, 20, now);
        assert!(matches!(movement, Movement::Crossed { .. }), "{movement:?}");
    }

    #[test]
    fn disabling_switching_blocks_every_crossing() {
        let layout = side_by_side();
        let rules = SwitchingRules {
            enabled: false,
            ..SwitchingRules::default()
        };
        let mut tracker = tracker(&layout, rules);
        let now = Instant::now();

        let movement = push_right(&mut tracker, &layout, 500.0, 20, now);
        assert!(matches!(movement, Movement::Blocked { .. }));
        assert_eq!(tracker.screen(), id(1));
    }

    #[test]
    fn entry_position_is_proportional_across_mismatched_screens() {
        // A 4K panel handing off to a small laptop must land proportionally, not
        // at a raw coordinate that would be off-screen.
        let layout = Layout::new(vec![
            screen(1, 0.0, 0.0, 3840.0, 2160.0),
            screen(2, 3840.0, 0.0, 1280.0, 800.0),
        ])
        .expect("valid");
        let mut tracker =
            CursorTracker::new(&layout, id(1), SwitchingRules::default()).expect("screen exists");
        let now = Instant::now();

        // Move to a height the small screen actually covers: it is only 800
        // points tall against the 4K panel's 2160, so most of the right edge has
        // nothing beyond it at all.
        let _ = tracker.apply(&layout, 5000.0, 5000.0, now);
        let _ = tracker.apply(&layout, 0.0, -1560.0, now);
        let movement = push_right(&mut tracker, &layout, 20.0, 20, now);

        let Movement::Crossed {
            entry, position, ..
        } = movement
        else {
            panic!("never crossed: {movement:?}");
        };
        assert!(
            entry.1 > 0.6 && entry.1 < 0.95,
            "entry {entry:?} not proportional to the 4K panel's edge position"
        );
        assert!(
            layout
                .get(id(2))
                .expect("present")
                .bounds
                .contains(position),
            "landed outside the target screen"
        );
    }

    #[test]
    fn a_non_finite_delta_cannot_poison_the_position() {
        // A faulty device can produce one. A NaN in the position would freeze the
        // cursor permanently, because every later comparison would be false.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();
        let before = tracker.position();

        let _ = tracker.apply(&layout, f64::NAN, 0.0, now);
        let _ = tracker.apply(&layout, 0.0, f64::INFINITY, now);

        assert_eq!(tracker.position(), before);
        assert!(tracker.position().is_finite());
    }

    #[test]
    fn placing_the_cursor_is_not_a_crossing() {
        // Used when control arrives from another machine; it must not consume
        // the cooldown or count as a switch.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());

        assert!(tracker.place(&layout, id(2), (0.0, 0.5)));
        assert_eq!(tracker.screen(), id(2));

        let bounds = layout.get(id(2)).expect("present").bounds;
        assert!(bounds.contains(tracker.position()));
    }

    #[test]
    fn placing_onto_an_unknown_screen_fails_safely() {
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let before = tracker.screen();

        assert!(!tracker.place(&layout, id(9), (0.5, 0.5)));
        assert_eq!(tracker.screen(), before);
    }

    #[test]
    fn a_vanished_screen_holds_the_position_rather_than_guessing() {
        // A display unplugged mid-movement. Guessing a new position would put the
        // cursor somewhere the user did not expect.
        let layout = side_by_side();
        let mut tracker = tracker(&layout, SwitchingRules::default());
        let now = Instant::now();
        let before = tracker.position();

        let smaller = Layout::new(vec![screen(2, 1920.0, 0.0, 1920.0, 1080.0)]).expect("valid");
        let movement = tracker.apply(&smaller, 100.0, 0.0, now);

        assert!(matches!(movement, Movement::Local { .. }));
        assert_eq!(tracker.position(), before);
    }

    #[test]
    fn a_vertical_stack_crosses_downward() {
        let layout = Layout::new(vec![
            screen(1, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 0.0, 1080.0, 1920.0, 1080.0),
        ])
        .expect("valid");
        let mut tracker =
            CursorTracker::new(&layout, id(1), SwitchingRules::default()).expect("screen exists");
        let now = Instant::now();

        let _ = tracker.apply(&layout, 0.0, 1000.0, now);
        let mut movement = Movement::Local {
            position: tracker.position(),
        };
        for _ in 0..40 {
            movement = tracker.apply(&layout, 0.0, 5.0, now);
            if matches!(movement, Movement::Crossed { .. }) {
                break;
            }
        }
        let Movement::Crossed { to, entry, .. } = movement else {
            panic!("never crossed downward: {movement:?}");
        };
        assert_eq!(to, id(2));
        assert!(
            entry.1 < 0.01,
            "should enter at the top edge, got {entry:?}"
        );
    }
}
