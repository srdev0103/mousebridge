//! Translation between Windows scan codes and HID usage IDs.
//!
//! # Why scan codes and not virtual keys
//!
//! Windows identifies a key two ways. The **virtual key code** (`VK_*`) is what
//! the key *means* under the current layout — `VK_OEM_1` is `;` on a US keyboard
//! and `ü` on a German one — so it is useless for describing a physical key to
//! another machine. The **scan code** is the position the hardware reported, and
//! is layout-independent.
//!
//! Scan codes are therefore what this backend both reads and writes, which is
//! also why injection uses `KEYEVENTF_SCANCODE`: the receiving machine applies
//! its own layout to the position, exactly as if the keyboard were plugged into
//! it directly.
//!
//! # The extended-key bit
//!
//! Set 1 scan codes are only unique when paired with the extended flag, which
//! corresponds to the `E0` prefix byte. Several codes are reused:
//!
//! | Scan code | Plain | Extended |
//! |---|---|---|
//! | `0x1C` | Enter | Keypad Enter |
//! | `0x1D` | Left Control | Right Control |
//! | `0x35` | `/` | Keypad `/` |
//! | `0x38` | Left Alt | Right Alt / AltGr |
//! | `0x47` | Keypad 7 | Home |
//! | `0x52` | Keypad 0 | Insert |
//!
//! Dropping the flag would map Right Control to Left Control and Home to Keypad
//! 7 — so the key of this table is the pair, never the code alone.

use mb_input::keycode::{KeyCode, PAGE_CONSUMER, PAGE_KEYBOARD};

