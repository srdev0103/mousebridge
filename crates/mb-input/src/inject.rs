//! Injecting input into the operating system.

use crate::error::InputError;
use crate::event::InputEvent;
use crate::state::InputStateTracker;

/// A platform's input injection backend.
///
/// Implementations receive only events that have already been validated — see
/// [`Validated`], which is how that is guaranteed rather than merely requested.
pub trait InputInject: Send {
    /// Injects one event.
    ///
    /// # Errors
    ///
    /// [`InputError::PermissionDenied`] if the OS gate is missing, or
    /// [`InputError::OsCall`] if the platform call failed.
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError>;

    /// Releases every key and button this injector is holding.
    ///
    /// The recovery path. Must be safe to call at any time, including when
    /// nothing is held and when the connection that produced the presses is
    /// already gone.
    ///
    /// # Errors
    ///
    /// [`InputError::OsCall`] if a release could not be delivered. An
    /// implementation must attempt *every* release even if one fails, and report
    /// the first error afterwards: abandoning the sequence half-way is how a
    /// modifier ends up stuck.
    fn release_all(&mut self) -> Result<(), InputError>;
}

/// Wraps an injector so malformed events can never reach the operating system.
///
/// Validation is a wrapper rather than a default trait method because a default
/// method can be overridden, and "please remember to validate" is not a
/// guarantee. Wiring the platform injector inside `Validated` makes the check
/// unavoidable: there is no path to the inner injector that skips it.
///
/// The threat is concrete. Injected events originate from another machine over
/// the network. A NaN coordinate reaching `CGWarpMouseCursorPosition` or
/// `SendInput` is undefined behaviour inside whatever application has focus, not
/// a cosmetic glitch, and a hostile peer could send one deliberately.
#[derive(Debug)]
pub struct Validated<T> {
    inner: T,
    /// Events rejected since construction, for diagnostics.
    rejected: u64,
}

impl<T: InputInject> Validated<T> {
    /// Wraps an injector.
    pub const fn new(inner: T) -> Self {
        Self { inner, rejected: 0 }
    }

    /// Number of events rejected as malformed.
    ///
    /// A non-zero count on a healthy LAN means a peer is misbehaving, and is
    /// worth surfacing in diagnostics.
    pub const fn rejected_count(&self) -> u64 {
        self.rejected
    }

    /// Returns the wrapped injector.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: InputInject> InputInject for Validated<T> {
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError> {
        if !event.is_finite() {
            self.rejected += 1;
            return Err(InputError::Malformed {
                reason: "coordinate is not a finite number",
            });
        }
        self.inner.inject(event)
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        self.inner.release_all()
    }
}

/// Injects a sequence, attempting every event even if one fails.
///
/// Stopping at the first error is wrong for a release sequence: abandoning it
/// half-way leaves the remaining keys held, which is the failure this system must
/// never produce. The first error is returned once the whole sequence has been
/// attempted.
///
/// # Errors
///
/// The first error encountered, after every event has been attempted.
pub fn inject_all<T: InputInject + ?Sized>(
    injector: &mut T,
    events: &[InputEvent],
) -> Result<(), InputError> {
    let mut first_error = None;
    for event in events {
        if let Err(e) = injector.inject(event)
            && first_error.is_none()
        {
            first_error = Some(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Builds the release sequence for a tracked state and injects it.
///
/// # Errors
///
/// As [`inject_all`].
pub fn release_tracked<T: InputInject + ?Sized>(
    injector: &mut T,
    tracker: &mut InputStateTracker,
) -> Result<(), InputError> {
    let events = tracker.release_events();
    let result = inject_all(injector, &events);
    // Cleared unconditionally. If a release could not be delivered, the OS state
    // is already beyond our knowledge, and continuing to believe we hold the key
    // would only suppress the next attempt to release it.
    tracker.clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::keys;
    use crate::modifiers::MouseButton;

    #[derive(Default)]
    struct RecordingInjector {
        events: Vec<InputEvent>,
        fail_on: Option<usize>,
        released: usize,
    }

    impl InputInject for RecordingInjector {
        fn inject(&mut self, event: &InputEvent) -> Result<(), InputError> {
            let index = self.events.len();
            self.events.push(event.clone());
            if self.fail_on == Some(index) {
                return Err(InputError::OsCall {
                    api: "test",
                    detail: "injected failure".to_owned(),
                });
            }
            Ok(())
        }
        fn release_all(&mut self) -> Result<(), InputError> {
            self.released += 1;
            Ok(())
        }
    }

    #[test]
    fn non_finite_events_never_reach_the_inner_injector() {
        let mut injector = Validated::new(RecordingInjector::default());
        let bad = InputEvent::MouseMoveTo {
            x: f64::NAN,
            y: 0.0,
        };
        assert!(matches!(
            injector.inject(&bad),
            Err(InputError::Malformed { .. })
        ));
        assert_eq!(injector.rejected_count(), 1);
        assert!(
            injector.into_inner().events.is_empty(),
            "a NaN reached the platform layer"
        );
    }

    #[test]
    fn valid_events_pass_through_unchanged() {
        let mut injector = Validated::new(RecordingInjector::default());
        let good = InputEvent::MouseMoveTo { x: 12.0, y: 34.0 };
        injector.inject(&good).expect("accepted");
        assert_eq!(injector.rejected_count(), 0);
        assert_eq!(injector.into_inner().events, vec![good]);
    }

    #[test]
    fn a_failure_mid_sequence_does_not_abandon_the_rest() {
        // The whole point: if releasing Shift fails, Ctrl must still be released.
        let mut injector = RecordingInjector {
            fail_on: Some(1),
            ..Default::default()
        };
        let events = vec![
            InputEvent::Key {
                key: keys::A,
                pressed: false,
                repeat: false,
            },
            InputEvent::Key {
                key: keys::LEFT_SHIFT,
                pressed: false,
                repeat: false,
            },
            InputEvent::Key {
                key: keys::LEFT_CTRL,
                pressed: false,
                repeat: false,
            },
        ];
        let result = inject_all(&mut injector, &events);
        assert!(result.is_err(), "the failure must still be reported");
        assert_eq!(
            injector.events.len(),
            3,
            "the sequence was abandoned after the failure"
        );
    }

    #[test]
    fn release_tracked_clears_state_even_when_injection_fails() {
        // If the OS rejected the release, our belief about what is held is
        // already wrong. Holding on to it would suppress the next attempt.
        let mut injector = RecordingInjector {
            fail_on: Some(0),
            ..Default::default()
        };
        let mut tracker = InputStateTracker::new();
        tracker.apply(&InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });
        tracker.apply(&InputEvent::Key {
            key: keys::LEFT_CTRL,
            pressed: true,
            repeat: false,
        });

        assert!(release_tracked(&mut injector, &mut tracker).is_err());
        assert!(tracker.is_clean(), "tracker kept stale held state");
        assert_eq!(injector.events.len(), 2, "both releases were attempted");
    }

    #[test]
    fn release_tracked_on_a_clean_tracker_is_a_no_op() {
        let mut injector = RecordingInjector::default();
        let mut tracker = InputStateTracker::new();
        release_tracked(&mut injector, &mut tracker).expect("no-op succeeds");
        assert!(injector.events.is_empty());
    }
}
