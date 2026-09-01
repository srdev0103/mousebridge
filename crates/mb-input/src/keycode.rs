//! Physical key identification.
//!
//! # Why HID usage codes
//!
//! A key can be named three ways: by the platform's virtual key code, by the
//! character it produces, or by its physical position. Only the third survives a
//! trip between machines.
//!
//! Virtual key codes are platform-specific — macOS `kVK_ANSI_A` is `0x00`,
//! Windows `VK_A` is `0x41` — so they cannot be exchanged without a translation
//! table on both ends. Characters are worse: they depend on the *sender's*
//! keyboard layout, so a German user pressing the key labelled `Z` on a QWERTZ
//! keyboard would type `Y` on a US-layout machine, and every dead key and AltGr
//! combination would break.
//!
//! USB HID usage IDs name the physical key and nothing else. The receiving
//! machine maps the position to whatever its own layout produces, which is
//! exactly what happens when you plug a keyboard into that machine directly.
//! Layout, dead keys and IME composition are then handled by the receiver's own
//! input stack, which is both correct and free.

use serde::{Deserialize, Serialize};
use std::fmt;

/// HID usage page for the Keyboard/Keypad page.
pub const PAGE_KEYBOARD: u16 = 0x07;

/// HID usage page for the Consumer page, used for media and volume keys.
pub const PAGE_CONSUMER: u16 = 0x0C;

/// A physical key, identified by HID usage page and usage ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeyCode {
    /// HID usage page. [`PAGE_KEYBOARD`] or [`PAGE_CONSUMER`] in practice.
    pub page: u16,
    /// HID usage ID within the page.
    pub usage: u16,
}

impl KeyCode {
    /// Builds a key code from an explicit page and usage.
    #[must_use]
    pub const fn new(page: u16, usage: u16) -> Self {
        Self { page, usage }
    }

    /// Builds a key on the keyboard page.
    #[must_use]
    pub const fn keyboard(usage: u16) -> Self {
        Self::new(PAGE_KEYBOARD, usage)
    }

    /// Builds a key on the consumer page.
    #[must_use]
    pub const fn consumer(usage: u16) -> Self {
        Self::new(PAGE_CONSUMER, usage)
    }

    /// Returns true if this key is one of the eight HID modifiers.
    #[must_use]
    pub const fn is_modifier(self) -> bool {
        self.page == PAGE_KEYBOARD && self.usage >= 0xE0 && self.usage <= 0xE7
    }

