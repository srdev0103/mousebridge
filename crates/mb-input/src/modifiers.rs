//! Modifier and mouse-button bit sets.
//!
//! Hand-rolled rather than pulling in `bitflags`: two small newtypes over `u8`
//! are less code than the dependency's documentation, and [`Modifiers`] has to
//! match the HID modifier byte layout exactly anyway, which a generic macro
//! would not enforce.

use crate::keycode::{KeyCode, keys};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The eight HID keyboard modifiers, laid out exactly as the HID modifier byte.
///
/// Bit order is Left Control, Left Shift, Left Alt, Left Meta, then the same four
/// on the right. This is not an internal convention — it is the wire layout of a
/// USB keyboard report, which is why a modifier's bit can be derived from its
/// usage ID by subtraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Self = Self(0);

    /// Left Control.
    pub const LEFT_CTRL: Self = Self(0x01);
    /// Left Shift.
    pub const LEFT_SHIFT: Self = Self(0x02);
    /// Left Alt / Option.
    pub const LEFT_ALT: Self = Self(0x04);
    /// Left Meta: Command on macOS, Windows key on Windows.
    pub const LEFT_META: Self = Self(0x08);
    /// Right Control.
    pub const RIGHT_CTRL: Self = Self(0x10);
    /// Right Shift.
    pub const RIGHT_SHIFT: Self = Self(0x20);
    /// Right Alt / AltGr.
    pub const RIGHT_ALT: Self = Self(0x40);
    /// Right Meta.
    pub const RIGHT_META: Self = Self(0x80);

    /// Builds from a raw HID modifier byte.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the raw HID modifier byte.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns true if no modifier is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns true if every modifier in `other` is held.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns this set with `other` removed.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Adds the bit for a modifier key. Non-modifier keys are ignored.
    #[must_use]
    pub const fn with_key(self, key: KeyCode) -> Self {
        match key.modifier_bit() {
            Some(bit) => Self(self.0 | bit),
            None => self,
        }
    }

    /// Removes the bit for a modifier key. Non-modifier keys are ignored.
    #[must_use]
    pub const fn without_key(self, key: KeyCode) -> Self {
        match key.modifier_bit() {
            Some(bit) => Self(self.0 & !bit),
            None => self,
        }
    }

    /// True if either Control is held.
    #[must_use]
    pub const fn ctrl(self) -> bool {
        self.0 & (Self::LEFT_CTRL.0 | Self::RIGHT_CTRL.0) != 0
    }

    /// True if either Shift is held.
    #[must_use]
    pub const fn shift(self) -> bool {
        self.0 & (Self::LEFT_SHIFT.0 | Self::RIGHT_SHIFT.0) != 0
    }

    /// True if either Alt is held.
    #[must_use]
    pub const fn alt(self) -> bool {
        self.0 & (Self::LEFT_ALT.0 | Self::RIGHT_ALT.0) != 0
    }

    /// True if either Meta — Command or Windows key — is held.
    #[must_use]
    pub const fn meta(self) -> bool {
        self.0 & (Self::LEFT_META.0 | Self::RIGHT_META.0) != 0
    }

    /// Iterates the modifier keys currently held, in HID bit order.
    pub fn held_keys(self) -> impl Iterator<Item = KeyCode> {
        keys::MODIFIERS
            .into_iter()
            .filter(move |k| k.modifier_bit().is_some_and(|bit| self.0 & bit != 0))
    }
}

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut first = true;
        for key in self.held_keys() {
            if !first {
                f.write_str("+")?;
            }
            f.write_str(key.name())?;
            first = false;
        }
        Ok(())
    }
}

/// Mouse buttons.
///
/// Five buttons cover every mouse the OS reports natively. Buttons beyond these
/// are vendor-driver territory and do not reach a system-wide hook or event tap
/// as ordinary button events, so pretending to support them would be dishonest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Wheel click.
    Middle,
    /// Thumb button 1, conventionally "back".
    Back,
    /// Thumb button 2, conventionally "forward".
    Forward,
}

impl MouseButton {
    /// Every button, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::Back,
        Self::Forward,
    ];

    /// Returns this button's bit within a [`MouseButtons`] set.
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::Left => 0x01,
            Self::Right => 0x02,
            Self::Middle => 0x04,
            Self::Back => 0x08,
            Self::Forward => 0x10,
        }
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Middle => "Middle",
            Self::Back => "Back",
            Self::Forward => "Forward",
        })
    }
}

/// A set of held mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MouseButtons(u8);

impl MouseButtons {
    /// No buttons held.
    pub const NONE: Self = Self(0);

    /// Bits that correspond to a real button.
    ///
    /// Only five of the eight bits are defined. The other three must never be
    /// storable: [`MouseButtons::held`] can only iterate buttons it knows about,
    /// so an undefined bit would make [`MouseButtons::is_empty`] report "still
    /// holding something" that no release sequence could ever clear — a button
    /// stuck down forever, on a machine the user has walked away from.
    const VALID: u8 = MouseButton::Left.bit()
        | MouseButton::Right.bit()
        | MouseButton::Middle.bit()
        | MouseButton::Back.bit()
        | MouseButton::Forward.bit();

