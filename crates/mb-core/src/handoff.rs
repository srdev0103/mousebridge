//! Turning cursor movement into a decision about which machine gets the input.
//!
//! # Why this runs on the capture thread
//!
//! The suppress-or-pass-through decision *is* the topology decision: whether an
//! event belongs to this machine depends on which screen the cursor is on. There
//! is no way to defer it to another thread, because the answer has to be
//! returned to the operating system before the hook returns.
//!
//! So the geometry runs there. That is affordable — [`mb_topology`] allocates
//! nothing and does a handful of comparisons — but it does mean the state it
//! reads cannot sit behind a lock the capture thread might block on.
//!
//! # The lock that is never waited on
//!
//! Layout and cursor state live behind a mutex, because the user interface also
//! writes to them when screens are rearranged. The capture thread only ever
//! **tries** to take it. If a layout edit happens to hold it at that instant,
//! that single event skips its topology update and is routed by the last known
//! destination instead — which is a stale answer for one event, rather than a
//! stalled input thread and a tap the OS disables for being slow.
//!
//! The destination itself is an atomic for exactly that reason: the routing
//! decision must always be answerable without waiting for anything.

use crate::routing::Destination;
use mb_topology::{CursorTracker, Layout, Movement, SwitchingRules};
use mb_types::{DeviceId, GlobalScreenId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What a movement means for the rest of the system.
#[derive(Debug, Clone, PartialEq)]
pub enum HandoffAction {
    /// Nothing changed. Keep routing as before.
    None,
    /// Control moved to a screen on another device.
    ///
    /// The caller must tell that device to take over, releasing everything held
    /// here first so nothing is left stuck on the machine being left.
    Leave {
        /// The device now receiving input.
        to: DeviceId,
        /// The screen the cursor entered.
        screen: GlobalScreenId,
        /// Normalised entry position along that screen's edge.
        entry: (f64, f64),
    },
    /// Control came back to a screen on this device.
    Enter {
        /// The device that was receiving input.
        from: DeviceId,
        /// The screen the cursor entered.
        screen: GlobalScreenId,
        /// Normalised entry position.
        entry: (f64, f64),
    },
}

/// Counters describing handoff behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HandoffStats {
    /// Crossings that handed control to another device.
    pub departures: u64,
    /// Crossings that brought control back to this device.
    pub arrivals: u64,
    /// Movements that skipped their topology update because the state was busy.
    ///
    /// Should be vanishingly rare. A rising count means the layout is being
    /// rewritten far more often than it should be.
    pub skipped: u64,
}

/// Mutable topology state, guarded for the rare writer.
#[derive(Debug)]
struct State {
    layout: Layout,
    cursor: CursorTracker,
}

/// Decides which machine input belongs to, as the cursor moves.
#[derive(Debug, Clone)]
pub struct Handoff {
    local: DeviceId,
    state: Arc<Mutex<State>>,
    departures: Arc<AtomicU64>,
    arrivals: Arc<AtomicU64>,
    skipped: Arc<AtomicU64>,
}