    /// Returns the modifier bit this key corresponds to, if it is a modifier.
    ///
    /// The HID specification assigns modifiers usages `0xE0`-`0xE7` in the same
    /// order as the bits of the modifier byte, so the mapping is a subtraction
    /// rather than a table.
    #[must_use]
    pub const fn modifier_bit(self) -> Option<u8> {
        if self.is_modifier() {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "usage is between 0xE0 and 0xE7, so the shift is 0..=7"
            )]
            Some(1u8 << (self.usage - 0xE0) as u8)
        } else {
            None
        }
    }

    /// Returns a stable name for diagnostics and tests.
    ///
    /// **Not for logging captured input.** Rendering which keys a user pressed is
    /// keylogging regardless of the log level; this crate has no logger for that
    /// reason. This exists for test failure messages and the settings UI.
    #[must_use]
    pub fn name(self) -> &'static str {
        match (self.page, self.usage) {
            (PAGE_KEYBOARD, 0x04..=0x1D) => LETTERS[(self.usage - 0x04) as usize],
            (PAGE_KEYBOARD, 0x1E..=0x27) => DIGITS[(self.usage - 0x1E) as usize],
            (PAGE_KEYBOARD, 0x3A..=0x45) => FUNCTION[(self.usage - 0x3A) as usize],
            (PAGE_KEYBOARD, 0x28) => "Enter",
            (PAGE_KEYBOARD, 0x29) => "Escape",
            (PAGE_KEYBOARD, 0x2A) => "Backspace",
            (PAGE_KEYBOARD, 0x2B) => "Tab",
            (PAGE_KEYBOARD, 0x2C) => "Space",
            (PAGE_KEYBOARD, 0x2D) => "Minus",
            (PAGE_KEYBOARD, 0x2E) => "Equal",
            (PAGE_KEYBOARD, 0x2F) => "BracketLeft",
            (PAGE_KEYBOARD, 0x30) => "BracketRight",
            (PAGE_KEYBOARD, 0x31) => "Backslash",
            (PAGE_KEYBOARD, 0x33) => "Semicolon",
            (PAGE_KEYBOARD, 0x34) => "Quote",
            (PAGE_KEYBOARD, 0x35) => "Backquote",
            (PAGE_KEYBOARD, 0x36) => "Comma",
            (PAGE_KEYBOARD, 0x37) => "Period",
            (PAGE_KEYBOARD, 0x38) => "Slash",
            (PAGE_KEYBOARD, 0x39) => "CapsLock",
            (PAGE_KEYBOARD, 0x46) => "PrintScreen",
            (PAGE_KEYBOARD, 0x47) => "ScrollLock",
            (PAGE_KEYBOARD, 0x48) => "Pause",
            (PAGE_KEYBOARD, 0x49) => "Insert",
            (PAGE_KEYBOARD, 0x4A) => "Home",
            (PAGE_KEYBOARD, 0x4B) => "PageUp",
            (PAGE_KEYBOARD, 0x4C) => "Delete",
            (PAGE_KEYBOARD, 0x4D) => "End",
            (PAGE_KEYBOARD, 0x4E) => "PageDown",
            (PAGE_KEYBOARD, 0x4F) => "ArrowRight",
            (PAGE_KEYBOARD, 0x50) => "ArrowLeft",
            (PAGE_KEYBOARD, 0x51) => "ArrowDown",
            (PAGE_KEYBOARD, 0x52) => "ArrowUp",
            (PAGE_KEYBOARD, 0xE0) => "LeftControl",
            (PAGE_KEYBOARD, 0xE1) => "LeftShift",
            (PAGE_KEYBOARD, 0xE2) => "LeftAlt",
            (PAGE_KEYBOARD, 0xE3) => "LeftMeta",
            (PAGE_KEYBOARD, 0xE4) => "RightControl",
            (PAGE_KEYBOARD, 0xE5) => "RightShift",
            (PAGE_KEYBOARD, 0xE6) => "RightAlt",
            (PAGE_KEYBOARD, 0xE7) => "RightMeta",
            (PAGE_CONSUMER, 0xB5) => "MediaNext",
            (PAGE_CONSUMER, 0xB6) => "MediaPrevious",
            (PAGE_CONSUMER, 0xB7) => "MediaStop",
            (PAGE_CONSUMER, 0xCD) => "MediaPlayPause",
            (PAGE_CONSUMER, 0xE2) => "Mute",
            (PAGE_CONSUMER, 0xE9) => "VolumeUp",
            (PAGE_CONSUMER, 0xEA) => "VolumeDown",
            _ => "Unknown",
        }
    }
}

const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const DIGITS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
const FUNCTION: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            "Unknown" => write!(f, "HID({:#04x}:{:#06x})", self.page, self.usage),
            name => f.write_str(name),
        }
    }
}

/// Named constants for keys the rest of the system refers to directly.
///
/// Not exhaustive by design: an arbitrary key is a [`KeyCode::keyboard`] call,
/// and enumerating all 200-odd usages as constants would be noise. These are the
/// ones with behavioural meaning — modifiers, and the keys used by tests.
pub mod keys {
    use super::KeyCode;

    /// `A`.
    pub const A: KeyCode = KeyCode::keyboard(0x04);
    /// `C`, used by the clipboard shortcut tests.
    pub const C: KeyCode = KeyCode::keyboard(0x06);
    /// `Q`.
    pub const Q: KeyCode = KeyCode::keyboard(0x14);
    /// `V`.
    pub const V: KeyCode = KeyCode::keyboard(0x19);
    /// `Z`.
    pub const Z: KeyCode = KeyCode::keyboard(0x1D);
    /// Return / Enter.
    pub const ENTER: KeyCode = KeyCode::keyboard(0x28);
    /// Escape.
    pub const ESCAPE: KeyCode = KeyCode::keyboard(0x29);
    /// Tab.
    pub const TAB: KeyCode = KeyCode::keyboard(0x2B);
    /// Space.
    pub const SPACE: KeyCode = KeyCode::keyboard(0x2C);
    /// Caps Lock.
    pub const CAPS_LOCK: KeyCode = KeyCode::keyboard(0x39);
    /// F1.
    pub const F1: KeyCode = KeyCode::keyboard(0x3A);

