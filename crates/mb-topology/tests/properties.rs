//! Property tests for the cursor state machine.
//!
//! The example tests cover the arrangements we thought of. These cover the
//! movement sequences we did not: violent flings, alternating directions,
//! non-finite deltas, and long random walks along seams.
//!
//! The central invariant is that **the cursor is always somewhere real**. A
//! position outside every screen, or a NaN coordinate, would freeze the pointer
//! permanently — every later comparison against it would be false.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_topology::cursor::{CursorTracker, Movement, SwitchingRules};
use mb_topology::layout::{Layout, PlacedScreen};
use mb_types::{DeviceId, GlobalScreenId, LogicalRect, Scale, ScreenId};
use proptest::prelude::*;
use std::time::{Duration, Instant};

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

/// Three screens in an L: two side by side, one below the first.
///
/// Chosen because it has edges that lead somewhere, edges that lead nowhere, and
/// an inside corner — the arrangement most likely to expose a hole.
fn awkward_layout() -> Layout {
    Layout::new(vec![
        screen(1, 0.0, 0.0, 1920.0, 1080.0),
        screen(2, 1920.0, 0.0, 1280.0, 800.0),
        screen(3, 0.0, 1080.0, 1920.0, 1080.0),
    ])
    .expect("valid layout")
}

/// Deltas spanning gentle drift, ordinary movement, and violent flings.
fn arb_delta() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-2.0f64..2.0),
        (-60.0f64..60.0),
        (-5000.0f64..5000.0),
        Just(0.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
    ]
}

proptest! {
    /// The cursor is always inside the screen it claims to be on.
    ///
    /// Violating this would mean the next movement starts from a point the
    /// current screen does not own, and the cursor could work its way into a
    /// position no screen contains — where it would stay forever.
    #[test]
    fn the_cursor_is_always_on_its_own_screen(
        moves in prop::collection::vec((arb_delta(), arb_delta()), 0..300)
    ) {
        let layout = awkward_layout();
        let mut tracker = CursorTracker::new(&layout, id(1), SwitchingRules::default())
            .expect("screen exists");
        let now = Instant::now();

        for (dx, dy) in moves {
            let _ = tracker.apply(&layout, dx, dy, now);

            let screen = layout.get(tracker.screen()).expect("screen still exists");
            prop_assert!(
                screen.bounds.contains(tracker.position()),
                "cursor at {:?} is outside its screen {}",
                tracker.position(),
                screen.id
            );
        }
    }

    /// The position never becomes non-finite, whatever arrives.
    #[test]
    fn the_position_is_always_finite(
        moves in prop::collection::vec((arb_delta(), arb_delta()), 0..300)
    ) {
        let layout = awkward_layout();
        let mut tracker = CursorTracker::new(&layout, id(1), SwitchingRules::default())
            .expect("screen exists");
        let now = Instant::now();

        for (dx, dy) in moves {
            let _ = tracker.apply(&layout, dx, dy, now);
            prop_assert!(tracker.position().is_finite(), "position became non-finite");
        }
    }

    /// A crossing always lands inside the screen it reports entering, with an
    /// entry position in the unit range.
    ///
    /// The receiving machine multiplies that entry position by its own screen
    /// size. A value outside `0.0..=1.0` would place the cursor off-screen.
    #[test]
    fn crossings_land_where_they_say_they_do(
        moves in prop::collection::vec((arb_delta(), arb_delta()), 0..300)
    ) {
        let layout = awkward_layout();
        let mut tracker = CursorTracker::new(&layout, id(1), SwitchingRules::default())
            .expect("screen exists");
        let mut now = Instant::now();

        for (dx, dy) in moves {
            // Advance past the cooldown so crossings are actually reachable.
            now += Duration::from_millis(250);

            if let Movement::Crossed { to, entry, position, from } =
                tracker.apply(&layout, dx, dy, now)
            {
                prop_assert_ne!(from, to, "crossed to the screen it was already on");

                let target = layout.get(to).expect("target exists");
                prop_assert!(
                    target.bounds.contains(position),
                    "landed at {:?} outside {}",
                    position,
                    to
                );
                prop_assert!((0.0..=1.0).contains(&entry.0), "entry.0 = {}", entry.0);
                prop_assert!((0.0..=1.0).contains(&entry.1), "entry.1 = {}", entry.1);
            }
        }
    }

    /// Arriving at an edge is never itself a crossing, however fast.
    ///
    /// The accidental-crossing guard, stated directly: from a resting position,
    /// no single movement crosses.
    #[test]
    fn no_single_movement_ever_crosses(dx in arb_delta(), dy in arb_delta()) {
        let layout = awkward_layout();
        let mut tracker = CursorTracker::new(&layout, id(1), SwitchingRules::default())
            .expect("screen exists");

        let movement = tracker.apply(&layout, dx, dy, Instant::now());
        prop_assert!(
            !matches!(movement, Movement::Crossed { .. }),
            "a single movement of ({dx}, {dy}) crossed"
        );
    }

    /// With switching disabled, the cursor never leaves its starting screen.
    #[test]
    fn disabled_switching_is_absolute(
        moves in prop::collection::vec((arb_delta(), arb_delta()), 0..300)
    ) {
        let layout = awkward_layout();
        let rules = SwitchingRules { enabled: false, ..SwitchingRules::default() };
        let mut tracker = CursorTracker::new(&layout, id(1), rules).expect("screen exists");
        let mut now = Instant::now();

        for (dx, dy) in moves {
            now += Duration::from_millis(250);
            let movement = tracker.apply(&layout, dx, dy, now);
            prop_assert!(
                !matches!(movement, Movement::Crossed { .. }),
                "crossed with switching disabled"
            );
            prop_assert_eq!(tracker.screen(), id(1));
        }
    }

    /// Crossings never occur closer together than the cooldown allows.
    ///
    /// This is what stops the cursor flipping between machines many times a
    /// second while resting on a seam.
    #[test]
    fn crossings_respect_the_cooldown(
        moves in prop::collection::vec((arb_delta(), arb_delta()), 0..400)
    ) {
        let layout = awkward_layout();
        let cooldown = Duration::from_millis(200);
        let rules = SwitchingRules { cooldown, ..SwitchingRules::default() };
        let mut tracker = CursorTracker::new(&layout, id(1), rules).expect("screen exists");

        let start = Instant::now();
        let mut now = start;
        let mut last_crossing: Option<Instant> = None;

        for (dx, dy) in moves {
            // Advance by less than the cooldown each step, so the rule is
            // genuinely under test rather than trivially satisfied.
            now += Duration::from_millis(30);

            if matches!(tracker.apply(&layout, dx, dy, now), Movement::Crossed { .. }) {
                if let Some(previous) = last_crossing {
                    prop_assert!(
                        now.duration_since(previous) >= cooldown,
                        "crossed {:?} after the previous crossing",
                        now.duration_since(previous)
                    );
                }
                last_crossing = Some(now);
            }
        }
    }

    /// Placing the cursor always leaves it somewhere valid.
    #[test]
    fn placement_always_lands_inside_the_target(
        nx in -10.0f64..10.0,
        ny in -10.0f64..10.0,
        which in 1u8..4,
    ) {
        let layout = awkward_layout();
        let mut tracker = CursorTracker::new(&layout, id(1), SwitchingRules::default())
            .expect("screen exists");

        prop_assert!(tracker.place(&layout, id(which), (nx, ny)));

        let screen = layout.get(id(which)).expect("exists");
        prop_assert!(
            screen.bounds.contains(tracker.position()),
            "placed at {:?}, outside {}",
            tracker.position(),
            screen.id
        );
    }
}
