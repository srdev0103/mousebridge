//! Translation between macOS virtual key codes and HID usage IDs.
//!
//! macOS identifies keys by `kVK_*` virtual key codes, which name **positions on
//! an ANSI keyboard**, not characters. Despite the "ANSI" naming they are
//! layout-independent: `kVK_ANSI_Z` is the key in the bottom-left of the main
//! block whatever it is labelled, so a German QWERTZ keyboard reports
//! `kVK_ANSI_Y` for the key printed `Z`.
//!
//! That makes the mapping to HID usage IDs a static table rather than a
//! layout-dependent lookup — which is the whole reason the wire format carries
//! HID usages. See [`crate::keycode`].
//!
//! The table follows the mapping used by Chromium and WebKit, which is the de
//! facto standard for this conversion.

use mb_input::keycode::{KeyCode, PAGE_CONSUMER, PAGE_KEYBOARD};

/// `(macOS virtual key code, HID usage page, HID usage id)`.
///
/// Ordered by virtual key code for readability; lookup is a linear scan over 120
/// entries, which is faster than a hash for this size and needs no allocation.
#[rustfmt::skip]
const TABLE: &[(u16, u16, u16)] = &[
    // Letters, in macOS's idiosyncratic physical order.
    (0x00, PAGE_KEYBOARD, 0x04), // A
    (0x01, PAGE_KEYBOARD, 0x16), // S
    (0x02, PAGE_KEYBOARD, 0x07), // D
    (0x03, PAGE_KEYBOARD, 0x09), // F
    (0x04, PAGE_KEYBOARD, 0x0B), // H
    (0x05, PAGE_KEYBOARD, 0x0A), // G
    (0x06, PAGE_KEYBOARD, 0x1D), // Z
    (0x07, PAGE_KEYBOARD, 0x1B), // X
    (0x08, PAGE_KEYBOARD, 0x06), // C
    (0x09, PAGE_KEYBOARD, 0x19), // V
    (0x0A, PAGE_KEYBOARD, 0x64), // ISO section / non-US backslash
    (0x0B, PAGE_KEYBOARD, 0x05), // B
    (0x0C, PAGE_KEYBOARD, 0x14), // Q
    (0x0D, PAGE_KEYBOARD, 0x1A), // W
    (0x0E, PAGE_KEYBOARD, 0x08), // E
    (0x0F, PAGE_KEYBOARD, 0x15), // R
    (0x10, PAGE_KEYBOARD, 0x1C), // Y
    (0x11, PAGE_KEYBOARD, 0x17), // T
    (0x12, PAGE_KEYBOARD, 0x1E), // 1
    (0x13, PAGE_KEYBOARD, 0x1F), // 2
    (0x14, PAGE_KEYBOARD, 0x20), // 3
    (0x15, PAGE_KEYBOARD, 0x21), // 4
    (0x16, PAGE_KEYBOARD, 0x23), // 6
    (0x17, PAGE_KEYBOARD, 0x22), // 5
    (0x18, PAGE_KEYBOARD, 0x2E), // =
    (0x19, PAGE_KEYBOARD, 0x26), // 9
    (0x1A, PAGE_KEYBOARD, 0x24), // 7
    (0x1B, PAGE_KEYBOARD, 0x2D), // -
    (0x1C, PAGE_KEYBOARD, 0x25), // 8
    (0x1D, PAGE_KEYBOARD, 0x27), // 0
    (0x1E, PAGE_KEYBOARD, 0x30), // ]
    (0x1F, PAGE_KEYBOARD, 0x12), // O
    (0x20, PAGE_KEYBOARD, 0x18), // U
    (0x21, PAGE_KEYBOARD, 0x2F), // [
    (0x22, PAGE_KEYBOARD, 0x0C), // I
    (0x23, PAGE_KEYBOARD, 0x13), // P
    (0x24, PAGE_KEYBOARD, 0x28), // Return
    (0x25, PAGE_KEYBOARD, 0x0F), // L
    (0x26, PAGE_KEYBOARD, 0x0D), // J
    (0x27, PAGE_KEYBOARD, 0x34), // '
    (0x28, PAGE_KEYBOARD, 0x0E), // K
    (0x29, PAGE_KEYBOARD, 0x33), // ;
    (0x2A, PAGE_KEYBOARD, 0x31), // backslash
    (0x2B, PAGE_KEYBOARD, 0x36), // ,
    (0x2C, PAGE_KEYBOARD, 0x38), // /
    (0x2D, PAGE_KEYBOARD, 0x11), // N
    (0x2E, PAGE_KEYBOARD, 0x10), // M
    (0x2F, PAGE_KEYBOARD, 0x37), // .
    (0x30, PAGE_KEYBOARD, 0x2B), // Tab
    (0x31, PAGE_KEYBOARD, 0x2C), // Space
    (0x32, PAGE_KEYBOARD, 0x35), // `
    (0x33, PAGE_KEYBOARD, 0x2A), // Delete (Backspace)
    (0x35, PAGE_KEYBOARD, 0x29), // Escape

    // Modifiers. macOS reports these through flagsChanged, not keyDown/keyUp.
    (0x36, PAGE_KEYBOARD, 0xE7), // Right Command
    (0x37, PAGE_KEYBOARD, 0xE3), // Left Command
    (0x38, PAGE_KEYBOARD, 0xE1), // Left Shift
    (0x39, PAGE_KEYBOARD, 0x39), // Caps Lock (a key, not a modifier)
    (0x3A, PAGE_KEYBOARD, 0xE2), // Left Option
    (0x3B, PAGE_KEYBOARD, 0xE0), // Left Control
    (0x3C, PAGE_KEYBOARD, 0xE5), // Right Shift
    (0x3D, PAGE_KEYBOARD, 0xE6), // Right Option
    (0x3E, PAGE_KEYBOARD, 0xE4), // Right Control
    // 0x3F is kVK_Function, the fn key. It has no HID keyboard usage and is
    // deliberately absent: forwarding it would mean inventing an encoding, and
    // the receiving machine has nothing to do with it.

    (0x40, PAGE_KEYBOARD, 0x6C), // F17
    (0x41, PAGE_KEYBOARD, 0x63), // Keypad .
    (0x43, PAGE_KEYBOARD, 0x55), // Keypad *
    (0x45, PAGE_KEYBOARD, 0x57), // Keypad +
    (0x47, PAGE_KEYBOARD, 0x53), // Keypad Clear / Num Lock
    (0x48, PAGE_CONSUMER, 0xE9), // Volume Up
    (0x49, PAGE_CONSUMER, 0xEA), // Volume Down
    (0x4A, PAGE_CONSUMER, 0xE2), // Mute
    (0x4B, PAGE_KEYBOARD, 0x54), // Keypad /
    (0x4C, PAGE_KEYBOARD, 0x58), // Keypad Enter
    (0x4E, PAGE_KEYBOARD, 0x56), // Keypad -
    (0x4F, PAGE_KEYBOARD, 0x6D), // F18
    (0x50, PAGE_KEYBOARD, 0x6E), // F19
    (0x51, PAGE_KEYBOARD, 0x67), // Keypad =
    (0x52, PAGE_KEYBOARD, 0x62), // Keypad 0
    (0x53, PAGE_KEYBOARD, 0x59), // Keypad 1
    (0x54, PAGE_KEYBOARD, 0x5A), // Keypad 2
    (0x55, PAGE_KEYBOARD, 0x5B), // Keypad 3
    (0x56, PAGE_KEYBOARD, 0x5C), // Keypad 4
    (0x57, PAGE_KEYBOARD, 0x5D), // Keypad 5
    (0x58, PAGE_KEYBOARD, 0x5E), // Keypad 6
    (0x59, PAGE_KEYBOARD, 0x5F), // Keypad 7
    (0x5A, PAGE_KEYBOARD, 0x6F), // F20
    (0x5B, PAGE_KEYBOARD, 0x60), // Keypad 8
    (0x5C, PAGE_KEYBOARD, 0x61), // Keypad 9
    (0x5D, PAGE_KEYBOARD, 0x89), // JIS Yen
    (0x5E, PAGE_KEYBOARD, 0x87), // JIS underscore
    (0x5F, PAGE_KEYBOARD, 0x85), // JIS keypad comma
    (0x60, PAGE_KEYBOARD, 0x3E), // F5
    (0x61, PAGE_KEYBOARD, 0x3F), // F6
    (0x62, PAGE_KEYBOARD, 0x40), // F7
    (0x63, PAGE_KEYBOARD, 0x3C), // F3
    (0x64, PAGE_KEYBOARD, 0x41), // F8
    (0x65, PAGE_KEYBOARD, 0x42), // F9
    (0x66, PAGE_KEYBOARD, 0x91), // JIS Eisu (Lang2)
    (0x67, PAGE_KEYBOARD, 0x44), // F11
    (0x68, PAGE_KEYBOARD, 0x90), // JIS Kana (Lang1)
    (0x69, PAGE_KEYBOARD, 0x68), // F13
    (0x6A, PAGE_KEYBOARD, 0x6B), // F16
    (0x6B, PAGE_KEYBOARD, 0x69), // F14
    (0x6D, PAGE_KEYBOARD, 0x43), // F10
    (0x6E, PAGE_KEYBOARD, 0x65), // Application / context menu
    (0x6F, PAGE_KEYBOARD, 0x45), // F12
    (0x71, PAGE_KEYBOARD, 0x6A), // F15
    (0x72, PAGE_KEYBOARD, 0x49), // Help / Insert
    (0x73, PAGE_KEYBOARD, 0x4A), // Home
    (0x74, PAGE_KEYBOARD, 0x4B), // Page Up
    (0x75, PAGE_KEYBOARD, 0x4C), // Forward Delete
    (0x76, PAGE_KEYBOARD, 0x3D), // F4
    (0x77, PAGE_KEYBOARD, 0x4D), // End
    (0x78, PAGE_KEYBOARD, 0x3B), // F2
    (0x79, PAGE_KEYBOARD, 0x4E), // Page Down
    (0x7A, PAGE_KEYBOARD, 0x3A), // F1
    (0x7B, PAGE_KEYBOARD, 0x50), // Left Arrow
    (0x7C, PAGE_KEYBOARD, 0x4F), // Right Arrow
    (0x7D, PAGE_KEYBOARD, 0x51), // Down Arrow
    (0x7E, PAGE_KEYBOARD, 0x52), // Up Arrow
];

