//! Turning CoreGraphics event fields into [`InputEvent`]s.
//!
//! The functions here take **plain field values** rather than a `CGEvent`. That
//! is deliberate: it puts every decision — which button, which unit, whether a
//! modifier went down or up — under ordinary unit test, leaving the FFI layer in
//! `tap.rs` as a thin extractor with no logic to get wrong.

use mb_input::event::ScrollDelta;
use mb_input::keycode::KeyCode;
use mb_input::modifiers::{Modifiers, MouseButton};

/// Pixels per line when converting a continuous scroll to line units.
///
/// macOS reports trackpad scrolling in pixels and wheel scrolling in lines, and
/// the wire format carries lines. Ten is the long-standing convention on this
/// platform; it is a constant rather than a literal so the value has one place to
/// be tuned once real hardware says whether it feels right.
pub const PIXELS_PER_LINE: f32 = 10.0;

// Device-dependent modifier masks from IOKit, as they appear in `CGEventFlags`.
//
// The documented `kCGEventFlagMask*` constants cannot distinguish left from
// right — `kCGEventFlagMaskShift` is set for either Shift. These `NX_DEVICE*`
// bits can, and side matters: conflating them corrupts the modifier bit set and
// breaks chords that depend on which side was used.
const NX_LEFT_CTRL: u64 = 0x0000_0001;
const NX_LEFT_SHIFT: u64 = 0x0000_0002;
const NX_RIGHT_SHIFT: u64 = 0x0000_0004;
const NX_LEFT_CMD: u64 = 0x0000_0008;
const NX_RIGHT_CMD: u64 = 0x0000_0010;
const NX_LEFT_ALT: u64 = 0x0000_0020;
const NX_RIGHT_ALT: u64 = 0x0000_0040;
const NX_RIGHT_CTRL: u64 = 0x0000_2000;

/// Maps a macOS mouse button number to a [`MouseButton`].
///
/// Returns `None` for buttons beyond the five the OS reports natively; those come
/// from vendor drivers and have no cross-platform meaning.
#[must_use]
pub const fn button_from_number(number: i64) -> Option<MouseButton> {
    match number {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Right),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Back),
        4 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Maps a [`MouseButton`] back to a macOS button number, for injection.
#[must_use]
pub const fn button_to_number(button: MouseButton) -> i64 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    }
}

/// Extracts the modifier set from a `CGEventFlags` value.
///
/// Uses the device-dependent bits so left and right stay distinct.
#[must_use]
pub const fn modifiers_from_flags(flags: u64) -> Modifiers {
    let mut bits = 0u8;
    if flags & NX_LEFT_CTRL != 0 {
        bits |= Modifiers::LEFT_CTRL.bits();
    }
    if flags & NX_LEFT_SHIFT != 0 {
        bits |= Modifiers::LEFT_SHIFT.bits();
    }
    if flags & NX_LEFT_ALT != 0 {
        bits |= Modifiers::LEFT_ALT.bits();
    }
    if flags & NX_LEFT_CMD != 0 {
        bits |= Modifiers::LEFT_META.bits();
    }
    if flags & NX_RIGHT_CTRL != 0 {
        bits |= Modifiers::RIGHT_CTRL.bits();
    }
    if flags & NX_RIGHT_SHIFT != 0 {
        bits |= Modifiers::RIGHT_SHIFT.bits();
    }
    if flags & NX_RIGHT_ALT != 0 {
        bits |= Modifiers::RIGHT_ALT.bits();
    }
    if flags & NX_RIGHT_CMD != 0 {
        bits |= Modifiers::RIGHT_META.bits();
    }
    Modifiers::from_bits(bits)
}

/// Derives key press and release events from a modifier state change.
///
/// macOS does not emit `keyDown`/`keyUp` for modifiers. It emits `flagsChanged`
/// carrying the *new* state, leaving the application to work out what moved.
/// Diffing the whole set rather than trusting the accompanying key code handles
/// the awkward cases correctly: several modifiers changing at once, and the
/// resynchronisation after sleep or a lost event, where more than one bit differs.
///
/// Releases are emitted before presses so that a swap — releasing Left Shift
/// while pressing Right Shift in the same event — never momentarily reports both.
///
/// Returns a fixed-size value rather than a `Vec`: this runs inside the event tap
/// callback, where an allocation is latency the user can feel and, worse, can
/// make the callback slow enough for macOS to disable the tap outright.
#[must_use]
pub fn modifier_transitions(previous: Modifiers, current: Modifiers) -> ModifierTransitions {
    let released = previous.bits() & !current.bits();
    let pressed = current.bits() & !previous.bits();

    let mut out = ModifierTransitions::default();
    for key in Modifiers::from_bits(released).held_keys() {
        out.push(key, false);
    }
    for key in Modifiers::from_bits(pressed).held_keys() {
        out.push(key, true);
    }
    out
}

