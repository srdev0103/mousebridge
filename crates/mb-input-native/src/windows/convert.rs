//! Turning Win32 hook parameters into [`InputEvent`]s, and back for injection.
//!
//! As with the macOS backend, these functions take **plain values** rather than
//! Win32 structures, so every decision is unit tested on any host and the FFI
//! layer stays a thin extractor.

use mb_input::event::ScrollDelta;
use mb_input::keycode::{KeyCode, keys};
use mb_input::modifiers::{Modifiers, MouseButton};

/// Windows mouse messages seen by a low-level hook.
pub mod messages {
    /// `WM_MOUSEMOVE`
    pub const MOUSEMOVE: u32 = 0x0200;
    /// `WM_LBUTTONDOWN`
    pub const LBUTTONDOWN: u32 = 0x0201;
    /// `WM_LBUTTONUP`
    pub const LBUTTONUP: u32 = 0x0202;
    /// `WM_RBUTTONDOWN`
    pub const RBUTTONDOWN: u32 = 0x0204;
    /// `WM_RBUTTONUP`
    pub const RBUTTONUP: u32 = 0x0205;
    /// `WM_MBUTTONDOWN`
    pub const MBUTTONDOWN: u32 = 0x0207;
    /// `WM_MBUTTONUP`
    pub const MBUTTONUP: u32 = 0x0208;
    /// `WM_MOUSEWHEEL`
    pub const MOUSEWHEEL: u32 = 0x020A;
    /// `WM_XBUTTONDOWN`
    pub const XBUTTONDOWN: u32 = 0x020B;
    /// `WM_XBUTTONUP`
    pub const XBUTTONUP: u32 = 0x020C;
    /// `WM_MOUSEHWHEEL`
    pub const MOUSEHWHEEL: u32 = 0x020E;

    /// `WM_KEYDOWN`
    pub const KEYDOWN: u32 = 0x0100;
    /// `WM_KEYUP`
    pub const KEYUP: u32 = 0x0101;
    /// `WM_SYSKEYDOWN` — a key pressed while Alt is held, or F10.
    ///
    /// Must be handled alongside `WM_KEYDOWN`: without it, every Alt combination
    /// is invisible to capture.
    pub const SYSKEYDOWN: u32 = 0x0104;
    /// `WM_SYSKEYUP`
    pub const SYSKEYUP: u32 = 0x0105;
}

/// One notch of a standard mouse wheel, as Windows defines it.
pub const WHEEL_DELTA: f32 = 120.0;

/// Default lines scrolled per notch when the system setting cannot be read.
///
/// Windows exposes the user's preference through `SPI_GETWHEELSCROLLLINES`; three
/// is the shipped default.
pub const DEFAULT_LINES_PER_NOTCH: f32 = 3.0;

/// `XBUTTON1`, reported in the high word of `mouseData`.
const XBUTTON1: u16 = 0x0001;
/// `XBUTTON2`.
const XBUTTON2: u16 = 0x0002;