impl Handoff {
    /// Builds a handoff coordinator starting on a local screen.
    ///
    /// # Errors
    ///
    /// Returns `None` if `start` is not in the layout.
    #[must_use]
    pub fn new(
        local: DeviceId,
        layout: Layout,
        start: GlobalScreenId,
        rules: SwitchingRules,
    ) -> Option<Self> {
        let cursor = CursorTracker::new(&layout, start, rules)?;
        Some(Self {
            local,
            state: Arc::new(Mutex::new(State { layout, cursor })),
            departures: Arc::new(AtomicU64::new(0)),
            arrivals: Arc::new(AtomicU64::new(0)),
            skipped: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Applies pointer movement and reports what it means.
    ///
    /// Never blocks. See the module documentation for what happens when the
    /// state is momentarily busy.
    #[must_use]
    pub fn movement(&self, dx: f64, dy: f64, now: std::time::Instant) -> HandoffAction {
        let Ok(mut state) = self.state.try_lock() else {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            return HandoffAction::None;
        };

        let State { layout, cursor } = &mut *state;
        let Movement::Crossed {
            from, to, entry, ..
        } = cursor.apply(layout, dx, dy, now)
        else {
            return HandoffAction::None;
        };

        let was_local = from.device == self.local;
        let now_local = to.device == self.local;

        match (was_local, now_local) {
            (true, false) => {
                self.departures.fetch_add(1, Ordering::Relaxed);
                HandoffAction::Leave {
                    to: to.device,
                    screen: to,
                    entry,
                }
            }
            (false, true) => {
                self.arrivals.fetch_add(1, Ordering::Relaxed);
                HandoffAction::Enter {
                    from: from.device,
                    screen: to,
                    entry,
                }
            }
            // Between two screens of the same device — either both local, or the
            // cursor moving across a remote machine's own multi-monitor setup.
            // Neither changes which machine is receiving input.
            _ => HandoffAction::None,
        }
    }

    /// Where input should currently go.
    #[must_use]
    pub fn destination(&self) -> Destination {
        self.state.lock().map_or(Destination::Local, |state| {
            if state.cursor.screen().device == self.local {
                Destination::Local
            } else {
                Destination::Remote
            }
        })
    }

    /// The screen the cursor is on.
    ///
    /// Returns `None` if the state is momentarily busy.
    #[must_use]
    pub fn screen(&self) -> Option<GlobalScreenId> {
        self.state
            .try_lock()
            .ok()
            .map(|state| state.cursor.screen())
    }

    /// Where the cursor sits within its screen, each axis in `0.0..=1.0`.
    ///
    /// This is what travels to the machine receiving input. A fraction rather
    /// than a coordinate, because the two screens are different sizes at
    /// different scale factors, and only the receiver can turn a position into
    /// one of its own pixels.
    #[must_use]
    pub fn cursor_normalized(&self) -> Option<(f64, f64)> {
        let state = self.state.try_lock().ok()?;
        let screen = state.layout.get(state.cursor.screen())?;
        Some(screen.bounds.normalize(state.cursor.position()))
    }

    /// Replaces the layout, re-placing the cursor if its screen is gone.
    ///
    /// Called on display hotplug and when the user edits the arrangement.
    ///
    /// # Errors
    ///
    /// Returns `false` if the state is unavailable or the layout has no screen
    /// to fall back to.
    pub fn set_layout(&self, layout: Layout) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };

        let current = state.cursor.screen();
        let fallback = layout.get(current).map_or_else(
            || layout.screens().first().map(|s| s.id),
            |screen| Some(screen.id),
        );
        let Some(target) = fallback else {
            return false;
        };

        state.layout = layout;
        if current != target {
            // The screen the cursor was on has gone. Putting it at the centre of
            // whatever remains is arbitrary, but leaving it at coordinates no
            // screen contains would strand it there permanently.
            let State { layout, cursor } = &mut *state;
            cursor.place(layout, target, (0.5, 0.5));
        }
        true
    }

    /// Places the cursor without treating it as a crossing.
    ///
    /// Used when a peer hands control to this machine.
    ///
    /// # Errors
    ///
    /// Returns `false` if the screen is unknown or the state is unavailable.
    pub fn place(&self, screen: GlobalScreenId, entry: (f64, f64)) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let State { layout, cursor } = &mut *state;
        cursor.place(layout, screen, entry)
    }

    /// Updates the switching rules after a settings change.
    pub fn set_rules(&self, rules: SwitchingRules) {
        if let Ok(mut state) = self.state.lock() {
            state.cursor.set_rules(rules);
        }
    }