/// The key transitions implied by one `flagsChanged` event.
///
/// At most eight, because there are eight modifier bits and each can only move
/// in one direction per event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierTransitions {
    items: [(KeyCode, bool); 8],
    len: usize,
}

impl Default for ModifierTransitions {
    fn default() -> Self {
        Self {
            items: [(KeyCode::new(0, 0), false); 8],
            len: 0,
        }
    }
}

impl ModifierTransitions {
    fn push(&mut self, key: KeyCode, pressed: bool) {
        if self.len < self.items.len() {
            self.items[self.len] = (key, pressed);
            self.len += 1;
        }
    }

    /// The transitions, releases first.
    #[must_use]
    pub fn as_slice(&self) -> &[(KeyCode, bool)] {
        &self.items[..self.len]
    }

    /// Number of transitions.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when nothing changed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Builds a scroll delta from CoreGraphics scroll fields.
///
/// A continuous scroll — a trackpad, or a free-spinning wheel — reports pixel
/// deltas and its line deltas are coarsely rounded, often to zero for small
/// movements. Using the pixel value and converting is what keeps slow trackpad
/// scrolling from stalling entirely.
#[must_use]
pub fn scroll_from_fields(
    is_continuous: bool,
    line_dy: f64,
    line_dx: f64,
    point_dy: f64,
    point_dx: f64,
) -> ScrollDelta {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "scroll deltas are small; f32 is ample and halves the wire size"
    )]
    if is_continuous {
        ScrollDelta {
            x: (point_dx / f64::from(PIXELS_PER_LINE)) as f32,
            y: (point_dy / f64::from(PIXELS_PER_LINE)) as f32,
            precise: true,
        }
    } else {
        ScrollDelta {
            x: line_dx as f32,
            y: line_dy as f32,
            precise: false,
        }
    }
}