/// Maps a mouse message to a button and its direction.
///
/// `mouse_data` is only consulted for the `X` buttons, where the high word says
/// which of the two thumb buttons moved.
#[must_use]
pub fn button_from_message(message: u32, mouse_data: u32) -> Option<(MouseButton, bool)> {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the high word is 16 bits by definition"
    )]
    let high_word = (mouse_data >> 16) as u16;

    match message {
        messages::LBUTTONDOWN => Some((MouseButton::Left, true)),
        messages::LBUTTONUP => Some((MouseButton::Left, false)),
        messages::RBUTTONDOWN => Some((MouseButton::Right, true)),
        messages::RBUTTONUP => Some((MouseButton::Right, false)),
        messages::MBUTTONDOWN => Some((MouseButton::Middle, true)),
        messages::MBUTTONUP => Some((MouseButton::Middle, false)),
        messages::XBUTTONDOWN | messages::XBUTTONUP => {
            let pressed = message == messages::XBUTTONDOWN;
            match high_word {
                XBUTTON1 => Some((MouseButton::Back, pressed)),
                XBUTTON2 => Some((MouseButton::Forward, pressed)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Builds a scroll delta from a wheel message.
///
/// `mouse_data`'s high word is a **signed** notch count scaled by
/// [`WHEEL_DELTA`]. Reading it as unsigned turns every upward scroll into a
/// violent downward one, which is the classic way to get this wrong.
#[must_use]
pub fn scroll_from_wheel(
    message: u32,
    mouse_data: u32,
    lines_per_notch: f32,
) -> Option<ScrollDelta> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "the high word is a signed 16-bit notch count by definition"
    )]
    let raw = ((mouse_data >> 16) as u16) as i16;
    let notches = f32::from(raw) / WHEEL_DELTA;
    let lines = notches * lines_per_notch;

    match message {
        messages::MOUSEWHEEL => Some(ScrollDelta::lines(0.0, lines)),
        messages::MOUSEHWHEEL => Some(ScrollDelta::lines(lines, 0.0)),
        _ => None,
    }
}

/// Converts a line count back to a `mouseData` wheel value for injection.
#[must_use]
pub fn lines_to_wheel_delta(lines: f32, lines_per_notch: f32) -> Option<i32> {
    if !lines.is_finite() || lines_per_notch <= 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "wheel deltas are far below i32 range"
    )]
    let value = (lines / lines_per_notch * WHEEL_DELTA).round() as i32;
    (value != 0).then_some(value)
}

/// The virtual desktop rectangle, in physical pixels.
///
/// Windows composes every monitor into one coordinate space whose origin can be
/// negative — a display positioned left of the primary one has a negative `left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualDesktop {
    /// Leftmost pixel, `SM_XVIRTUALSCREEN`.
    pub left: i32,
    /// Topmost pixel, `SM_YVIRTUALSCREEN`.
    pub top: i32,
    /// Width in pixels, `SM_CXVIRTUALSCREEN`.
    pub width: i32,
    /// Height in pixels, `SM_CYVIRTUALSCREEN`.
    pub height: i32,
}

/// Normalises a virtual-desktop pixel position to the 0..65535 range
/// `SendInput` expects with `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
///
/// The division is by `width - 1`, not `width`: the range is inclusive at both
/// ends, so dividing by the width leaves the rightmost pixel unreachable. On a
/// 3840-wide desktop that is a one-pixel dead strip along the right edge —
/// precisely where a screen-edge crossing happens.
///
/// Returns `None` for a degenerate desktop, which is reachable in practice while
/// every display is asleep.
#[must_use]
pub fn to_absolute(x: f64, y: f64, desktop: VirtualDesktop) -> Option<(i32, i32)> {
    if desktop.width <= 1 || desktop.height <= 1 || !x.is_finite() || !y.is_finite() {
        return None;
    }
    let span_x = f64::from(desktop.width - 1);
    let span_y = f64::from(desktop.height - 1);

    let nx = ((x - f64::from(desktop.left)) / span_x).clamp(0.0, 1.0);
    let ny = ((y - f64::from(desktop.top)) / span_y).clamp(0.0, 1.0);

    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is clamped to 0..=65535 before conversion"
    )]
    Some(((nx * 65535.0).round() as i32, (ny * 65535.0).round() as i32))
}

/// Whether releasing this key alone would trigger a Windows shell shortcut.
///
/// Windows treats a press-and-release of a lone `Windows` key as "open the Start
/// menu", and a lone `Alt` as "focus the menu bar". Both fire on the *release*.
///
/// This matters because of how a boundary crossing works. The user holds the
/// Windows key, moves the pointer to another machine, and the local machine
/// releases everything it was holding — so Windows sees the key go down and come
/// back up with nothing in between, and the Start menu opens on the computer the
/// user just left.
///
/// The fix is to inject an inert keystroke before the release, so the sequence is
/// no longer "modifier alone". `saw_other_input` records whether anything else
/// was already injected while the modifier was held.
#[must_use]
pub fn needs_shell_shortcut_guard(key: KeyCode, saw_other_input: bool) -> bool {
    if saw_other_input {
        return false;
    }
    matches!(
        key,
        keys::LEFT_META | keys::RIGHT_META | keys::LEFT_ALT | keys::RIGHT_ALT
    )
}

