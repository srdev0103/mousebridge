//! macOS input injection via CoreGraphics synthetic events.
//!
//! # Avoiding a feedback loop
//!
//! Injected events are posted to the HID tap location, which is exactly where
//! this application's own capture tap listens. Without a marker, a machine
//! receiving remote input would capture its own synthetic events and forward them
//! straight back. Every event created here therefore carries
//! [`INJECTION_MARKER`] in its user-data field, and the capture tap drops events
//! that carry it.
//!
//! # Injection fails silently without Accessibility
//!
//! `CGEventPost` returns `void`. Without the Accessibility permission it does
//! nothing at all and reports no error, and `CGEventSourceCreate` still succeeds,
//! so **nothing in this module can detect the missing grant**. Verified on macOS
//! 15.7.9: the self-test in `examples/input-check.rs` requested a cursor move,
//! got no error, and the cursor did not move.
//!
//! Two consequences, both load-bearing:
//!
//! * Callers must gate injection on [`mb_platform::Platform::missing_permissions`]
//!   rather than on an error from here, or the application will look connected
//!   and simply do nothing.
//! * Any test of injection must assert the *observable effect* — read the cursor
//!   back — never the return value.
//!
//! # Validation status
//!
//! **Requires platform validation.** See `docs/platform-validation.md`.
//!
//! This module contains no `unsafe`: the `core-graphics` wrappers cover every
//! call it makes, and the thread-local event source removed the last reason to
//! assert `Send` by hand.

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use mb_input::InputInject;
use mb_input::error::InputError;
use mb_input::event::InputEvent;
use mb_input::modifiers::{Modifiers, MouseButton};
use mb_input::state::InputStateTracker;

/// Written into every synthetic event's user-data field so this application's own
/// capture tap can recognise and ignore it.
///
/// An arbitrary constant; only its uniqueness matters.
pub const INJECTION_MARKER: i64 = 0x004D_4252_4944_4745; // "MBRIDGE" in ASCII

thread_local! {
    /// The event source used for synthesis, created once per thread.
    ///
    /// Held in a thread-local rather than in [`MacInjector`] for a specific
    /// reason: `CGEventSource` is `!Send`, but an injector must be movable onto
    /// the dedicated injection thread. Rather than asserting `unsafe impl Send`
    /// on the basis that CoreFoundation reference counting is atomic — probably
    /// true, but an assertion about someone else's library — the source simply
    /// never crosses a thread. Each thread creates its own on first use, which is
    /// also the usage pattern Apple's APIs are least surprising under.
    static EVENT_SOURCE: Result<CGEventSource, ()> =
        CGEventSource::new(CGEventSourceStateID::HIDSystemState);
}

/// Runs `f` with this thread's event source.
fn with_source<T>(f: impl FnOnce(CGEventSource) -> Result<T, InputError>) -> Result<T, InputError> {
    EVENT_SOURCE.with(|source| match source {
        Ok(source) => f(source.clone()),
        Err(()) => Err(InputError::OsCall {
            api: "CGEventSourceCreate",
            detail: "could not create an event source for this thread".to_owned(),
        }),
    })
}

/// macOS injection backend.
pub struct MacInjector {
    /// What this injector has pressed, so it can always release it.
    tracker: InputStateTracker,
    /// Last known cursor position, in global display points.
    ///
    /// Mouse button and drag events must be posted at a position, and the
    /// position must be the one the cursor is actually at, or a click lands
    /// somewhere the user did not aim.
    cursor: CGPoint,
}

impl MacInjector {
    /// Builds an injector.
    ///
    /// # Errors
    ///
    /// [`InputError::OsCall`] if the event source could not be created, which in
    /// practice means the process has no window server connection.
    /// `HIDSystemState` is used rather than a private state so that synthetic
    /// events combine with genuine hardware input — modifier state and click
    /// counts included. A private state would isolate them and break any chord
    /// formed across the two, which is exactly what a software KVM must not do.
    pub fn new() -> Result<Self, InputError> {
        // Fail here rather than on the first keystroke, so a broken window-server
        // connection surfaces at start-up where it can be reported.
        with_source(|_| Ok(()))?;
        Ok(Self {
            tracker: InputStateTracker::new(),
            cursor: CGPoint::new(0.0, 0.0),
        })
    }

    /// The position this injector believes the cursor occupies.
    #[must_use]
    pub const fn cursor(&self) -> (f64, f64) {
        (self.cursor.x, self.cursor.y)
    }

    /// What this injector currently holds.
    #[must_use]
    pub const fn tracker(&self) -> &InputStateTracker {
        &self.tracker
    }

