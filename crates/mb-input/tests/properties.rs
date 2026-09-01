//! Property tests for the input state machine.
//!
//! The example-based tests in `state.rs` check the cases we thought of. These
//! check the ones we did not: they assert the invariants hold for *any* sequence
//! of events, including sequences no real keyboard would produce — duplicate
//! presses, releases without presses, interleaved `Enter`/`Leave`, and hostile
//! values arriving from the network.
//!
//! The central invariant is the product requirement stated directly:
//! **whatever has happened, the system can always get back to holding nothing.**

use mb_input::event::HeldInput;
use mb_input::inject::{Validated, release_tracked};
use mb_input::keycode::{PAGE_CONSUMER, PAGE_KEYBOARD};
use mb_input::state::MAX_HELD_KEYS;
use mb_input::virtual_backend::VirtualInjector;
use mb_input::{
    InputEvent, InputInject, InputStateTracker, KeyCode, Modifiers, MouseButton, MouseButtons,
    ScrollDelta,
};
use proptest::prelude::*;

/// Keys drawn from a small pool so collisions — repeated presses of the same key
/// — actually occur. A uniformly random usage would almost never repeat, and
/// repeats are where the interesting bugs live.
fn arb_key() -> impl Strategy<Value = KeyCode> {
    prop_oneof![
        // Modifiers, which take the bit-set path.
        (0xE0u16..=0xE7).prop_map(KeyCode::keyboard),
        // A handful of ordinary keys, to force repeats.
        (0x04u16..=0x0F).prop_map(KeyCode::keyboard),
        // Anything at all, including usages we have no name for.
        (0x00u16..=0xFFFF).prop_map(KeyCode::keyboard),
        (0x00u16..=0xFFFF).prop_map(KeyCode::consumer),
    ]
}

fn arb_button() -> impl Strategy<Value = MouseButton> {
    prop_oneof![
        Just(MouseButton::Left),
        Just(MouseButton::Right),
        Just(MouseButton::Middle),
        Just(MouseButton::Back),
        Just(MouseButton::Forward),
    ]
}

/// Any event, including malformed ones. Coordinates deliberately include NaN and
/// infinity, because those can arrive from a hostile or buggy peer.
fn arb_event() -> impl Strategy<Value = InputEvent> {
    prop_oneof![
        (any::<f64>(), any::<f64>()).prop_map(|(dx, dy)| InputEvent::MouseMove { dx, dy }),
        (any::<f64>(), any::<f64>()).prop_map(|(x, y)| InputEvent::MouseMoveTo { x, y }),
        (arb_button(), any::<bool>())
            .prop_map(|(button, pressed)| InputEvent::MouseButton { button, pressed }),
        (any::<f32>(), any::<f32>(), any::<bool>()).prop_map(|(x, y, precise)| {
            InputEvent::MouseWheel {
                delta: ScrollDelta { x, y, precise },
            }
        }),
        (arb_key(), any::<bool>(), any::<bool>()).prop_map(|(key, pressed, repeat)| {
            InputEvent::Key {
                key,
                pressed,
                repeat,
            }
        }),
        (
            any::<u32>(),
            any::<f64>(),
            any::<f64>(),
            prop::collection::vec(arb_key(), 0..4),
            any::<u8>(),
            any::<u8>(),
        )
            .prop_map(|(screen, x, y, keys, mods, buttons)| InputEvent::Enter {
                screen: mb_types::ScreenId(screen),
                position: (x, y),
                held: HeldInput {
                    keys,
                    modifiers: Modifiers::from_bits(mods),
                    buttons: MouseButtons::from_bits(buttons),
                },
            }),
        Just(InputEvent::Leave),
        Just(InputEvent::ReleaseAll),
    ]
}