    /// Left Control.
    pub const LEFT_CTRL: KeyCode = KeyCode::keyboard(0xE0);
    /// Left Shift.
    pub const LEFT_SHIFT: KeyCode = KeyCode::keyboard(0xE1);
    /// Left Alt / Option.
    pub const LEFT_ALT: KeyCode = KeyCode::keyboard(0xE2);
    /// Left Meta: Command on macOS, Windows key on Windows.
    pub const LEFT_META: KeyCode = KeyCode::keyboard(0xE3);
    /// Right Control.
    pub const RIGHT_CTRL: KeyCode = KeyCode::keyboard(0xE4);
    /// Right Shift.
    pub const RIGHT_SHIFT: KeyCode = KeyCode::keyboard(0xE5);
    /// Right Alt / AltGr.
    pub const RIGHT_ALT: KeyCode = KeyCode::keyboard(0xE6);
    /// Right Meta.
    pub const RIGHT_META: KeyCode = KeyCode::keyboard(0xE7);

    /// Every modifier, in HID bit order.
    pub const MODIFIERS: [KeyCode; 8] = [
        LEFT_CTRL,
        LEFT_SHIFT,
        LEFT_ALT,
        LEFT_META,
        RIGHT_CTRL,
        RIGHT_SHIFT,
        RIGHT_ALT,
        RIGHT_META,
    ];

    /// Play/pause.
    pub const MEDIA_PLAY_PAUSE: KeyCode = KeyCode::consumer(0xCD);
    /// Volume up.
    pub const VOLUME_UP: KeyCode = KeyCode::consumer(0xE9);
    /// Mute.
    pub const MUTE: KeyCode = KeyCode::consumer(0xE2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use keys::*;

    #[test]
    fn modifiers_map_to_distinct_consecutive_bits() {
        // The HID spec guarantees this ordering; the subtraction in
        // `modifier_bit` depends on it, so assert it rather than trusting it.
        let bits: Vec<u8> = MODIFIERS
            .iter()
            .map(|k| k.modifier_bit().expect("is a modifier"))
            .collect();
        assert_eq!(bits, vec![0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn non_modifiers_have_no_modifier_bit() {
        for key in [A, ENTER, SPACE, F1, CAPS_LOCK, MEDIA_PLAY_PAUSE] {
            assert!(!key.is_modifier(), "{key} claimed to be a modifier");
            assert_eq!(key.modifier_bit(), None);
        }
    }

    #[test]
    fn caps_lock_is_not_a_modifier() {
        // Deliberate: Caps Lock is a locking key, not a held modifier. Treating
        // it as one would make the state tracker try to "release" it, which is
        // meaningless and would desynchronise the two machines.
        assert!(!CAPS_LOCK.is_modifier());
    }

    #[test]
    fn consumer_page_keys_are_never_modifiers() {
        // 0xE2 is Left Alt on the keyboard page and Mute on the consumer page.
        // Comparing usage without the page would confuse the two.
        const { assert!(MUTE.usage == LEFT_ALT.usage, "the test premise still holds") };
        assert!(!MUTE.is_modifier());
        assert!(LEFT_ALT.is_modifier());
        assert_ne!(MUTE, LEFT_ALT);
    }

    #[test]
    fn names_are_assigned_correctly_at_range_boundaries() {
        assert_eq!(A.name(), "A");
        assert_eq!(KeyCode::keyboard(0x1D).name(), "Z");
        assert_eq!(KeyCode::keyboard(0x1E).name(), "1");
        assert_eq!(KeyCode::keyboard(0x27).name(), "0");
        assert_eq!(F1.name(), "F1");
        assert_eq!(KeyCode::keyboard(0x45).name(), "F12");
    }

    #[test]
    fn unknown_usages_render_diagnosably() {
        let odd = KeyCode::keyboard(0xFFF);
        assert_eq!(odd.name(), "Unknown");
        assert_eq!(odd.to_string(), "HID(0x07:0x0fff)");
    }

    #[test]
    fn key_codes_round_trip_through_serde() {
        for key in [A, LEFT_META, MEDIA_PLAY_PAUSE, KeyCode::keyboard(0x99)] {
            let json = serde_json::to_string(&key).expect("serialises");
            let back: KeyCode = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(key, back);
        }
    }
}
