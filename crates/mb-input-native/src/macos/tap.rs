//! macOS input capture via a CoreGraphics event tap.
//!
//! # Threading
//!
//! The tap runs on a **dedicated thread with its own `CFRunLoop`**, never on the
//! main thread. macOS disables an event tap whose callback runs too slowly, and
//! the main run loop belongs to AppKit and the webview — a slow render there
//! would silently kill input capture.
//!
//! # Callback discipline
//!
//! The callback does the minimum: convert, ask the sink, return. It performs no
//! allocation and takes no lock, which is why
//! [`crate::macos::convert::modifier_transitions`] returns a fixed-size value and why
//! shared state is held in atomics.
//!
//! # Validation status
//!
//! **Requires platform validation.** Compiling proves the bindings type-check; it
//! proves nothing about tap behaviour under load, permission edge cases, or the
//! re-arm path. See `docs/platform-validation.md`.

#![allow(unsafe_code)]

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, CFRunLoopRef, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use mb_input::capture::{Disposition, InputCapture, InputSink};
use mb_input::error::InputError;
use mb_input::event::{InputEvent, ScrollDelta};
use mb_input::modifiers::Modifiers;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

// `CGEventTapEnable` re-enables a tap the system disabled. The `core-graphics`
// wrapper exposes it only as a method on `CGEventTap`, which does not exist yet
// at the point the callback is constructed, so it is bound directly here.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
}

/// State shared between the owning thread and the tap callback.
///
/// Every field is an atomic: the callback must never block, and a mutex here
/// would be exactly the kind of stall that gets the tap disabled.
#[derive(Debug)]
struct TapShared {
    /// True while input should be swallowed rather than reaching this machine.
    suppress: AtomicBool,
    /// Set by the owner to ask the run loop to exit.
    stop: AtomicBool,
    /// True once the tap is installed and the run loop is spinning.
    running: AtomicBool,
    /// How many times the OS disabled the tap and we re-enabled it.
    ///
    /// A rising count is the signal that the callback is too slow, and is worth
    /// surfacing in diagnostics rather than silently absorbing.
    rearms: AtomicU64,
    /// Keys seen that have no HID equivalent, such as `fn`.
    unmapped_keys: AtomicU64,
    /// Previous modifier state, for diffing `flagsChanged`.
    previous_modifiers: AtomicU8,
    /// The tap's mach port, so the callback can re-enable itself.
    tap_port: AtomicPtr<c_void>,
    /// The capture thread's run loop, so the owner can stop it.
    runloop: AtomicPtr<c_void>,
}

impl TapShared {
    fn new() -> Self {
        Self {
            suppress: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            running: AtomicBool::new(false),
            rearms: AtomicU64::new(0),
            unmapped_keys: AtomicU64::new(0),
            previous_modifiers: AtomicU8::new(0),
            tap_port: AtomicPtr::new(std::ptr::null_mut()),
            runloop: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}

/// Diagnostics from a running capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureDiagnostics {
    /// Times the OS disabled the tap and it was re-armed.
    pub rearms: u64,
    /// Key events dropped because they have no HID equivalent.
    pub unmapped_keys: u64,
}

/// macOS event-tap capture backend.
pub struct MacCapture {
    shared: Arc<TapShared>,
    thread: Option<JoinHandle<()>>,
}

impl Default for MacCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl MacCapture {
    /// Builds a stopped capture backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(TapShared::new()),
            thread: None,
        }
    }

    /// Sets whether captured input is swallowed instead of reaching this machine.
    ///
    /// Enabled while control is on another computer. Without it, keystrokes would
    /// land on both machines at once.
    pub fn set_suppressed(&self, suppressed: bool) {
        self.shared.suppress.store(suppressed, Ordering::Relaxed);
    }

    /// Returns counters worth surfacing in diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            rearms: self.shared.rearms.load(Ordering::Relaxed),
            unmapped_keys: self.shared.unmapped_keys.load(Ordering::Relaxed),
        }
    }
}