    /// Counters for diagnostics.
    #[must_use]
    pub fn stats(&self) -> HandoffStats {
        HandoffStats {
            departures: self.departures.load(Ordering::Relaxed),
            arrivals: self.arrivals.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_topology::layout::PlacedScreen;
    use mb_types::{LogicalRect, Scale, ScreenId};
    use std::time::Instant;

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn screen(byte: u8, x: f64, y: f64, w: f64, h: f64) -> PlacedScreen {
        PlacedScreen {
            id: GlobalScreenId::new(device(byte), ScreenId(0)),
            bounds: LogicalRect::from_parts(x, y, w, h).expect("valid"),
            scale: Scale::ONE,
        }
    }

    /// Local machine on the left, a peer on the right.
    fn handoff() -> Handoff {
        let layout = Layout::new(vec![
            screen(1, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 1920.0, 0.0, 1920.0, 1080.0),
        ])
        .expect("valid");
        Handoff::new(
            device(1),
            layout,
            GlobalScreenId::new(device(1), ScreenId(0)),
            SwitchingRules::default(),
        )
        .expect("starts on a known screen")
    }

    /// Pushes until something other than `None` happens.
    fn push(handoff: &Handoff, dx: f64, times: usize, now: Instant) -> HandoffAction {
        let mut last = HandoffAction::None;
        for _ in 0..times {
            last = handoff.movement(dx, 0.0, now);
            if last != HandoffAction::None {
                return last;
            }
        }
        last
    }

    #[test]
    fn the_normalised_position_tracks_the_cursor() {
        // What actually travels to the other machine. A stale or zeroed value
        // here puts the remote pointer in the wrong place, or nowhere.
        let handoff = handoff();
        let start = handoff.cursor_normalized().expect("has a position");
        assert!((start.0 - 0.5).abs() < 0.01, "starts centred: {start:?}");

        let _ = handoff.movement(400.0, 0.0, Instant::now());
        let moved = handoff.cursor_normalized().expect("has a position");
        assert!(
            moved.0 > start.0,
            "moving right did not change the position"
        );
        assert!((0.0..=1.0).contains(&moved.0));
        assert!((0.0..=1.0).contains(&moved.1));
    }

    #[test]
    fn the_normalised_position_follows_a_crossing() {
        // After handing over, the position must be relative to the *new* screen,
        // or the remote pointer lands wherever the old screen's fraction happened
        // to point.
        let handoff = handoff();
        let now = Instant::now();
        let _ = handoff.movement(1000.0, 0.0, now);
        push(&handoff, 5.0, 40, now);
        assert_eq!(handoff.destination(), Destination::Remote);

        let entry = handoff.cursor_normalized().expect("has a position");
        assert!(
            entry.0 < 0.05,
            "should be at the left edge of the new screen: {entry:?}"
        );
    }

    #[test]
    fn input_starts_local() {
        let handoff = handoff();
        assert_eq!(handoff.destination(), Destination::Local);
    }

    #[test]
    fn crossing_to_a_peer_hands_over() {
        let handoff = handoff();
        let now = Instant::now();

        let _ = handoff.movement(1000.0, 0.0, now);
        let action = push(&handoff, 5.0, 40, now);

        let HandoffAction::Leave { to, entry, .. } = action else {
            panic!("never handed over: {action:?}");
        };
        assert_eq!(to, device(2));
        assert!(entry.0 < 0.01, "should enter at the left edge: {entry:?}");
        assert_eq!(handoff.destination(), Destination::Remote);
        assert_eq!(handoff.stats().departures, 1);
    }

    #[test]
    fn crossing_back_returns_control() {
        let handoff = handoff();
        let start = Instant::now();

        let _ = handoff.movement(1000.0, 0.0, start);
        push(&handoff, 5.0, 40, start);
        assert_eq!(handoff.destination(), Destination::Remote);

        // Past the cooldown, come back.
        let later = start + std::time::Duration::from_millis(500);
        let _ = handoff.movement(-1000.0, 0.0, later);
        let action = push(&handoff, -5.0, 40, later);

        let HandoffAction::Enter { from, .. } = action else {
            panic!("never came back: {action:?}");
        };
        assert_eq!(from, device(2));
        assert_eq!(handoff.destination(), Destination::Local);
        assert_eq!(handoff.stats().arrivals, 1);
    }

    #[test]
    fn moving_between_two_local_screens_is_not_a_handoff() {
        // A local multi-monitor setup. The cursor changes screen, but the machine
        // receiving input does not change, so nothing should be sent anywhere.
        let layout = Layout::new(vec![
            screen(1, 0.0, 0.0, 1920.0, 1080.0),
            PlacedScreen {
                id: GlobalScreenId::new(device(1), ScreenId(1)),
                bounds: LogicalRect::from_parts(1920.0, 0.0, 1920.0, 1080.0).expect("valid"),
                scale: Scale::ONE,
            },
        ])
        .expect("valid");
        let handoff = Handoff::new(
            device(1),
            layout,
            GlobalScreenId::new(device(1), ScreenId(0)),
            SwitchingRules::default(),
        )
        .expect("starts");

        let now = Instant::now();
        let _ = handoff.movement(1000.0, 0.0, now);
        let action = push(&handoff, 5.0, 40, now);

        assert_eq!(
            action,
            HandoffAction::None,
            "a local screen change handed over"
        );
        assert_eq!(handoff.destination(), Destination::Local);
        assert_eq!(handoff.stats().departures, 0);
    }

    #[test]
    fn a_removed_screen_re_places_the_cursor_rather_than_stranding_it() {
        // Display hotplug. Leaving the cursor at coordinates no screen contains
        // would strand it there permanently.
        let handoff = handoff();
        let now = Instant::now();
        let _ = handoff.movement(1000.0, 0.0, now);
        push(&handoff, 5.0, 40, now);
        assert_eq!(handoff.destination(), Destination::Remote);

        // The peer's screen disappears.
        let smaller = Layout::new(vec![screen(1, 0.0, 0.0, 1920.0, 1080.0)]).expect("valid");
        assert!(handoff.set_layout(smaller));

        assert_eq!(handoff.destination(), Destination::Local);
        assert_eq!(
            handoff.screen(),
            Some(GlobalScreenId::new(device(1), ScreenId(0)))
        );
    }

    #[test]
    fn a_layout_that_still_contains_the_current_screen_keeps_the_cursor() {
        let handoff = handoff();
        let before = handoff.screen();

        let rearranged = Layout::new(vec![
            screen(1, 0.0, 0.0, 1920.0, 1080.0),
            screen(2, 0.0, 1080.0, 1920.0, 1080.0),
        ])
        .expect("valid");
        assert!(handoff.set_layout(rearranged));

        assert_eq!(handoff.screen(), before, "the cursor jumped unnecessarily");
    }

    #[test]
    fn placement_from_a_peer_is_not_a_handoff() {
        // A peer handing control here must not be counted as a crossing, or the
        // cooldown would be consumed by an event the user did not cause.
        let handoff = handoff();
        assert!(handoff.place(GlobalScreenId::new(device(2), ScreenId(0)), (0.0, 0.5)));

        assert_eq!(handoff.destination(), Destination::Remote);
        assert_eq!(handoff.stats().departures, 0);
        assert_eq!(handoff.stats().arrivals, 0);
    }

    #[test]
    fn an_unknown_screen_cannot_be_placed_on() {
        let handoff = handoff();
        assert!(!handoff.place(GlobalScreenId::new(device(9), ScreenId(0)), (0.5, 0.5)));
        assert_eq!(handoff.destination(), Destination::Local);
    }

    #[test]
    fn a_busy_state_skips_the_update_rather_than_blocking() {
        // The capture thread must never wait. A stale answer for one event is
        // acceptable; a stalled input thread gets the tap disabled by the OS.
        let handoff = handoff();
        let guard = handoff.state.lock().expect("lock");

        let action = handoff.movement(1000.0, 0.0, Instant::now());
        assert_eq!(action, HandoffAction::None);
        assert_eq!(handoff.stats().skipped, 1);

        drop(guard);
    }

    #[test]
    fn starting_on_an_unknown_screen_is_refused() {
        let layout = Layout::new(vec![screen(1, 0.0, 0.0, 100.0, 100.0)]).expect("valid");
        assert!(
            Handoff::new(
                device(1),
                layout,
                GlobalScreenId::new(device(9), ScreenId(0)),
                SwitchingRules::default()
            )
            .is_none()
        );
    }
}