    /// Stamps and posts an event.
    fn post(&self, event: &CGEvent) {
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTION_MARKER);
        event.post(CGEventTapLocation::HID);
    }

    /// Translates tracked modifiers into `CGEventFlags`.
    ///
    /// Synthetic key events carry no modifier state of their own. Without this,
    /// injecting Command then `C` produces two unrelated keystrokes instead of a
    /// copy — the single most common bug in synthetic-input code on this
    /// platform.
    fn flags(&self) -> CGEventFlags {
        let m = self.tracker.modifiers();
        let mut flags = CGEventFlags::CGEventFlagNull;
        if m.ctrl() {
            flags |= CGEventFlags::CGEventFlagControl;
        }
        if m.shift() {
            flags |= CGEventFlags::CGEventFlagShift;
        }
        if m.alt() {
            flags |= CGEventFlags::CGEventFlagAlternate;
        }
        if m.meta() {
            flags |= CGEventFlags::CGEventFlagCommand;
        }
        flags
    }

    /// Chooses the correct motion event type for the currently held buttons.
    ///
    /// Posting `MouseMoved` while a button is down does not continue a drag; the
    /// OS requires the matching `*MouseDragged` type, or dragging a window across
    /// the screen boundary simply stops.
    fn motion_type(&self) -> (CGEventType, CGMouseButton) {
        let buttons = self.tracker.buttons();
        if buttons.contains(MouseButton::Left) {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if buttons.contains(MouseButton::Right) {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else if !buttons.is_empty() {
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        }
    }

    fn inject_move_to(&mut self, x: f64, y: f64) -> Result<(), InputError> {
        self.cursor = CGPoint::new(x, y);
        let (event_type, button) = self.motion_type();
        let cursor = self.cursor;
        let event = with_source(|source| {
            CGEvent::new_mouse_event(source, event_type, cursor, button).map_err(|()| {
                InputError::OsCall {
                    api: "CGEventCreateMouseEvent",
                    detail: "could not create a motion event".to_owned(),
                }
            })
        })?;
        self.post(&event);
        Ok(())
    }

    fn inject_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), InputError> {
        let (event_type, cg_button) = match (button, pressed) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
            (_, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
            (_, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
        };

        let cursor = self.cursor;
        let event = with_source(|source| {
            CGEvent::new_mouse_event(source, event_type, cursor, cg_button).map_err(|()| {
                InputError::OsCall {
                    api: "CGEventCreateMouseEvent",
                    detail: "could not create a button event".to_owned(),
                }
            })
        })?;

        // CGMouseButton has no Back or Forward. The button number field is what
        // actually identifies the button for the `Other*` event types.
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_BUTTON_NUMBER,
            crate::macos::convert::button_to_number(button),
        );
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }

    fn inject_key(&mut self, key: mb_input::KeyCode, pressed: bool) -> Result<(), InputError> {
        let Some(vk) = crate::macos::keymap::from_hid(key) else {
            // The peer sent a key this layout has no position for. Dropping it is
            // correct; there is nothing sensible to press instead.
            return Ok(());
        };

        let event = with_source(|source| {
            CGEvent::new_keyboard_event(source, vk, pressed).map_err(|()| InputError::OsCall {
                api: "CGEventCreateKeyboardEvent",
                detail: "could not create a keyboard event".to_owned(),
            })
        })?;
        // Flags are taken *after* the tracker has been updated by the caller, so
        // that pressing Command and then C produces a real chord.
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }

    fn inject_scroll(&mut self, delta: mb_input::ScrollDelta) -> Result<(), InputError> {
        // `CGScrollEventUnit`: 0 is pixels, 1 is lines.
        let (unit, wheel1, wheel2) = if delta.precise {
            #[allow(clippy::cast_possible_truncation, reason = "scroll deltas are small")]
            let px = |lines: f32| (lines * crate::macos::convert::PIXELS_PER_LINE).round() as i32;
            (0u32, px(delta.y), px(delta.x))
        } else {
            match (
                crate::macos::convert::lines_to_notches(delta.y),
                crate::macos::convert::lines_to_notches(delta.x),
            ) {
                (None, None) => return Ok(()),
                (y, x) => (1u32, y.unwrap_or(0), x.unwrap_or(0)),
            }
        };

        let event = with_source(|source| {
            CGEvent::new_scroll_event(source, unit, 2, wheel1, wheel2, 0).map_err(|()| {
                InputError::OsCall {
                    api: "CGEventCreateScrollWheelEvent2",
                    detail: "could not create a scroll event".to_owned(),
                }
            })
        })?;
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }
}

impl InputInject for MacInjector {
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError> {
        // Tracked before posting, so `flags()` already reflects a modifier that
        // is part of this very event.
        self.tracker.apply(event);

        match event {
            InputEvent::MouseMoveTo { x, y } => self.inject_move_to(*x, *y),
            InputEvent::MouseMove { dx, dy } => {
                // Relative motion is a capture-side representation. Injecting it
                // would pass through this machine's pointer acceleration, which
                // the sender has already applied, so the motion would accelerate
                // twice and drift. Resolving it to a position is the router's
                // job; applying the delta to the last known position is the
                // closest correct fallback.
                let (x, y) = (self.cursor.x + dx, self.cursor.y + dy);
                self.inject_move_to(x, y)
            }
            InputEvent::MouseButton { button, pressed } => self.inject_button(*button, *pressed),
            InputEvent::MouseWheel { delta } => self.inject_scroll(*delta),
            InputEvent::Key { key, pressed, .. } => self.inject_key(*key, *pressed),
            InputEvent::ReleaseAll | InputEvent::Leave => self.release_all(),
            InputEvent::Enter { held, .. } => {
                // The peer is handing over mid-chord or mid-drag. Re-assert what
                // it says the user is physically holding.
                let events = self.tracker.press_events();
                let _ = held;
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
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for MacInjector {
    fn drop(&mut self) {
        // Dropping while holding a modifier would leave it stuck with no owner
        // left to release it.
        let _ = self.release_all();
    }
}

/// Documented above; referenced so the import is not unused in every build.
const _: Option<Modifiers> = None;