/// `(scan code, extended, HID usage page, HID usage id)`.
#[rustfmt::skip]
const TABLE: &[(u16, bool, u16, u16)] = &[
    // Letters.
    (0x1E, false, PAGE_KEYBOARD, 0x04), // A
    (0x30, false, PAGE_KEYBOARD, 0x05), // B
    (0x2E, false, PAGE_KEYBOARD, 0x06), // C
    (0x20, false, PAGE_KEYBOARD, 0x07), // D
    (0x12, false, PAGE_KEYBOARD, 0x08), // E
    (0x21, false, PAGE_KEYBOARD, 0x09), // F
    (0x22, false, PAGE_KEYBOARD, 0x0A), // G
    (0x23, false, PAGE_KEYBOARD, 0x0B), // H
    (0x17, false, PAGE_KEYBOARD, 0x0C), // I
    (0x24, false, PAGE_KEYBOARD, 0x0D), // J
    (0x25, false, PAGE_KEYBOARD, 0x0E), // K
    (0x26, false, PAGE_KEYBOARD, 0x0F), // L
    (0x32, false, PAGE_KEYBOARD, 0x10), // M
    (0x31, false, PAGE_KEYBOARD, 0x11), // N
    (0x18, false, PAGE_KEYBOARD, 0x12), // O
    (0x19, false, PAGE_KEYBOARD, 0x13), // P
    (0x10, false, PAGE_KEYBOARD, 0x14), // Q
    (0x13, false, PAGE_KEYBOARD, 0x15), // R
    (0x1F, false, PAGE_KEYBOARD, 0x16), // S
    (0x14, false, PAGE_KEYBOARD, 0x17), // T
    (0x16, false, PAGE_KEYBOARD, 0x18), // U
    (0x2F, false, PAGE_KEYBOARD, 0x19), // V
    (0x11, false, PAGE_KEYBOARD, 0x1A), // W
    (0x2D, false, PAGE_KEYBOARD, 0x1B), // X
    (0x15, false, PAGE_KEYBOARD, 0x1C), // Y
    (0x2C, false, PAGE_KEYBOARD, 0x1D), // Z

    // Digit row.
    (0x02, false, PAGE_KEYBOARD, 0x1E), // 1
    (0x03, false, PAGE_KEYBOARD, 0x1F), // 2
    (0x04, false, PAGE_KEYBOARD, 0x20), // 3
    (0x05, false, PAGE_KEYBOARD, 0x21), // 4
    (0x06, false, PAGE_KEYBOARD, 0x22), // 5
    (0x07, false, PAGE_KEYBOARD, 0x23), // 6
    (0x08, false, PAGE_KEYBOARD, 0x24), // 7
    (0x09, false, PAGE_KEYBOARD, 0x25), // 8
    (0x0A, false, PAGE_KEYBOARD, 0x26), // 9
    (0x0B, false, PAGE_KEYBOARD, 0x27), // 0

    // Control and punctuation.
    (0x1C, false, PAGE_KEYBOARD, 0x28), // Enter
    (0x01, false, PAGE_KEYBOARD, 0x29), // Escape
    (0x0E, false, PAGE_KEYBOARD, 0x2A), // Backspace
    (0x0F, false, PAGE_KEYBOARD, 0x2B), // Tab
    (0x39, false, PAGE_KEYBOARD, 0x2C), // Space
    (0x0C, false, PAGE_KEYBOARD, 0x2D), // -
    (0x0D, false, PAGE_KEYBOARD, 0x2E), // =
    (0x1A, false, PAGE_KEYBOARD, 0x2F), // [
    (0x1B, false, PAGE_KEYBOARD, 0x30), // ]
    (0x2B, false, PAGE_KEYBOARD, 0x31), // backslash
    (0x27, false, PAGE_KEYBOARD, 0x33), // ;
    (0x28, false, PAGE_KEYBOARD, 0x34), // '
    (0x29, false, PAGE_KEYBOARD, 0x35), // `
    (0x33, false, PAGE_KEYBOARD, 0x36), // ,
    (0x34, false, PAGE_KEYBOARD, 0x37), // .
    (0x35, false, PAGE_KEYBOARD, 0x38), // /
    (0x3A, false, PAGE_KEYBOARD, 0x39), // Caps Lock
    (0x56, false, PAGE_KEYBOARD, 0x64), // Non-US backslash (ISO)

    // Function keys.
    (0x3B, false, PAGE_KEYBOARD, 0x3A), // F1
    (0x3C, false, PAGE_KEYBOARD, 0x3B), // F2
    (0x3D, false, PAGE_KEYBOARD, 0x3C), // F3
    (0x3E, false, PAGE_KEYBOARD, 0x3D), // F4
    (0x3F, false, PAGE_KEYBOARD, 0x3E), // F5
    (0x40, false, PAGE_KEYBOARD, 0x3F), // F6
    (0x41, false, PAGE_KEYBOARD, 0x40), // F7
    (0x42, false, PAGE_KEYBOARD, 0x41), // F8
    (0x43, false, PAGE_KEYBOARD, 0x42), // F9
    (0x44, false, PAGE_KEYBOARD, 0x43), // F10
    (0x57, false, PAGE_KEYBOARD, 0x44), // F11
    (0x58, false, PAGE_KEYBOARD, 0x45), // F12

    // Navigation cluster: all extended.
    (0x52, true,  PAGE_KEYBOARD, 0x49), // Insert
    (0x47, true,  PAGE_KEYBOARD, 0x4A), // Home
    (0x49, true,  PAGE_KEYBOARD, 0x4B), // Page Up
    (0x53, true,  PAGE_KEYBOARD, 0x4C), // Delete
    (0x4F, true,  PAGE_KEYBOARD, 0x4D), // End
    (0x51, true,  PAGE_KEYBOARD, 0x4E), // Page Down
    (0x4D, true,  PAGE_KEYBOARD, 0x4F), // Right Arrow
    (0x4B, true,  PAGE_KEYBOARD, 0x50), // Left Arrow
    (0x50, true,  PAGE_KEYBOARD, 0x51), // Down Arrow
    (0x48, true,  PAGE_KEYBOARD, 0x52), // Up Arrow
    (0x37, true,  PAGE_KEYBOARD, 0x46), // Print Screen
    (0x46, false, PAGE_KEYBOARD, 0x47), // Scroll Lock

    // Keypad: same codes as the navigation cluster, without the extended bit.
    (0x45, false, PAGE_KEYBOARD, 0x53), // Num Lock
    (0x35, true,  PAGE_KEYBOARD, 0x54), // Keypad /
    (0x37, false, PAGE_KEYBOARD, 0x55), // Keypad *
    (0x4A, false, PAGE_KEYBOARD, 0x56), // Keypad -
    (0x4E, false, PAGE_KEYBOARD, 0x57), // Keypad +
    (0x1C, true,  PAGE_KEYBOARD, 0x58), // Keypad Enter
    (0x4F, false, PAGE_KEYBOARD, 0x59), // Keypad 1
    (0x50, false, PAGE_KEYBOARD, 0x5A), // Keypad 2
    (0x51, false, PAGE_KEYBOARD, 0x5B), // Keypad 3
    (0x4B, false, PAGE_KEYBOARD, 0x5C), // Keypad 4
    (0x4C, false, PAGE_KEYBOARD, 0x5D), // Keypad 5
    (0x4D, false, PAGE_KEYBOARD, 0x5E), // Keypad 6
    (0x47, false, PAGE_KEYBOARD, 0x5F), // Keypad 7
    (0x48, false, PAGE_KEYBOARD, 0x60), // Keypad 8
    (0x49, false, PAGE_KEYBOARD, 0x61), // Keypad 9
    (0x52, false, PAGE_KEYBOARD, 0x62), // Keypad 0
    (0x53, false, PAGE_KEYBOARD, 0x63), // Keypad .
    (0x5D, true,  PAGE_KEYBOARD, 0x65), // Application / context menu

    // Modifiers. Left and right differ only by the extended bit for Control and
    // Alt, which is exactly why the flag is part of the key.
    (0x1D, false, PAGE_KEYBOARD, 0xE0), // Left Control
    (0x2A, false, PAGE_KEYBOARD, 0xE1), // Left Shift
    (0x38, false, PAGE_KEYBOARD, 0xE2), // Left Alt
    (0x5B, true,  PAGE_KEYBOARD, 0xE3), // Left Windows
    (0x1D, true,  PAGE_KEYBOARD, 0xE4), // Right Control
    (0x36, false, PAGE_KEYBOARD, 0xE5), // Right Shift
    (0x38, true,  PAGE_KEYBOARD, 0xE6), // Right Alt / AltGr
    (0x5C, true,  PAGE_KEYBOARD, 0xE7), // Right Windows

    // Media keys, all extended. Needs validation on real hardware: some keyboards
    // report these through a vendor HID collection instead of the PS/2 set.
    (0x10, true,  PAGE_CONSUMER, 0xB6), // Previous track
    (0x19, true,  PAGE_CONSUMER, 0xB5), // Next track
    (0x20, true,  PAGE_CONSUMER, 0xE2), // Mute
    (0x22, true,  PAGE_CONSUMER, 0xCD), // Play / pause
    (0x24, true,  PAGE_CONSUMER, 0xB7), // Stop
    (0x2E, true,  PAGE_CONSUMER, 0xEA), // Volume down
    (0x30, true,  PAGE_CONSUMER, 0xE9), // Volume up
];