/// Tracks whether a modifier release would look like a lone press to the shell.
///
/// Extracted from the injector so it can be tested on any host. The rule is
/// simple to state and easy to get wrong in place: a guard is needed when a
/// `Windows` or `Alt` key is released and *nothing else* was injected while it
/// was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellShortcutGuard {
    saw_other_input: bool,
}

impl ShellShortcutGuard {
    /// A guard with no history.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            saw_other_input: false,
        }
    }

    /// Records a key event.
    ///
    /// `modifiers_after` is the modifier set *after* this event has been applied,
    /// which is what the caller has to hand. Using it avoids the ordering trap of
    /// asking whether the set was empty before — by then the press has already
    /// been recorded, so the answer is always "no".
    pub fn on_key(&mut self, key: KeyCode, pressed: bool, modifiers_after: Modifiers) {
        if !key.is_modifier() {
            self.saw_other_input = true;
            return;
        }
        // Exactly one modifier held after a press means a chord is starting, so
        // the "nothing else yet" window opens here.
        if pressed && modifiers_after.bits().count_ones() == 1 {
            self.saw_other_input = false;
        }
    }

    /// Records a mouse button, which also counts as intervening input.
    pub const fn on_button(&mut self) {
        self.saw_other_input = true;
    }

    /// Whether releasing `key` now needs the inert-keystroke guard.
    #[must_use]
    pub fn needs_guard(&self, key: KeyCode) -> bool {
        needs_shell_shortcut_guard(key, self.saw_other_input)
    }

    /// Clears the history, after a full release.
    pub const fn reset(&mut self) {
        self.saw_other_input = false;
    }
}