    /// Builds from a raw bit set, discarding bits that name no button.
    ///
    /// Masking rather than rejecting: this value arrives from the network, and
    /// dropping an unknown bit is both forward-compatible and safe, whereas
    /// storing it is not. Found by the `release_events_always_reach_a_clean_state`
    /// property test.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::VALID)
    }

    /// Returns the raw bit set.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns true if no button is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns true if the button is held.
    #[must_use]
    pub const fn contains(self, button: MouseButton) -> bool {
        self.0 & button.bit() != 0
    }

    /// Returns this set with the button added.
    #[must_use]
    pub const fn with(self, button: MouseButton) -> Self {
        Self(self.0 | button.bit())
    }

    /// Returns this set with the button removed.
    #[must_use]
    pub const fn without(self, button: MouseButton) -> Self {
        Self(self.0 & !button.bit())
    }

    /// Iterates the held buttons in a stable order.
    pub fn held(self) -> impl Iterator<Item = MouseButton> {
        MouseButton::ALL
            .into_iter()
            .filter(move |b| self.contains(*b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::keys;

    #[test]
    fn modifier_bits_match_the_hid_report_byte() {
        // Not an internal convention: this is the USB HID keyboard report layout.
        assert_eq!(Modifiers::LEFT_CTRL.bits(), 0b0000_0001);
        assert_eq!(Modifiers::LEFT_SHIFT.bits(), 0b0000_0010);
        assert_eq!(Modifiers::LEFT_ALT.bits(), 0b0000_0100);
        assert_eq!(Modifiers::LEFT_META.bits(), 0b0000_1000);
        assert_eq!(Modifiers::RIGHT_META.bits(), 0b1000_0000);
    }

    #[test]
    fn keys_and_bits_agree() {
        // The two representations must never drift: `with_key` derives the bit
        // from the usage ID, while the constants above are written by hand.
        assert_eq!(
            Modifiers::NONE.with_key(keys::LEFT_SHIFT),
            Modifiers::LEFT_SHIFT
        );
        assert_eq!(
            Modifiers::NONE.with_key(keys::RIGHT_META),
            Modifiers::RIGHT_META
        );
    }

    #[test]
    fn non_modifier_keys_do_not_change_the_set() {
        let m = Modifiers::LEFT_CTRL;
        assert_eq!(m.with_key(keys::A), m);
        assert_eq!(m.without_key(keys::SPACE), m);
        assert_eq!(
            m.with_key(keys::CAPS_LOCK),
            m,
            "Caps Lock is not a modifier"
        );
    }

    #[test]
    fn left_and_right_are_distinct_but_either_satisfies_the_query() {
        let left = Modifiers::LEFT_SHIFT;
        let right = Modifiers::RIGHT_SHIFT;
        assert_ne!(left, right);
        assert!(left.shift() && right.shift());
        assert!(!left.contains(right), "sides must not be conflated");
    }

    #[test]
    fn held_keys_round_trips_through_the_bit_set() {
        let m = Modifiers::LEFT_CTRL
            .union(Modifiers::RIGHT_ALT)
            .union(Modifiers::LEFT_META);
        let rebuilt = m.held_keys().fold(Modifiers::NONE, Modifiers::with_key);
        assert_eq!(rebuilt, m);
        assert_eq!(m.held_keys().count(), 3);
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(Modifiers::NONE.to_string(), "none");
        assert_eq!(
            Modifiers::LEFT_META
                .union(Modifiers::LEFT_SHIFT)
                .to_string(),
            "LeftShift+LeftMeta"
        );
    }

    #[test]
    fn mouse_button_bits_are_distinct() {
        let mut seen = 0u8;
        for b in MouseButton::ALL {
            assert_eq!(seen & b.bit(), 0, "{b} reuses a bit");
            seen |= b.bit();
        }
    }

    #[test]
    fn undefined_button_bits_are_discarded_not_stored() {
        // Regression: an undefined bit made `is_empty` false forever while
        // `held` yielded nothing, so no release sequence could clear it.
        let hostile = MouseButtons::from_bits(0b1110_0000);
        assert!(hostile.is_empty(), "an unclearable button bit was stored");
        assert_eq!(hostile.held().count(), 0);

        let mixed = MouseButtons::from_bits(0b1110_0001);
        assert_eq!(mixed.held().count(), 1);
        assert!(mixed.contains(MouseButton::Left));
        assert_eq!(
            mixed.bits().count_ones(),
            mixed.held().count() as u32,
            "every stored bit must be reachable through held()"
        );
    }

    #[test]
    fn button_set_add_and_remove() {
        let set = MouseButtons::NONE
            .with(MouseButton::Left)
            .with(MouseButton::Middle);
        assert!(set.contains(MouseButton::Left));
        assert!(!set.contains(MouseButton::Right));
        assert_eq!(set.held().count(), 2);

        let after = set.without(MouseButton::Left);
        assert!(!after.contains(MouseButton::Left));
        assert!(after.contains(MouseButton::Middle));

        // Removing a button that is not held must be a no-op, not an underflow.
        assert_eq!(after.without(MouseButton::Left), after);
    }
}