/// Converts one `CGEvent` and offers it to the sink.
///
/// Returns whether the event should be suppressed locally.
fn handle_event(
    shared: &TapShared,
    sink: &Arc<dyn InputSink>,
    event_type: CGEventType,
    event: &CGEvent,
) -> Disposition {
    // Events this application synthesised must never be captured. A machine
    // receiving remote input posts to the same HID location this tap listens on,
    // so without this check it would capture its own synthetic events and send
    // them straight back to the sender — an infinite echo that arrives as a
    // storm of duplicated keystrokes.
    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
        == crate::macos::inject::INJECTION_MARKER
    {
        return Disposition::PassThrough;
    }

    let mut suppress = false;
    let mut offer = |e: InputEvent| {
        if sink.on_event(&e) == Disposition::Suppress {
            suppress = true;
        }
    };

    match event_type {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let dx = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_X);
            let dy = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_Y);
            let moved = InputEvent::MouseMove { dx, dy };
            // Zero-delta moves are common and carry nothing; dropping them here
            // keeps them off the queue and off the wire.
            if !moved.is_noop() {
                offer(moved);
            }
        }

        CGEventType::LeftMouseDown
        | CGEventType::RightMouseDown
        | CGEventType::OtherMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseUp
        | CGEventType::OtherMouseUp => {
            let number = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
            let pressed = matches!(
                event_type,
                CGEventType::LeftMouseDown
                    | CGEventType::RightMouseDown
                    | CGEventType::OtherMouseDown
            );
            if let Some(button) = crate::macos::convert::button_from_number(number) {
                offer(InputEvent::MouseButton { button, pressed });
            }
        }

        CGEventType::ScrollWheel => {
            let continuous =
                event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS) != 0;
            let delta = crate::macos::convert::scroll_from_fields(
                continuous,
                event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1),
                event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2),
                event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1),
                event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2),
            );
            if !delta.is_zero() {
                offer(InputEvent::MouseWheel { delta });
            }
        }

        CGEventType::KeyDown | CGEventType::KeyUp => {
            let vk = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            match u16::try_from(vk)
                .ok()
                .and_then(crate::macos::keymap::to_hid)
            {
                Some(key) => offer(InputEvent::Key {
                    key,
                    pressed: matches!(event_type, CGEventType::KeyDown),
                    repeat,
                }),
                None => {
                    // Counted rather than logged: this is the input path, and a
                    // key with no HID equivalent must not be forwarded under an
                    // invented encoding.
                    shared.unmapped_keys.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        CGEventType::FlagsChanged => {
            // macOS reports no keyUp for modifiers, only the resulting flag
            // state. Diffing against the previous state derives the transitions,
            // and handles several modifiers changing in one event.
            let current = crate::macos::convert::modifiers_from_flags(event.get_flags().bits());
            let previous = Modifiers::from_bits(shared.previous_modifiers.load(Ordering::Relaxed));
            shared
                .previous_modifiers
                .store(current.bits(), Ordering::Relaxed);

            for (key, pressed) in
                crate::macos::convert::modifier_transitions(previous, current).as_slice()
            {
                offer(InputEvent::Key {
                    key: *key,
                    pressed: *pressed,
                    repeat: false,
                });
            }
        }

        _ => {}
    }

    if suppress {
        Disposition::Suppress
    } else {
        Disposition::PassThrough
    }
}

/// Event types the tap subscribes to.
fn events_of_interest() -> Vec<CGEventType> {
    vec![
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ]
}

impl InputCapture for MacCapture {
    fn start(&mut self, sink: Arc<dyn InputSink>) -> Result<(), InputError> {
        if self.thread.is_some() {
            return Ok(());
        }
        self.shared.stop.store(false, Ordering::SeqCst);

        let shared = Arc::clone(&self.shared);
        let (tx, rx) = mpsc::channel::<Result<(), InputError>>();

        let thread = std::thread::Builder::new()
            .name("mousebridge-tap".to_owned())
            .spawn(move || {
                let callback_shared = Arc::clone(&shared);
                let tap = CGEventTap::new(
                    // The HID location sees events before any application, which
                    // is what makes suppression possible at all.
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    // Default (not ListenOnly) is required in order to drop
                    // events. It is also what makes Accessibility mandatory.
                    CGEventTapOptions::Default,
                    events_of_interest(),
                    move |_proxy, event_type, event| {
                        if matches!(
                            event_type,
                            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                        ) {
                            // The silent killer: without re-enabling here, the
                            // application keeps running and captures nothing.
                            callback_shared.rearms.fetch_add(1, Ordering::Relaxed);
                            let port = callback_shared.tap_port.load(Ordering::Acquire);
                            if !port.is_null() {
                                // SAFETY: `port` is the tap's CFMachPort, stored
                                // after a successful creation and cleared before
                                // the tap is dropped, so it is valid here.
                                unsafe { CGEventTapEnable(port, true) };
                            }
                            return CallbackResult::Keep;
                        }

                        match handle_event(&callback_shared, &sink, event_type, event) {
                            Disposition::Suppress => CallbackResult::Drop,
                            Disposition::PassThrough => CallbackResult::Keep,
                        }
                    },
                );

                let Ok(tap) = tap else {
                    // CGEventTapCreate returns NULL when Accessibility has not
                    // been granted. That is by far the most likely cause, and
                    // reporting it as a generic failure would send the user
                    // hunting for a problem that has a one-click fix.
                    let _ = tx.send(Err(InputError::PermissionDenied {
                        operation: "create event tap",
                        permission: "Accessibility",
                    }));
                    return;
                };

                let port_ptr = tap.mach_port().as_concrete_TypeRef().cast::<c_void>();
                shared.tap_port.store(port_ptr, Ordering::Release);

                let source = tap.mach_port().create_runloop_source(0);
                let Ok(source) = source else {
                    let _ = tx.send(Err(InputError::OsCall {
                        api: "CFMachPortCreateRunLoopSource",
                        detail: "could not create a run loop source".to_owned(),
                    }));
                    return;
                };

                let runloop = CFRunLoop::get_current();
                // SAFETY: reading an immutable CFString constant exported by
                // CoreFoundation.
                unsafe { runloop.add_source(&source, kCFRunLoopCommonModes) };
                tap.enable();

                shared.runloop.store(
                    runloop.as_concrete_TypeRef().cast::<c_void>(),
                    Ordering::Release,
                );
                shared.running.store(true, Ordering::SeqCst);
                let _ = tx.send(Ok(()));

                // Blocks until `stop` calls CFRunLoopStop.
                CFRunLoop::run_current();

                shared.running.store(false, Ordering::SeqCst);
                shared
                    .tap_port
                    .store(std::ptr::null_mut(), Ordering::Release);
                shared
                    .runloop
                    .store(std::ptr::null_mut(), Ordering::Release);
            })
            .map_err(|e| InputError::OsCall {
                api: "thread::spawn",
                detail: e.to_string(),
            })?;

        // Block until the thread reports success or failure, so `start` returning
        // Ok genuinely means capture is running.
        match rx.recv() {
            Ok(Ok(())) => {
                self.thread = Some(thread);
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(InputError::OsCall {
                    api: "event tap thread",
                    detail: "the capture thread exited before reporting".to_owned(),
                })
            }
        }
    }

    fn stop(&mut self) -> Result<(), InputError> {
        self.shared.stop.store(true, Ordering::SeqCst);

        let runloop = self.shared.runloop.load(Ordering::Acquire);
        if !runloop.is_null() {
            // SAFETY: the pointer is a CFRunLoopRef published by the capture
            // thread while its run loop is live, and cleared before the thread
            // exits. CFRunLoopStop is documented as thread-safe.
            unsafe {
                let rl = CFRunLoop::wrap_under_get_rule(runloop.cast::<c_void>() as CFRunLoopRef);
                rl.stop();
            }
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    fn rearm(&mut self) -> Result<(), InputError> {
        let port = self.shared.tap_port.load(Ordering::Acquire);
        if port.is_null() {
            return Err(InputError::NotRunning);
        }
        // SAFETY: as in the callback — a non-null port is a live CFMachPort.
        unsafe { CGEventTapEnable(port, true) };
        Ok(())
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        // Leaving a tap installed would keep swallowing the user's input after
        // the application is gone.
        let _ = self.stop();
    }
}

/// Unused import guard: `ScrollDelta` documents the conversion contract above.
const _: Option<ScrollDelta> = None;