/// Virtual key used as the inert keystroke by the shell-shortcut guard.
///
/// `VK_NONAME` is reserved by Microsoft and bound to no function, which is
/// exactly the property needed: it must break the "modifier alone" sequence
/// without doing anything a user would notice.
pub const VK_NONAME: u16 = 0xFC;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_buttons_map_in_both_directions() {
        assert_eq!(
            button_from_message(messages::LBUTTONDOWN, 0),
            Some((MouseButton::Left, true))
        );
        assert_eq!(
            button_from_message(messages::RBUTTONUP, 0),
            Some((MouseButton::Right, false))
        );
        assert_eq!(
            button_from_message(messages::MBUTTONDOWN, 0),
            Some((MouseButton::Middle, true))
        );
    }

    #[test]
    fn thumb_buttons_are_distinguished_by_the_high_word() {
        assert_eq!(
            button_from_message(messages::XBUTTONDOWN, u32::from(XBUTTON1) << 16),
            Some((MouseButton::Back, true))
        );
        assert_eq!(
            button_from_message(messages::XBUTTONUP, u32::from(XBUTTON2) << 16),
            Some((MouseButton::Forward, false))
        );
        // A thumb button the OS does not define must be dropped, not guessed.
        assert_eq!(
            button_from_message(messages::XBUTTONDOWN, 0x0009 << 16),
            None
        );
    }

    #[test]
    fn non_button_messages_are_ignored() {
        assert_eq!(button_from_message(messages::MOUSEMOVE, 0), None);
        assert_eq!(button_from_message(messages::MOUSEWHEEL, 0), None);
    }

    #[test]
    fn wheel_delta_is_read_as_signed() {
        // The bug this guards: reading the high word as unsigned turns a scroll
        // up into a scroll down of about 546 notches.
        let up = scroll_from_wheel(messages::MOUSEWHEEL, 120 << 16, 3.0).expect("wheel");
        assert!((up.y - 3.0).abs() < 1e-6, "expected +3 lines, got {}", up.y);

        let down_raw = ((-120i16) as u16 as u32) << 16;
        let down = scroll_from_wheel(messages::MOUSEWHEEL, down_raw, 3.0).expect("wheel");
        assert!(
            (down.y + 3.0).abs() < 1e-6,
            "expected -3 lines, got {}",
            down.y
        );
    }

    #[test]
    fn horizontal_wheel_lands_on_the_x_axis() {
        let d = scroll_from_wheel(messages::MOUSEHWHEEL, 120 << 16, 3.0).expect("wheel");
        assert!((d.x - 3.0).abs() < 1e-6);
        assert_eq!(d.y, 0.0);
    }

    #[test]
    fn wheel_respects_the_users_lines_per_notch_setting() {
        let d = scroll_from_wheel(messages::MOUSEWHEEL, 120 << 16, 10.0).expect("wheel");
        assert!((d.y - 10.0).abs() < 1e-6);
    }

    #[test]
    fn wheel_conversion_round_trips() {
        for lines in [3.0f32, -3.0, 6.0, -9.0] {
            let raw = lines_to_wheel_delta(lines, 3.0).expect("non-zero");
            let packed = ((raw as i16) as u16 as u32) << 16;
            let back = scroll_from_wheel(messages::MOUSEWHEEL, packed, 3.0).expect("wheel");
            assert!((back.y - lines).abs() < 1e-3, "{lines} -> {}", back.y);
        }
        assert_eq!(lines_to_wheel_delta(0.0, 3.0), None);
        assert_eq!(lines_to_wheel_delta(f32::NAN, 3.0), None);
    }

    #[test]
    fn absolute_coordinates_reach_both_extremes() {
        // The off-by-one that matters: dividing by width instead of width-1
        // leaves the rightmost pixel unreachable, and that pixel column is
        // exactly where an edge crossing happens.
        let desktop = VirtualDesktop {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(to_absolute(0.0, 0.0, desktop), Some((0, 0)));
        assert_eq!(to_absolute(1919.0, 1079.0, desktop), Some((65535, 65535)));
    }

    #[test]
    fn absolute_coordinates_handle_a_negative_origin() {
        // A monitor positioned left of the primary one gives the virtual desktop
        // a negative left edge. Ignoring the origin puts the cursor on the wrong
        // screen entirely.
        let desktop = VirtualDesktop {
            left: -1920,
            top: 0,
            width: 3840,
            height: 1080,
        };
        assert_eq!(to_absolute(-1920.0, 0.0, desktop), Some((0, 0)));
        assert_eq!(
            to_absolute(1919.0, 1079.0, desktop).map(|p| p.0),
            Some(65535)
        );

        // The true centre of the inclusive pixel range [-1920, 1919] is -0.5,
        // not 0.0 — the half-pixel offset is a consequence of dividing by
        // `width - 1`, which is what makes the last pixel column reachable.
        let (centre, _) = to_absolute(-0.5, 0.0, desktop).expect("centre");
        assert!((centre - 32768).abs() <= 1, "centre landed at {centre}");

        // The primary origin therefore sits a hair right of centre, and must
        // still be on the correct side of it.
        let (origin, _) = to_absolute(0.0, 0.0, desktop).expect("origin");
        assert!(
            origin > centre,
            "origin must be right of the desktop centre"
        );
        assert!(
            origin - centre < 32,
            "but only by a pixel: {origin} vs {centre}"
        );
    }

    #[test]
    fn absolute_coordinates_clamp_rather_than_wrap() {
        let desktop = VirtualDesktop {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(to_absolute(99999.0, -5.0, desktop), Some((65535, 0)));
    }

    #[test]
    fn a_degenerate_desktop_is_rejected() {
        // Reachable while every display is asleep. Dividing by zero here would
        // send a NaN into SendInput.
        let asleep = VirtualDesktop {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(to_absolute(10.0, 10.0, asleep), None);
        let ok = VirtualDesktop {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(to_absolute(f64::NAN, 0.0, ok), None);
    }

    #[test]
    fn a_lone_meta_or_alt_release_needs_a_guard() {
        // Crossing a screen edge while holding the Windows key would otherwise
        // pop the Start menu on the machine the user just left.
        assert!(needs_shell_shortcut_guard(keys::LEFT_META, false));
        assert!(needs_shell_shortcut_guard(keys::RIGHT_META, false));
        assert!(needs_shell_shortcut_guard(keys::LEFT_ALT, false));
        assert!(needs_shell_shortcut_guard(keys::RIGHT_ALT, false));
    }

    #[test]
    fn no_guard_is_needed_once_something_else_was_typed() {
        // Win+E already opened Explorer; the release is not a lone modifier.
        assert!(!needs_shell_shortcut_guard(keys::LEFT_META, true));
        assert!(!needs_shell_shortcut_guard(keys::LEFT_ALT, true));
    }

    #[test]
    fn the_guard_opens_its_window_when_a_chord_starts() {
        // The ordering trap: the modifier set is queried *after* the press has
        // been applied, so it already contains the key. Asking "was it empty
        // before" would always answer no, and the guard would never arm.
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::A, true, Modifiers::NONE);
        assert!(!guard.needs_guard(keys::LEFT_META), "no modifier held yet");

        guard.on_key(keys::LEFT_META, true, Modifiers::LEFT_META);
        assert!(
            guard.needs_guard(keys::LEFT_META),
            "a fresh chord must arm the guard even after earlier typing"
        );
    }

    #[test]
    fn typing_during_a_chord_closes_the_window() {
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::LEFT_META, true, Modifiers::LEFT_META);
        assert!(guard.needs_guard(keys::LEFT_META));

        // Win+E: Explorer already opened, so the release is not a lone press.
        guard.on_key(keys::A, true, Modifiers::LEFT_META);
        assert!(!guard.needs_guard(keys::LEFT_META));
    }

    #[test]
    fn a_mouse_click_also_closes_the_window() {
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::LEFT_ALT, true, Modifiers::LEFT_ALT);
        assert!(guard.needs_guard(keys::LEFT_ALT));
        guard.on_button();
        assert!(!guard.needs_guard(keys::LEFT_ALT));
    }

    #[test]
    fn a_second_modifier_does_not_reopen_the_window() {
        // Ctrl+Alt is still "no ordinary input yet", but adding the second
        // modifier must not reset a window that typing had already closed.
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::LEFT_ALT, true, Modifiers::LEFT_ALT);
        guard.on_key(keys::A, true, Modifiers::LEFT_ALT);
        guard.on_key(
            keys::LEFT_CTRL,
            true,
            Modifiers::LEFT_ALT.union(Modifiers::LEFT_CTRL),
        );
        assert!(!guard.needs_guard(keys::LEFT_ALT), "window was reopened");
    }

    #[test]
    fn the_boundary_crossing_case_is_guarded() {
        // The scenario that motivated this: the user holds the Windows key and
        // moves the pointer to another machine. The local side releases
        // everything, and without a guard the Start menu opens behind them.
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::LEFT_META, true, Modifiers::LEFT_META);
        assert!(guard.needs_guard(keys::LEFT_META));
    }

    #[test]
    fn reset_clears_the_history() {
        let mut guard = ShellShortcutGuard::new();
        guard.on_key(keys::A, true, Modifiers::NONE);
        guard.reset();
        guard.on_key(keys::LEFT_META, true, Modifiers::LEFT_META);
        assert!(guard.needs_guard(keys::LEFT_META));
    }

    #[test]
    fn other_modifiers_need_no_guard() {
        // Control and Shift have no lone-press shell behaviour.
        assert!(!needs_shell_shortcut_guard(keys::LEFT_CTRL, false));
        assert!(!needs_shell_shortcut_guard(keys::LEFT_SHIFT, false));
        assert!(!needs_shell_shortcut_guard(keys::A, false));
    }
}