proptest! {
    /// The product requirement, stated as a property: after *any* history, the
    /// release sequence returns the machine to holding nothing.
    #[test]
    fn release_events_always_reach_a_clean_state(
        events in prop::collection::vec(arb_event(), 0..200)
    ) {
        let mut tracker = InputStateTracker::new();
        for event in &events {
            tracker.apply(event);
        }

        let release = tracker.release_events();
        for event in &release {
            tracker.apply(event);
        }

        prop_assert!(
            tracker.is_clean(),
            "still holding {:?} / {} after {} release events",
            tracker.keys(),
            tracker.modifiers(),
            release.len()
        );
    }

    /// `ReleaseAll` is the unconditional escape hatch and must never fail to clear.
    #[test]
    fn release_all_always_clears(
        events in prop::collection::vec(arb_event(), 0..200)
    ) {
        let mut tracker = InputStateTracker::new();
        for event in &events {
            tracker.apply(event);
        }
        tracker.apply(&InputEvent::ReleaseAll);
        prop_assert!(tracker.is_clean());
    }

    /// The fixed-capacity storage must never be exceeded, whatever arrives.
    #[test]
    fn tracked_keys_never_exceed_capacity(
        events in prop::collection::vec(arb_event(), 0..400)
    ) {
        let mut tracker = InputStateTracker::new();
        for event in &events {
            tracker.apply(event);
            prop_assert!(tracker.keys().len() <= MAX_HELD_KEYS);
        }
    }

    /// A key is either in the key list or in the modifier set, never both.
    ///
    /// Violating this is how a modifier gets released once and stays held: the
    /// release clears one representation and leaves the other.
    #[test]
    fn a_key_is_never_tracked_in_two_places(
        events in prop::collection::vec(arb_event(), 0..200)
    ) {
        let mut tracker = InputStateTracker::new();
        for event in &events {
            tracker.apply(event);
            for key in tracker.keys() {
                prop_assert!(
                    !key.is_modifier(),
                    "{key} is a modifier but occupies a key slot"
                );
            }
        }
    }

    /// The key list must never contain duplicates: one release must be enough.
    #[test]
    fn tracked_keys_are_unique(
        events in prop::collection::vec(arb_event(), 0..200)
    ) {
        let mut tracker = InputStateTracker::new();
        for event in &events {
            tracker.apply(event);
            let mut seen = tracker.keys().to_vec();
            let before = seen.len();
            seen.sort_unstable();
            seen.dedup();
            prop_assert_eq!(seen.len(), before, "a key was tracked twice");
        }
    }

    /// Reconstructing a state elsewhere and then tearing it down must be clean.
    #[test]
    fn press_then_release_round_trips_on_a_fresh_machine(
        events in prop::collection::vec(arb_event(), 0..100)
    ) {
        let mut source = InputStateTracker::new();
        for event in &events {
            source.apply(event);
        }

        let mut target = InputStateTracker::new();
        for event in source.press_events() {
            target.apply(&event);
        }
        prop_assert_eq!(target.keys(), source.keys());
        prop_assert_eq!(target.modifiers(), source.modifiers());
        prop_assert_eq!(target.buttons(), source.buttons());

        for event in target.release_events() {
            target.apply(&event);
        }
        prop_assert!(target.is_clean());
    }

    /// No malformed event ever reaches the platform injector.
    ///
    /// The events here include NaN and infinite coordinates, exactly what a
    /// hostile peer would send. A NaN reaching a platform injection call is
    /// undefined behaviour in the focused application, not a cosmetic glitch.
    #[test]
    fn validation_blocks_every_non_finite_event(
        events in prop::collection::vec(arb_event(), 0..200)
    ) {
        let mut injector = Validated::new(VirtualInjector::new());
        let mut expected_rejects = 0u64;

        for event in &events {
            if !event.is_finite() {
                expected_rejects += 1;
            }
            let _ = injector.inject(event);
        }

        prop_assert_eq!(injector.rejected_count(), expected_rejects);
        for event in injector.into_inner().events() {
            prop_assert!(event.is_finite(), "{event} reached the platform layer");
        }
    }

    /// A simulated machine can always be returned to holding nothing, even when
    /// the injection that would release something fails.
    #[test]
    fn a_machine_can_always_be_cleaned_up(
        events in prop::collection::vec(arb_event(), 0..100),
        fail_at in 0usize..20,
    ) {
        let mut injector = VirtualInjector::new();
        for (i, event) in events.iter().enumerate() {
            if i == fail_at {
                injector.fail_next(mb_input::InputError::OsCall {
                    api: "test",
                    detail: "simulated".to_owned(),
                });
            }
            let _ = injector.inject(event);
        }

        let mut tracker = injector.tracker().clone();
        let _ = release_tracked(&mut injector, &mut tracker);
        prop_assert!(tracker.is_clean());

        prop_assert!(injector.release_all().is_ok());
        prop_assert!(injector.is_clean(), "machine left holding input");
    }

    /// Page and usage must both matter: 0xE2 is Left Alt on the keyboard page and
    /// Mute on the consumer page. Conflating them would send a modifier when the
    /// user pressed a volume key.
    #[test]
    fn page_disambiguates_identical_usages(usage in 0xE0u16..=0xE7) {
        let keyboard = KeyCode::new(PAGE_KEYBOARD, usage);
        let consumer = KeyCode::new(PAGE_CONSUMER, usage);
        prop_assert_ne!(keyboard, consumer);
        prop_assert!(keyboard.is_modifier());
        prop_assert!(!consumer.is_modifier());
    }
}