/// Converts a line count back to whole wheel notches for injection.
///
/// Returns `None` when the movement rounds to nothing, so the caller can skip the
/// event rather than injecting a zero scroll that some applications treat as the
/// end of a gesture.
#[must_use]
pub fn lines_to_notches(lines: f32) -> Option<i32> {
    if !lines.is_finite() {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "line counts are far below i32 range"
    )]
    let notches = lines.round() as i32;
    (notches != 0).then_some(notches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_input::keycode::keys;

    #[test]
    fn mouse_buttons_round_trip() {
        for button in MouseButton::ALL {
            let n = button_to_number(button);
            assert_eq!(button_from_number(n), Some(button));
        }
    }

    #[test]
    fn unknown_button_numbers_are_dropped() {
        // Vendor-driver buttons have no cross-platform meaning; inventing one
        // would press an unrelated button on the other machine.
        assert_eq!(button_from_number(5), None);
        assert_eq!(button_from_number(-1), None);
        assert_eq!(button_from_number(99), None);
    }

    #[test]
    fn device_flags_distinguish_left_from_right() {
        assert_eq!(modifiers_from_flags(NX_LEFT_SHIFT), Modifiers::LEFT_SHIFT);
        assert_eq!(modifiers_from_flags(NX_RIGHT_SHIFT), Modifiers::RIGHT_SHIFT);
        assert_ne!(
            modifiers_from_flags(NX_LEFT_CTRL),
            modifiers_from_flags(NX_RIGHT_CTRL)
        );
    }

    #[test]
    fn all_eight_device_flags_are_recognised() {
        let all = NX_LEFT_CTRL
            | NX_LEFT_SHIFT
            | NX_RIGHT_SHIFT
            | NX_LEFT_CMD
            | NX_RIGHT_CMD
            | NX_LEFT_ALT
            | NX_RIGHT_ALT
            | NX_RIGHT_CTRL;
        assert_eq!(modifiers_from_flags(all).bits(), 0xFF);
    }

    #[test]
    fn unrelated_flag_bits_are_ignored() {
        // CGEventFlags also carries Caps Lock, the fn key, and the numeric-pad
        // bit. None is a held modifier, and none must leak into the set.
        let noise = 0x0001_0000 | 0x0002_0000 | 0x0080_0000;
        assert_eq!(modifiers_from_flags(noise), Modifiers::NONE);
    }

    #[test]
    fn a_single_modifier_press_and_release_is_derived_correctly() {
        let press = modifier_transitions(Modifiers::NONE, Modifiers::LEFT_SHIFT);
        assert_eq!(press.as_slice(), &[(keys::LEFT_SHIFT, true)]);

        let release = modifier_transitions(Modifiers::LEFT_SHIFT, Modifiers::NONE);
        assert_eq!(release.as_slice(), &[(keys::LEFT_SHIFT, false)]);
    }

    #[test]
    fn an_unchanged_state_produces_nothing() {
        assert!(modifier_transitions(Modifiers::LEFT_CTRL, Modifiers::LEFT_CTRL).is_empty());
        assert!(modifier_transitions(Modifiers::NONE, Modifiers::NONE).is_empty());
    }

    #[test]
    fn releases_are_emitted_before_presses() {
        // Swapping hands between the two Shift keys arrives as one flagsChanged.
        // Emitting the press first would momentarily report both as held.
        let events = modifier_transitions(Modifiers::LEFT_SHIFT, Modifiers::RIGHT_SHIFT);
        assert_eq!(
            events.as_slice(),
            &[(keys::LEFT_SHIFT, false), (keys::RIGHT_SHIFT, true)]
        );
    }

    #[test]
    fn several_modifiers_changing_at_once_are_all_reported() {
        // Happens after sleep or a dropped event, when the diff spans many bits.
        // Trusting the event's own key code instead would report only one.
        let before = Modifiers::LEFT_CTRL.union(Modifiers::LEFT_ALT);
        let after = Modifiers::LEFT_SHIFT.union(Modifiers::LEFT_META);
        let events = modifier_transitions(before, after);
        assert_eq!(events.len(), 4);
        assert_eq!(events.as_slice().iter().filter(|(_, d)| !d).count(), 2);
        assert_eq!(events.as_slice().iter().filter(|(_, d)| *d).count(), 2);
    }

    #[test]
    fn transitions_applied_to_a_tracker_reproduce_the_target_state() {
        // The property that matters: whatever the diff, replaying it must land
        // the tracker exactly on the new state.
        let mut tracker = mb_input::InputStateTracker::new();
        let mut current = Modifiers::NONE;
        for target in [
            Modifiers::LEFT_SHIFT,
            Modifiers::LEFT_SHIFT.union(Modifiers::LEFT_META),
            Modifiers::RIGHT_ALT,
            Modifiers::NONE,
        ] {
            for (key, pressed) in modifier_transitions(current, target).as_slice() {
                tracker.apply(&mb_input::InputEvent::Key {
                    key: *key,
                    pressed: *pressed,
                    repeat: false,
                });
            }
            assert_eq!(tracker.modifiers(), target);
            current = target;
        }
    }

    #[test]
    fn a_notched_wheel_uses_line_deltas() {
        let d = scroll_from_fields(false, 3.0, 0.0, 0.0, 0.0);
        assert_eq!(d.y, 3.0);
        assert!(!d.precise);
    }

    #[test]
    fn a_trackpad_uses_pixel_deltas_so_slow_scrolling_does_not_stall() {
        // The realistic failure: a small trackpad movement rounds its line delta
        // to zero, and using that value would drop the scroll entirely.
        let d = scroll_from_fields(true, 0.0, 0.0, 5.0, 0.0);
        assert!(d.precise);
        assert!((d.y - 0.5).abs() < 1e-6, "expected 0.5 lines, got {}", d.y);
        assert!(!d.is_zero(), "a slow trackpad scroll was dropped");
    }

    #[test]
    fn scroll_signs_are_preserved_on_both_axes() {
        let d = scroll_from_fields(false, -2.0, 4.0, 0.0, 0.0);
        assert_eq!((d.x, d.y), (4.0, -2.0));
    }

    #[test]
    fn notch_conversion_drops_movements_that_round_to_nothing() {
        // Injecting a zero scroll is not harmless: some applications read it as
        // the end of a gesture.
        assert_eq!(lines_to_notches(0.0), None);
        assert_eq!(lines_to_notches(0.2), None);
        assert_eq!(lines_to_notches(f32::NAN), None);
        assert_eq!(lines_to_notches(1.0), Some(1));
        assert_eq!(lines_to_notches(-2.6), Some(-3));
    }
}
