//! Tracking what is physically held, and getting back to a clean state.
//!
//! This module exists for one requirement: **never leave the user's keyboard or
//! mouse in a stuck state.** If the network dies while `Ctrl` is held on a remote
//! machine, that machine must be able to recover on its own. That is only
//! possible if both ends know exactly what they are holding.

use crate::event::{HeldInput, InputEvent};
use crate::keycode::KeyCode;
use crate::modifiers::{Modifiers, MouseButtons};

/// Maximum simultaneously held non-modifier keys.
///
/// Chosen so the tracker is a fixed-size value with no heap allocation, which
/// matters because [`InputStateTracker::apply`] runs on the capture thread.
/// Thirty-two exceeds any physical keyboard's rollover — full NKRO hardware
/// reports at most a handful of simultaneous keys in practice — so reaching this
/// limit indicates desynchronisation rather than a fast typist.
pub const MAX_HELD_KEYS: usize = 32;

/// Placeholder occupying unused slots. Usage 0 is "no event" in HID, so it can
/// never collide with a real key.
const EMPTY_KEY: KeyCode = KeyCode::new(0, 0);

/// What [`InputStateTracker::apply`] did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// State changed: the key or button is now held, or is now released.
    Changed,
    /// The event carried no new information.
    ///
    /// A press of an already-held key, an auto-repeat, or a release of something
    /// that was not held. All three are normal — the last one happens routinely
    /// after a [`InputEvent::ReleaseAll`] — and none is an error.
    Redundant,
    /// The event did not affect tracked state, such as pointer motion.
    Irrelevant,
    /// More than [`MAX_HELD_KEYS`] keys were held at once, so this one is not
    /// being tracked.
    ///
    /// Deliberately *not* silent. An untracked key cannot be released later,
    /// which is precisely the stuck-key failure this module exists to prevent, so
    /// the caller must respond by forcing a full release rather than continuing.
    CapacityExceeded,
}

/// Tracks the keys and buttons currently held.
///
/// Used on both sides: the sender tracks what the user physically holds, and the
/// receiver tracks what it has synthesised. The receiver's copy is what lets it
/// clean up autonomously when the connection dies.
#[derive(Debug, Clone)]
pub struct InputStateTracker {
    keys: [KeyCode; MAX_HELD_KEYS],
    key_count: usize,
    modifiers: Modifiers,
    buttons: MouseButtons,
}

