//! Windows input injection via `SendInput`.
//!
//! # Why motion is absolute
//!
//! `MOUSEEVENTF_MOVE` without `MOUSEEVENTF_ABSOLUTE` passes the delta through
//! Windows pointer ballistics — the "Enhanced pointer precision" acceleration
//! curve. The sending machine has already applied *its* acceleration at capture
//! time, so relative injection accelerates the same movement twice: motion
//! becomes non-linear, small movements round to zero, and the two cursors drift
//! apart with no way to resynchronise.
//!
//! Every motion event is therefore injected as an absolute position in the
//! virtual desktop, normalised to `0..=65535`. See `docs/adr/0001`.
//!
//! # Why keys are scan codes
//!
//! `KEYEVENTF_SCANCODE` describes the physical position and lets the receiving
//! machine apply its own layout. Injecting a virtual key code would impose the
//! *sender's* layout, so a German keyboard driving a US machine would type the
//! wrong letters.
//!
//! # Validation status
//!
//! **Compile-checked only.** See `docs/platform-validation.md`.

#![allow(unsafe_code)]

use crate::windows::convert::{self, ShellShortcutGuard, VK_NONAME, VirtualDesktop};
use crate::windows::hook::INJECTION_MARKER;
use mb_input::error::InputError;
use mb_input::event::InputEvent;
use mb_input::modifiers::MouseButton;
use mb_input::state::InputStateTracker;
use mb_input::{InputInject, KeyCode};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// `XBUTTON1`, for `mouseData` on the thumb buttons.
const XBUTTON1: u32 = 0x0001;
/// `XBUTTON2`.
const XBUTTON2: u32 = 0x0002;

/// Windows injection backend.
pub struct WindowsInjector {
    /// What this injector has pressed, so it can always release it.
    tracker: InputStateTracker,
    /// Last requested cursor position, in virtual-desktop pixels.
    cursor: (f64, f64),
    /// Decides when a modifier release needs an inert keystroke in front of it.
    ///
    /// A lone `Windows` press-and-release opens the Start menu, and a lone `Alt`
    /// focuses the menu bar. The state machine lives in `convert` so it can be
    /// tested on any host.
    guard: ShellShortcutGuard,
}