/// Converts a macOS virtual key code to a HID key code.
///
/// Returns `None` for keys with no HID equivalent — notably `fn` — which must be
/// dropped rather than forwarded under an invented encoding.
#[must_use]
pub fn to_hid(virtual_key: u16) -> Option<KeyCode> {
    TABLE
        .iter()
        .find(|(vk, _, _)| *vk == virtual_key)
        .map(|(_, page, usage)| KeyCode::new(*page, *usage))
}

/// Converts a HID key code to a macOS virtual key code.
///
/// Returns `None` for usages this keyboard layout has no position for, which is
/// normal: a peer may send a key that does not exist on the receiving machine.
#[must_use]
pub fn from_hid(key: KeyCode) -> Option<u16> {
    TABLE
        .iter()
        .find(|(_, page, usage)| *page == key.page && *usage == key.usage)
        .map(|(vk, _, _)| *vk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_input::keycode::keys;

    #[test]
    fn the_table_has_no_duplicate_virtual_keys() {
        // A duplicate would make `to_hid` silently prefer whichever came first.
        let mut seen: Vec<u16> = TABLE.iter().map(|(vk, _, _)| *vk).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            before,
            "duplicate virtual key code in the table"
        );
    }

    #[test]
    fn the_table_has_no_duplicate_hid_usages() {
        // A duplicate would make `from_hid` ambiguous, so injecting a key could
        // press the wrong physical position.
        let mut seen: Vec<(u16, u16)> = TABLE.iter().map(|(_, p, u)| (*p, *u)).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate HID usage in the table");
    }

    #[test]
    fn every_entry_round_trips_in_both_directions() {
        for (vk, page, usage) in TABLE {
            let key = KeyCode::new(*page, *usage);
            assert_eq!(to_hid(*vk), Some(key), "vk {vk:#04x} did not map forward");
            assert_eq!(from_hid(key), Some(*vk), "{key} did not map back");
        }
    }

    #[test]
    fn modifiers_map_to_the_correct_sides() {
        // Left and right must stay distinct: conflating them breaks chords that
        // depend on the side, and desynchronises the modifier bit set.
        assert_eq!(to_hid(0x37), Some(keys::LEFT_META));
        assert_eq!(to_hid(0x36), Some(keys::RIGHT_META));
        assert_eq!(to_hid(0x38), Some(keys::LEFT_SHIFT));
        assert_eq!(to_hid(0x3C), Some(keys::RIGHT_SHIFT));
        assert_eq!(to_hid(0x3A), Some(keys::LEFT_ALT));
        assert_eq!(to_hid(0x3D), Some(keys::RIGHT_ALT));
        assert_eq!(to_hid(0x3B), Some(keys::LEFT_CTRL));
        assert_eq!(to_hid(0x3E), Some(keys::RIGHT_CTRL));
    }

    #[test]
    fn caps_lock_maps_to_a_key_not_a_modifier() {
        let caps = to_hid(0x39).expect("Caps Lock is mapped");
        assert_eq!(caps, keys::CAPS_LOCK);
        assert!(!caps.is_modifier(), "Caps Lock must not be a held modifier");
    }

    #[test]
    fn the_fn_key_is_dropped_rather_than_invented() {
        // kVK_Function has no HID keyboard usage. Forwarding it would require
        // inventing an encoding the receiver could not act on.
        assert_eq!(to_hid(0x3F), None);
    }

    #[test]
    fn unmapped_virtual_keys_return_none() {
        assert_eq!(to_hid(0xFFFF), None);
        assert_eq!(to_hid(0x42), None, "gap in the table must stay a gap");
    }

    #[test]
    fn media_keys_land_on_the_consumer_page() {
        // 0xE2 is Mute on the consumer page and Left Alt on the keyboard page.
        // Losing the page would turn a volume key into a modifier.
        let mute = to_hid(0x4A).expect("mute is mapped");
        assert_eq!(mute, keys::MUTE);
        assert_eq!(mute.page, PAGE_CONSUMER);
        assert!(!mute.is_modifier());
        assert_ne!(mute, keys::LEFT_ALT);
    }

    #[test]
    fn letters_map_through_position_not_alphabetical_order() {
        // macOS orders letters by physical position. Assuming alphabetical order
        // is the classic way to get this table wrong.
        assert_eq!(to_hid(0x00), Some(keys::A));
        assert_eq!(to_hid(0x06), Some(keys::Z), "0x06 is Z, not G");
        assert_eq!(to_hid(0x08), Some(keys::C));
        assert_eq!(to_hid(0x0C), Some(keys::Q));
    }

    #[test]
    fn a_key_absent_from_this_layout_maps_back_to_none() {
        // A peer may send a key this machine has no position for. That must be a
        // clean `None`, not a panic or a wrong key.
        let nonexistent = KeyCode::keyboard(0xFF);
        assert_eq!(from_hid(nonexistent), None);
    }
}