impl Default for InputStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl InputStateTracker {
    /// Builds an empty tracker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: [EMPTY_KEY; MAX_HELD_KEYS],
            key_count: 0,
            modifiers: Modifiers::NONE,
            buttons: MouseButtons::NONE,
        }
    }

    /// Updates state from an event.
    ///
    /// Allocation-free and branch-light: this runs on the capture thread, where a
    /// heap allocation or a lock would show up as input latency.
    pub fn apply(&mut self, event: &InputEvent) -> ApplyOutcome {
        match event {
            InputEvent::Key {
                key,
                pressed,
                repeat,
            } => self.apply_key(*key, *pressed, *repeat),
            InputEvent::MouseButton { button, pressed } => {
                let held = self.buttons.contains(*button);
                if held == *pressed {
                    return ApplyOutcome::Redundant;
                }
                self.buttons = if *pressed {
                    self.buttons.with(*button)
                } else {
                    self.buttons.without(*button)
                };
                ApplyOutcome::Changed
            }
            InputEvent::ReleaseAll | InputEvent::Leave => {
                let was_clean = self.is_clean();
                self.clear();
                if was_clean {
                    ApplyOutcome::Redundant
                } else {
                    ApplyOutcome::Changed
                }
            }
            InputEvent::Enter { held, .. } => {
                self.adopt(held);
                ApplyOutcome::Changed
            }
            InputEvent::MouseMove { .. }
            | InputEvent::MouseMoveTo { .. }
            | InputEvent::MouseWheel { .. } => ApplyOutcome::Irrelevant,
        }
    }

    fn apply_key(&mut self, key: KeyCode, pressed: bool, repeat: bool) -> ApplyOutcome {
        if key.is_modifier() {
            let already = self.modifiers.with_key(key) == self.modifiers;
            if already == pressed {
                return ApplyOutcome::Redundant;
            }
            self.modifiers = if pressed {
                self.modifiers.with_key(key)
            } else {
                self.modifiers.without_key(key)
            };
            return ApplyOutcome::Changed;
        }

        let position = self.keys[..self.key_count].iter().position(|k| *k == key);

        if pressed {
            if position.is_some() {
                // An auto-repeat, or a duplicate press from a desynchronised
                // peer. Either way the key is already held; counting it twice
                // would make the release arithmetic wrong.
                return ApplyOutcome::Redundant;
            }
            // A repeat for a key we have no record of means capture started
            // mid-hold. Tracking it is the safe choice: an untracked held key is
            // one we can never release.
            let _ = repeat;
            if self.key_count >= MAX_HELD_KEYS {
                return ApplyOutcome::CapacityExceeded;
            }
            self.keys[self.key_count] = key;
            self.key_count += 1;
            ApplyOutcome::Changed
        } else {
            let Some(index) = position else {
                // Releasing something we never saw pressed. Routine after a
                // ReleaseAll, and never an error.
                return ApplyOutcome::Redundant;
            };
            // Order-preserving removal: `release_events` replays presses in the
            // order they happened, which keeps behaviour predictable.
            self.keys.copy_within(index + 1..self.key_count, index);
            self.key_count -= 1;
            self.keys[self.key_count] = EMPTY_KEY;
            ApplyOutcome::Changed
        }
    }

    /// Replaces tracked state with what a peer says it is holding.
    ///
    /// The peer's list is untrusted: it may repeat a key, exceed capacity, or mix
    /// modifiers into the key list. Duplicates in particular are dangerous — a key
    /// stored twice needs two releases, but only one arrives, so it stays held
    /// forever. Found by the `tracked_keys_are_unique` property test.
    fn adopt(&mut self, held: &HeldInput) {
        self.clear();
        self.modifiers = held.modifiers;
        self.buttons = held.buttons;
        for key in &held.keys {
            if key.is_modifier() {
                self.modifiers = self.modifiers.with_key(*key);
            } else if self.key_count < MAX_HELD_KEYS && !self.keys[..self.key_count].contains(key) {
                self.keys[self.key_count] = *key;
                self.key_count += 1;
            }
        }
    }

    /// Forgets everything without emitting events.
    pub fn clear(&mut self) {
        self.keys = [EMPTY_KEY; MAX_HELD_KEYS];
        self.key_count = 0;
        self.modifiers = Modifiers::NONE;
        self.buttons = MouseButtons::NONE;
    }

    /// True when nothing is held.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.key_count == 0 && self.modifiers.is_empty() && self.buttons.is_empty()
    }

    /// Modifiers currently held.
    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Mouse buttons currently held.
    #[must_use]
    pub const fn buttons(&self) -> MouseButtons {
        self.buttons
    }

    /// Non-modifier keys currently held, in press order.
    #[must_use]
    pub fn keys(&self) -> &[KeyCode] {
        &self.keys[..self.key_count]
    }

    /// Snapshot for handing state to another machine at a boundary crossing.
    #[must_use]
    pub fn held(&self) -> HeldInput {
        HeldInput {
            keys: self.keys().to_vec(),
            modifiers: self.modifiers,
            buttons: self.buttons,
        }
    }

    /// The events that return the system to a clean state.
    ///
    /// # Ordering
    ///
    /// Buttons, then ordinary keys, then modifiers — the order a person's hand
    /// actually relaxes. Modifiers last matters: releasing `Shift` while a
    /// letter is still down briefly presents an unshifted chord to the focused
    /// application, and some applications act on that.
    ///
    /// # Platform note
    ///
    /// Releasing a lone `Meta` on Windows opens the Start menu, and a lone `Alt`
    /// focuses the menu bar. When a release sequence would end with one of those
    /// alone, the Windows injector must neutralise it — conventionally by
    /// bracketing with an inert key. That belongs in the platform layer, not
    /// here, and is tracked in `docs/platform-validation.md`.
    ///
    /// Allocates, and is meant to: this runs on boundary crossings, disconnects
    /// and sleep, never on the per-event path.
    #[must_use]
    pub fn release_events(&self) -> Vec<InputEvent> {
        let mut events =
            Vec::with_capacity(self.key_count + self.modifiers.bits().count_ones() as usize + 5);

        for button in self.buttons.held() {
            events.push(InputEvent::MouseButton {
                button,
                pressed: false,
            });
        }
        for key in self.keys().iter().rev() {
            events.push(InputEvent::Key {
                key: *key,
                pressed: false,
                repeat: false,
            });
        }
        for key in self.modifiers.held_keys() {
            events.push(InputEvent::Key {
                key,
                pressed: false,
                repeat: false,
            });
        }
        events
    }

    /// The events that re-assert this state on another machine.
    ///
    /// The mirror of [`InputStateTracker::release_events`]: modifiers first, so
    /// that a chord is reconstructed the way the user formed it.
    #[must_use]
    pub fn press_events(&self) -> Vec<InputEvent> {
        let mut events = Vec::with_capacity(self.key_count + 8);
        for key in self.modifiers.held_keys() {
            events.push(InputEvent::Key {
                key,
                pressed: true,
                repeat: false,
            });
        }
        for key in self.keys() {
            events.push(InputEvent::Key {
                key: *key,
                pressed: true,
                repeat: false,
            });
        }
        for button in self.buttons.held() {
            events.push(InputEvent::MouseButton {
                button,
                pressed: true,
            });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ScrollDelta;
    use crate::keycode::keys;
    use crate::modifiers::MouseButton;

    fn press(key: KeyCode) -> InputEvent {
        InputEvent::Key {
            key,
            pressed: true,
            repeat: false,
        }
    }
    fn release(key: KeyCode) -> InputEvent {
        InputEvent::Key {
            key,
            pressed: false,
            repeat: false,
        }
    }

    #[test]
    fn tracks_a_simple_press_and_release() {
        let mut t = InputStateTracker::new();
        assert!(t.is_clean());
        assert_eq!(t.apply(&press(keys::A)), ApplyOutcome::Changed);
        assert!(!t.is_clean());
        assert_eq!(t.keys(), &[keys::A]);
        assert_eq!(t.apply(&release(keys::A)), ApplyOutcome::Changed);
        assert!(t.is_clean());
    }

    #[test]
    fn modifiers_are_tracked_separately_from_keys() {
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::LEFT_SHIFT));
        t.apply(&press(keys::A));
        assert_eq!(t.modifiers(), Modifiers::LEFT_SHIFT);
        assert_eq!(
            t.keys(),
            &[keys::A],
            "a modifier must not occupy a key slot"
        );
    }

    #[test]
    fn auto_repeat_does_not_double_count() {
        // The classic stuck-key bug: counting repeats as presses, so one release
        // is not enough to clear the key.
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::A));
        for _ in 0..50 {
            let repeat = InputEvent::Key {
                key: keys::A,
                pressed: true,
                repeat: true,
            };
            assert_eq!(t.apply(&repeat), ApplyOutcome::Redundant);
        }
        t.apply(&release(keys::A));
        assert!(t.is_clean(), "key survived its release");
    }

    #[test]
    fn a_repeat_without_a_preceding_press_is_still_tracked() {
        // Happens when capture starts while a key is already held. Tracking it is
        // the safe choice: an untracked held key can never be released.
        let mut t = InputStateTracker::new();
        let repeat = InputEvent::Key {
            key: keys::A,
            pressed: true,
            repeat: true,
        };
        assert_eq!(t.apply(&repeat), ApplyOutcome::Changed);
        assert_eq!(t.keys(), &[keys::A]);
    }

    #[test]
    fn releasing_something_never_pressed_is_not_an_error() {
        let mut t = InputStateTracker::new();
        assert_eq!(t.apply(&release(keys::A)), ApplyOutcome::Redundant);
        assert_eq!(
            t.apply(&InputEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false
            }),
            ApplyOutcome::Redundant
        );
        assert!(t.is_clean());
    }

    #[test]
    fn release_events_clear_everything_they_report() {
        // The core invariant: applying what release_events() returns must always
        // reach a clean state, whatever was held.
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::LEFT_CTRL));
        t.apply(&press(keys::RIGHT_SHIFT));
        t.apply(&press(keys::A));
        t.apply(&press(keys::Q));
        t.apply(&InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });

        let events = t.release_events();
        let mut replay = t.clone();
        for e in &events {
            replay.apply(e);
        }
        assert!(replay.is_clean(), "left holding {:?}", replay.keys());
    }

    #[test]
    fn release_order_is_buttons_then_keys_then_modifiers() {
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::LEFT_SHIFT));
        t.apply(&press(keys::A));
        t.apply(&InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });

        let events = t.release_events();
        assert!(matches!(events[0], InputEvent::MouseButton { .. }));
        assert!(
            matches!(events[1], InputEvent::Key { key, .. } if key == keys::A),
            "ordinary keys must precede modifiers"
        );
        assert!(matches!(events[2], InputEvent::Key { key, .. } if key == keys::LEFT_SHIFT));
    }

    #[test]
    fn release_all_clears_and_is_idempotent() {
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::LEFT_META));
        t.apply(&press(keys::Q));
        assert_eq!(t.apply(&InputEvent::ReleaseAll), ApplyOutcome::Changed);
        assert!(t.is_clean());
        // Sending one more must always be safe: it is emitted defensively on
        // every crossing, disconnect and wake.
        assert_eq!(t.apply(&InputEvent::ReleaseAll), ApplyOutcome::Redundant);
        assert!(t.is_clean());
    }

    #[test]
    fn leave_also_clears() {
        // Leaving without clearing is exactly how a key gets stuck on the
        // machine the user just walked away from.
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::LEFT_CTRL));
        t.apply(&InputEvent::Leave);
        assert!(t.is_clean());
    }

    #[test]
    fn enter_adopts_the_peers_held_state() {
        // Crossing a boundary mid-drag with Shift held must not drop either.
        let mut t = InputStateTracker::new();
        t.apply(&InputEvent::Enter {
            screen: mb_types::ScreenId(0),
            position: (0.0, 0.5),
            held: HeldInput {
                keys: vec![keys::A],
                modifiers: Modifiers::LEFT_SHIFT,
                buttons: MouseButtons::NONE.with(MouseButton::Left),
            },
        });
        assert_eq!(t.keys(), &[keys::A]);
        assert_eq!(t.modifiers(), Modifiers::LEFT_SHIFT);
        assert!(t.buttons().contains(MouseButton::Left));
    }

    #[test]
    fn enter_with_a_modifier_in_the_key_list_still_lands_in_the_modifier_set() {
        // A peer may report modifiers either way; both must be accepted, or the
        // modifier ends up occupying a key slot and never gets released.
        let mut t = InputStateTracker::new();
        t.apply(&InputEvent::Enter {
            screen: mb_types::ScreenId(0),
            position: (0.0, 0.0),
            held: HeldInput {
                keys: vec![keys::LEFT_ALT],
                modifiers: Modifiers::NONE,
                buttons: MouseButtons::NONE,
            },
        });
        assert!(t.keys().is_empty());
        assert_eq!(t.modifiers(), Modifiers::LEFT_ALT);
    }

    #[test]
    fn a_peer_reporting_duplicate_keys_does_not_create_a_stuck_key() {
        // Regression: a duplicated key was stored twice, so the single release
        // that followed left one copy held forever.
        let mut t = InputStateTracker::new();
        t.apply(&InputEvent::Enter {
            screen: mb_types::ScreenId(0),
            position: (0.0, 0.0),
            held: HeldInput {
                keys: vec![keys::A, keys::A, keys::A],
                modifiers: Modifiers::NONE,
                buttons: MouseButtons::NONE,
            },
        });
        assert_eq!(t.keys(), &[keys::A]);
        t.apply(&release(keys::A));
        assert!(t.is_clean());
    }

    #[test]
    fn a_peer_exceeding_capacity_is_truncated_not_overflowed() {
        let many: Vec<KeyCode> = (0..100).map(|i| KeyCode::keyboard(0x04 + i)).collect();
        let mut t = InputStateTracker::new();
        t.apply(&InputEvent::Enter {
            screen: mb_types::ScreenId(0),
            position: (0.0, 0.0),
            held: HeldInput {
                keys: many,
                modifiers: Modifiers::NONE,
                buttons: MouseButtons::NONE,
            },
        });
        assert_eq!(t.keys().len(), MAX_HELD_KEYS);
    }

    #[test]
    fn press_and_release_events_are_inverses() {
        let mut source = InputStateTracker::new();
        source.apply(&press(keys::LEFT_META));
        source.apply(&press(keys::C));
        source.apply(&InputEvent::MouseButton {
            button: MouseButton::Right,
            pressed: true,
        });

        // Reconstruct on a fresh machine, then tear down.
        let mut target = InputStateTracker::new();
        for e in source.press_events() {
            target.apply(&e);
        }
        assert_eq!(target.keys(), source.keys());
        assert_eq!(target.modifiers(), source.modifiers());
        assert_eq!(target.buttons(), source.buttons());

        for e in target.release_events() {
            target.apply(&e);
        }
        assert!(target.is_clean());
    }

    #[test]
    fn capacity_overflow_is_reported_not_swallowed() {
        let mut t = InputStateTracker::new();
        for i in 0..MAX_HELD_KEYS {
            let key = KeyCode::keyboard(0x04 + u16::try_from(i).expect("small"));
            assert_eq!(t.apply(&press(key)), ApplyOutcome::Changed);
        }
        // One more cannot be tracked. Silently dropping it would create exactly
        // the untracked-held-key situation this module exists to prevent.
        let overflow = KeyCode::keyboard(0x400);
        assert_eq!(t.apply(&press(overflow)), ApplyOutcome::CapacityExceeded);
        assert_eq!(t.keys().len(), MAX_HELD_KEYS);
    }

    #[test]
    fn motion_never_touches_tracked_state() {
        let mut t = InputStateTracker::new();
        t.apply(&press(keys::A));
        let before = t.keys().to_vec();
        for e in [
            InputEvent::MouseMove { dx: 5.0, dy: -2.0 },
            InputEvent::MouseMoveTo { x: 10.0, y: 10.0 },
            InputEvent::MouseWheel {
                delta: ScrollDelta::lines(0.0, 1.0),
            },
        ] {
            assert_eq!(t.apply(&e), ApplyOutcome::Irrelevant);
        }
        assert_eq!(t.keys(), before.as_slice());
    }

    #[test]
    fn key_removal_preserves_press_order() {
        let mut t = InputStateTracker::new();
        for k in [keys::A, keys::C, keys::Q, keys::V] {
            t.apply(&press(k));
        }
        t.apply(&release(keys::C));
        assert_eq!(t.keys(), &[keys::A, keys::Q, keys::V]);
    }
}