impl Default for WindowsInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsInjector {
    /// Builds an injector.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tracker: InputStateTracker::new(),
            cursor: (0.0, 0.0),
            guard: ShellShortcutGuard::new(),
        }
    }

    /// What this injector currently holds.
    #[must_use]
    pub const fn tracker(&self) -> &InputStateTracker {
        &self.tracker
    }

    /// Reads the current virtual desktop rectangle.
    ///
    /// Queried per injection rather than cached: displays are hot-plugged, and a
    /// stale rectangle sends the cursor to the wrong screen with no error.
    fn virtual_desktop() -> VirtualDesktop {
        // SAFETY: `GetSystemMetrics` takes an index and returns an integer. It
        // has no preconditions and is callable from any thread.
        unsafe {
            VirtualDesktop {
                left: GetSystemMetrics(SM_XVIRTUALSCREEN),
                top: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        }
    }

    /// Sends a batch of prepared inputs.
    fn send(inputs: &[INPUT]) -> Result<(), InputError> {
        if inputs.is_empty() {
            return Ok(());
        }
        let size = i32::try_from(size_of::<INPUT>()).unwrap_or(0);
        // SAFETY: `inputs` is a valid slice of correctly initialised INPUT
        // structures, and `size` is the size of that structure.
        let sent = unsafe { SendInput(inputs, size) };

        if sent as usize == inputs.len() {
            return Ok(());
        }
        // A short count means UIPI blocked the injection, which happens whenever
        // an elevated window has focus. Reported specifically because it is a
        // documented product limitation rather than a transient fault, and the
        // UI must be able to explain it rather than showing a generic error.
        Err(InputError::OsCall {
            api: "SendInput",
            detail: format!(
                "only {sent} of {} events were accepted; an elevated window may have focus",
                inputs.len()
            ),
        })
    }

    fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, mouse_data: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: mouse_data,
                    dwFlags: flags,
                    time: 0,
                    // Marks the event as ours so our own hook ignores it.
                    // Without this a receiving machine captures what it just
                    // injected and echoes it back to the sender.
                    dwExtraInfo: INJECTION_MARKER,
                },
            },
        }
    }

    fn key_input(scan_code: u16, extended: bool, pressed: bool) -> INPUT {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !pressed {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                    wScan: scan_code,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: INJECTION_MARKER,
                },
            },
        }
    }

    /// An inert keystroke used to break a "modifier alone" sequence.
    fn noname_input(pressed: bool) -> INPUT {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        if !pressed {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    // Sent as a virtual key rather than a scan code: VK_NONAME
                    // has no physical position, which is exactly why it is inert.
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(VK_NONAME),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: INJECTION_MARKER,
                },
            },
        }
    }

    fn inject_move_to(&mut self, x: f64, y: f64) -> Result<(), InputError> {
        self.cursor = (x, y);
        let Some((ax, ay)) = convert::to_absolute(x, y, Self::virtual_desktop()) else {
            // Reachable while every display is asleep. Dropping the motion is
            // correct: there is nowhere to put the cursor.
            return Ok(());
        };
        Self::send(&[Self::mouse_input(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            ax,
            ay,
            0,
        )])
    }

    fn inject_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), InputError> {
        self.guard.on_button();
        let (flags, data) = match (button, pressed) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        Self::send(&[Self::mouse_input(flags, 0, 0, data)])
    }

    fn inject_scroll(&mut self, delta: mb_input::ScrollDelta) -> Result<(), InputError> {
        let lines_per_notch = convert::DEFAULT_LINES_PER_NOTCH;
        let mut inputs = Vec::new();

        if let Some(v) = convert::lines_to_wheel_delta(delta.y, lines_per_notch) {
            #[allow(
                clippy::cast_sign_loss,
                reason = "mouseData carries a signed value in an unsigned field, per the API"
            )]
            inputs.push(Self::mouse_input(MOUSEEVENTF_WHEEL, 0, 0, v as u32));
        }
        if let Some(h) = convert::lines_to_wheel_delta(delta.x, lines_per_notch) {
            #[allow(
                clippy::cast_sign_loss,
                reason = "mouseData carries a signed value in an unsigned field, per the API"
            )]
            inputs.push(Self::mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, h as u32));
        }
        Self::send(&inputs)
    }

    fn inject_key(&mut self, key: KeyCode, pressed: bool) -> Result<(), InputError> {
        let Some((scan_code, extended)) = crate::windows::keymap::from_hid(key) else {
            // The peer sent a key this layout has no position for. Dropping it is
            // correct; there is nothing sensible to press instead.
            return Ok(());
        };

        // Guard against the shell shortcuts. Releasing a lone Windows key opens
        // the Start menu and a lone Alt focuses the menu bar — on the machine the
        // user has just left, if the crossing happened mid-chord. Injecting an
        // inert keystroke first means the release is no longer "modifier alone".
        let needs_guard = !pressed && self.guard.needs_guard(key);

        // Recorded after the guard decision and with the post-apply modifier set,
        // which `inject` has already updated on the tracker.
        self.guard.on_key(key, pressed, self.tracker.modifiers());

        if needs_guard {
            Self::send(&[Self::noname_input(true), Self::noname_input(false)])?;
        }
        Self::send(&[Self::key_input(scan_code, extended, pressed)])
    }
}

impl InputInject for WindowsInjector {
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError> {
        self.tracker.apply(event);

        match event {
            InputEvent::MouseMoveTo { x, y } => self.inject_move_to(*x, *y),
            InputEvent::MouseMove { dx, dy } => {
                // Relative motion is a capture-side representation; injecting it
                // would double-apply pointer acceleration. Resolving it against
                // the topology is the router's job, and applying the delta to the
                // last known position is the closest correct fallback.
                let (x, y) = (self.cursor.0 + dx, self.cursor.1 + dy);
                self.inject_move_to(x, y)
            }
            InputEvent::MouseButton { button, pressed } => self.inject_button(*button, *pressed),
            InputEvent::MouseWheel { delta } => self.inject_scroll(*delta),
            InputEvent::Key { key, pressed, .. } => self.inject_key(*key, *pressed),
            InputEvent::ReleaseAll | InputEvent::Leave => self.release_all(),
            InputEvent::Enter { .. } => {
                let events = self.tracker.press_events();
                let mut first_error = None;
                for e in &events {
                    if let Err(err) = self.inject(e)
                        && first_error.is_none()
                    {
                        first_error = Some(err);
                    }
                }
                first_error.map_or(Ok(()), Err)
            }
        }
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        let events = self.tracker.release_events();
        let mut first_error = None;
        for e in &events {
            let result = match e {
                InputEvent::Key { key, .. } => self.inject_key(*key, false),
                InputEvent::MouseButton { button, .. } => self.inject_button(*button, false),
                _ => Ok(()),
            };
            // Every release is attempted even if one fails: abandoning the
            // sequence half-way is how a modifier ends up stuck.
            if let Err(err) = result
                && first_error.is_none()
            {
                first_error = Some(err);
            }
        }
        self.tracker.clear();
        self.guard.reset();
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for WindowsInjector {
    fn drop(&mut self) {
        // Dropping while holding a modifier would leave it stuck with no owner
        // left to release it.
        let _ = self.release_all();
    }
}