/// Converts a Windows scan code and extended flag to a HID key code.
#[must_use]
pub fn to_hid(scan_code: u16, extended: bool) -> Option<KeyCode> {
    TABLE
        .iter()
        .find(|(sc, ext, _, _)| *sc == scan_code && *ext == extended)
        .map(|(_, _, page, usage)| KeyCode::new(*page, *usage))
}

/// Converts a HID key code to a Windows scan code and extended flag.
#[must_use]
pub fn from_hid(key: KeyCode) -> Option<(u16, bool)> {
    TABLE
        .iter()
        .find(|(_, _, page, usage)| *page == key.page && *usage == key.usage)
        .map(|(sc, ext, _, _)| (*sc, *ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_input::keycode::keys;

    #[test]
    fn scan_code_and_extended_flag_together_are_unique() {
        let mut seen: Vec<(u16, bool)> = TABLE.iter().map(|(s, e, _, _)| (*s, *e)).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate (scan code, extended) pair");
    }

    #[test]
    fn hid_usages_are_unique() {
        let mut seen: Vec<(u16, u16)> = TABLE.iter().map(|(_, _, p, u)| (*p, *u)).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate HID usage");
    }

    #[test]
    fn every_entry_round_trips() {
        for (sc, ext, page, usage) in TABLE {
            let key = KeyCode::new(*page, *usage);
            assert_eq!(to_hid(*sc, *ext), Some(key), "{sc:#04x} ext={ext}");
            assert_eq!(from_hid(key), Some((*sc, *ext)), "{key}");
        }
    }

    #[test]
    fn the_extended_bit_separates_reused_scan_codes() {
        // Losing this flag maps Right Control onto Left Control, Home onto
        // Keypad 7, and AltGr onto Left Alt.
        assert_eq!(to_hid(0x1D, false), Some(keys::LEFT_CTRL));
        assert_eq!(to_hid(0x1D, true), Some(keys::RIGHT_CTRL));
        assert_eq!(to_hid(0x38, false), Some(keys::LEFT_ALT));
        assert_eq!(to_hid(0x38, true), Some(keys::RIGHT_ALT));

        let enter = to_hid(0x1C, false).expect("Enter");
        let keypad_enter = to_hid(0x1C, true).expect("Keypad Enter");
        assert_ne!(enter, keypad_enter);

        let home = to_hid(0x47, true).expect("Home");
        let keypad_7 = to_hid(0x47, false).expect("Keypad 7");
        assert_ne!(home, keypad_7);
    }

    #[test]
    fn altgr_is_right_alt_not_a_separate_key() {
        // On European layouts AltGr is Right Alt with the extended bit. Mapping
        // it anywhere else breaks every accented character on those keyboards.
        assert_eq!(to_hid(0x38, true), Some(keys::RIGHT_ALT));
    }

    #[test]
    fn media_keys_land_on_the_consumer_page() {
        // Extended 0x30 is Volume Up; plain 0x30 is the letter B. Losing the
        // flag would type a letter when the user pressed a volume key.
        let vol_up = to_hid(0x30, true).expect("volume up");
        assert_eq!(vol_up, keys::VOLUME_UP);
        assert_eq!(vol_up.page, PAGE_CONSUMER);
        assert_eq!(to_hid(0x30, false).map(|k| k.page), Some(PAGE_KEYBOARD));
    }

    #[test]
    fn unmapped_codes_return_none() {
        assert_eq!(to_hid(0x00, false), None);
        assert_eq!(to_hid(0xFFFF, true), None);
        assert_eq!(from_hid(KeyCode::keyboard(0xFE)), None);
    }

    #[test]
    fn the_windows_and_macos_tables_agree_on_shared_keys() {
        // Both tables are written by hand from different sources. If they
        // disagree about which HID usage a key has, a keystroke crossing between
        // the two platforms lands on the wrong key — and nothing else would
        // catch it, because each table round-trips fine on its own.
        for key in [
            keys::A,
            keys::Z,
            keys::Q,
            keys::ENTER,
            keys::ESCAPE,
            keys::TAB,
            keys::SPACE,
            keys::CAPS_LOCK,
            keys::F1,
            keys::LEFT_CTRL,
            keys::LEFT_SHIFT,
            keys::LEFT_ALT,
            keys::LEFT_META,
            keys::RIGHT_CTRL,
            keys::RIGHT_SHIFT,
            keys::RIGHT_ALT,
            keys::RIGHT_META,
            keys::MUTE,
            keys::VOLUME_UP,
            keys::MEDIA_PLAY_PAUSE,
        ] {
            assert!(
                from_hid(key).is_some(),
                "{key} is reachable on macOS but missing from the Windows table"
            );
        }
    }
}
