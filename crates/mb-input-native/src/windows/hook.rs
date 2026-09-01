//! Windows input capture via low-level hooks.
//!
//! # Threading
//!
//! `SetWindowsHookEx` delivers callbacks on the thread that installed the hook,
//! and only while that thread pumps messages. Capture therefore runs on a
//! **dedicated thread with its own message loop**, never on the UI thread, where
//! a slow render would stall input delivery.
//!
//! # The timeout
//!
//! Windows gives a low-level hook procedure a hard budget —
//! `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout`, 300 ms by
//! default. Exceed it and the hook is skipped, silently, with no notification.
//! The procedure here converts, asks the sink, and returns; it allocates nothing
//! and takes no lock.
//!
//! # Suppression and cursor anchoring
//!
//! Blocking a mouse-move event stops the cursor moving, which creates a problem:
//! the hook reports an absolute destination point, so deltas are computed by
//! diffing against the previous position. That still works while suppressing —
//! Windows computes the destination from the *current* cursor position plus the
//! physical movement, and the cursor is not moving — but only away from the
//! screen edges. Pinned against an edge, the destination is clamped and the
//! movement beyond it is lost. Entering suppression therefore parks the cursor at
//! the centre of the primary display, where there is room to move in every
//! direction.
//!
//! # Validation status
//!
//! **Compile-checked only.** See `docs/platform-validation.md`.

#![allow(unsafe_code)]

use crate::windows::convert::{self, messages};
use mb_input::capture::{Disposition, InputCapture, InputSink};
use mb_input::error::InputError;
use mb_input::event::InputEvent;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, KBDLLHOOKSTRUCT, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

/// `LLKHF_EXTENDED` — the key carried an `E0` prefix.
const LLKHF_EXTENDED: u32 = 0x01;
/// `LLMHF_INJECTED` — a *mouse* event was synthesised rather than moved.
///
/// Deliberately separate from the keyboard flag below: they are different bits,
/// and using the keyboard value for mouse events means every injected pointer
/// move looks physical. On a machine receiving remote input that is a feedback
/// loop — it captures the motion it just injected and sends it back.
const LLMHF_INJECTED: u32 = 0x01;

/// `LLKHF_INJECTED` — a *keyboard* event was synthesised rather than typed.
const LLKHF_INJECTED: u32 = 0x10;

/// Written into `dwExtraInfo` on every event this application injects, so the
/// hook can recognise its own output.
///
/// Without it, a machine receiving remote input would capture the events it just
/// injected and send them straight back — an echo loop that arrives at the
/// sender as duplicated keystrokes.
pub const INJECTION_MARKER: usize = 0x004D_4252; // "MBR" in ASCII

/// State shared between the owning thread and the hook procedures.
#[derive(Debug)]
struct HookShared {
    suppress: AtomicBool,
    running: AtomicBool,
    thread_id: AtomicU32,
    /// Key events dropped because the scan code mapped to no HID usage.
    unmapped_keys: AtomicU64,
    /// Key events dropped because Windows reported no scan code at all.
    missing_scancodes: AtomicU64,
    /// User preference for lines scrolled per wheel notch, times 100.
    ///
    /// Stored scaled because there is no atomic float, and the value is read on
    /// the hook path where a lock is not acceptable.
    lines_per_notch_x100: AtomicU32,
}

/// Per-thread hook state.
///
/// Held in an [`Rc`] rather than an [`Arc`]: it never leaves the hook thread, so
/// requiring `Sync` would buy nothing and would rule out the [`Cell`]s.
struct HookState {
    sink: Arc<dyn InputSink>,
    shared: Arc<HookShared>,
    last_position: Cell<POINT>,
    have_last_position: Cell<bool>,
}

thread_local! {
    /// Hook procedures receive no user pointer, so state has to reach them out
    /// of band. A thread-local is the narrowest option: the procedure runs on
    /// the thread that installed it, so nothing is shared across threads and no
    /// lock is needed on the input path.
    static STATE: RefCell<Option<Rc<HookState>>> = const { RefCell::new(None) };
}

/// Runs `f` with the hook state, if this thread has any.
///
/// Uses `try_borrow` and never panics: unwinding out of a hook procedure crosses
/// an FFI boundary, which is undefined behaviour.
fn with_state<T>(f: impl FnOnce(&HookState) -> T) -> Option<T> {
    STATE
        .try_with(|state| {
            state
                .try_borrow()
                .ok()
                .and_then(|s| s.as_ref().map(|s| f(s)))
        })
        .ok()
        .flatten()
}

/// Diagnostics from a running capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureDiagnostics {
    /// Key events whose scan code mapped to no HID usage.
    pub unmapped_keys: u64,
    /// Key events for which Windows reported no scan code.
    pub missing_scancodes: u64,
}

/// Windows low-level hook capture backend.
pub struct WindowsCapture {
    shared: Arc<HookShared>,
    thread: Option<JoinHandle<()>>,
}

impl Default for WindowsCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsCapture {
    /// Builds a stopped capture backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(HookShared {
                suppress: AtomicBool::new(false),
                running: AtomicBool::new(false),
                thread_id: AtomicU32::new(0),
                unmapped_keys: AtomicU64::new(0),
                missing_scancodes: AtomicU64::new(0),
                lines_per_notch_x100: AtomicU32::new(
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "the default is a small positive constant"
                    )]
                    {
                        (convert::DEFAULT_LINES_PER_NOTCH * 100.0) as u32
                    },
                ),
            }),
            thread: None,
        }
    }

    /// Sets the user's wheel-scroll preference, from `SPI_GETWHEELSCROLLLINES`.
    pub fn set_lines_per_notch(&self, lines: f32) {
        if lines.is_finite() && lines > 0.0 {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "validated finite and positive immediately above"
            )]
            self.shared
                .lines_per_notch_x100
                .store((lines * 100.0) as u32, Ordering::Relaxed);
        }
    }

    /// Sets whether captured input is swallowed instead of reaching this machine.
    ///
    /// Entering suppression parks the cursor at the centre of the primary
    /// display. See the module documentation: without room to move, the
    /// destination point the hook reports is clamped at the screen edge and the
    /// movement beyond it is lost.
    pub fn set_suppressed(&self, suppressed: bool) {
        let was = self.shared.suppress.swap(suppressed, Ordering::SeqCst);
        if suppressed && !was {
            // SAFETY: `GetSystemMetrics` and `SetCursorPos` take plain integers
            // and are documented as callable from any thread.
            unsafe {
                let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
                if w > 0 && h > 0 {
                    let _ = SetCursorPos(w / 2, h / 2);
                }
            }
        }
    }

    /// Returns counters worth surfacing in diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            unmapped_keys: self.shared.unmapped_keys.load(Ordering::Relaxed),
            missing_scancodes: self.shared.missing_scancodes.load(Ordering::Relaxed),
        }
    }
}

/// Offers an event to the sink and reports whether to swallow it.
fn offer(state: &HookState, event: &InputEvent) -> Disposition {
    state.sink.on_event(event)
}

/// True if this event was synthesised rather than physically performed.
///
/// Both our own injected events and any other application's are ignored. A
/// software KVM forwards what the user physically did; replaying another
/// program's automation across machines is neither expected nor wanted.
///
/// `injected_flag` differs between the two hooks — see [`LLMHF_INJECTED`].
const fn is_synthetic(flags: u32, injected_flag: u32, extra_info: usize) -> bool {
    flags & injected_flag != 0 || extra_info == INJECTION_MARKER
}

/// `WH_MOUSE_LL` procedure.
///
/// # Safety
///
/// Called by Windows with `lparam` pointing to a valid `MSLLHOOKSTRUCT` whenever
/// `code >= 0`.
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        // SAFETY: documented contract — forward without inspecting the payload.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // SAFETY: for `code >= 0`, Windows guarantees `lparam` is a valid
    // `MSLLHOOKSTRUCT` for the duration of this call.
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    let message = u32::try_from(wparam.0).unwrap_or(0);

    let suppress = with_state(|state| {
        if is_synthetic(info.flags, LLMHF_INJECTED, info.dwExtraInfo) {
            return false;
        }

        let mut suppress = false;
        let mut deliver = |event: InputEvent| {
            if offer(state, &event) == Disposition::Suppress {
                suppress = true;
            }
        };

        match message {
            messages::MOUSEMOVE => {
                let last = state.last_position.get();
                let have_last = state.have_last_position.get();
                state.last_position.set(info.pt);
                state.have_last_position.set(true);

                if have_last {
                    let event = InputEvent::MouseMove {
                        dx: f64::from(info.pt.x - last.x),
                        dy: f64::from(info.pt.y - last.y),
                    };
                    if !event.is_noop() {
                        deliver(event);
                    } else {
                        // Still suppress a zero-delta move while remote, or the
                        // local cursor drifts on sub-pixel motion.
                        suppress = state.shared.suppress.load(Ordering::Relaxed);
                    }
                } else {
                    // First event after starting: there is no previous point to
                    // diff against, so this one only establishes the baseline.
                    suppress = state.shared.suppress.load(Ordering::Relaxed);
                }
            }

            messages::MOUSEWHEEL | messages::MOUSEHWHEEL => {
                let lines = f32::from(
                    u16::try_from(state.shared.lines_per_notch_x100.load(Ordering::Relaxed))
                        .unwrap_or(300),
                ) / 100.0;
                if let Some(delta) = convert::scroll_from_wheel(message, info.mouseData, lines)
                    && !delta.is_zero()
                {
                    deliver(InputEvent::MouseWheel { delta });
                }
            }

            _ => {
                if let Some((button, pressed)) =
                    convert::button_from_message(message, info.mouseData)
                {
                    deliver(InputEvent::MouseButton { button, pressed });
                }
            }
        }
        suppress
    })
    .unwrap_or(false);

    if suppress {
        // Non-zero swallows the event: it never reaches any application.
        return LRESULT(1);
    }
    // SAFETY: documented contract for a hook that does not consume the event.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// `WH_KEYBOARD_LL` procedure.
///
/// # Safety
///
/// Called by Windows with `lparam` pointing to a valid `KBDLLHOOKSTRUCT` whenever
/// `code >= 0`.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        // SAFETY: documented contract — forward without inspecting the payload.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // SAFETY: for `code >= 0`, Windows guarantees `lparam` is a valid
    // `KBDLLHOOKSTRUCT` for the duration of this call.
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let message = u32::try_from(wparam.0).unwrap_or(0);

    let suppress = with_state(|state| {
        if is_synthetic(info.flags.0, LLKHF_INJECTED, info.dwExtraInfo) {
            return false;
        }

        // `WM_SYSKEYDOWN` carries every Alt combination. Handling only
        // `WM_KEYDOWN` would make Alt chords invisible to capture.
        let pressed = matches!(message, messages::KEYDOWN | messages::SYSKEYDOWN);
        if !pressed && !matches!(message, messages::KEYUP | messages::SYSKEYUP) {
            return false;
        }

        let Ok(scan_code) = u16::try_from(info.scanCode) else {
            state
                .shared
                .missing_scancodes
                .fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if scan_code == 0 {
            // Some virtual keys arrive with no scan code. There is no physical
            // position to describe, so the event cannot be forwarded honestly.
            state
                .shared
                .missing_scancodes
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let extended = info.flags.0 & LLKHF_EXTENDED != 0;
        let Some(key) = crate::windows::keymap::to_hid(scan_code, extended) else {
            state.shared.unmapped_keys.fetch_add(1, Ordering::Relaxed);
            return false;
        };

        // The low-level hook reports no repeat count, so auto-repeat is not
        // distinguishable here. Reporting `false` is safe: the state tracker
        // treats a press of an already-held key as redundant, and the repeat
        // still reaches the remote machine as an extra key-down, which is what
        // produces the repeat there.
        offer(
            state,
            &InputEvent::Key {
                key,
                pressed,
                repeat: false,
            },
        ) == Disposition::Suppress
    })
    .unwrap_or(false);

    if suppress {
        return LRESULT(1);
    }
    // SAFETY: documented contract for a hook that does not consume the event.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

impl InputCapture for WindowsCapture {
    fn start(&mut self, sink: Arc<dyn InputSink>) -> Result<(), InputError> {
        if self.thread.is_some() {
            return Ok(());
        }

        let shared = Arc::clone(&self.shared);
        let (tx, rx) = mpsc::channel::<Result<(), InputError>>();

        let thread = std::thread::Builder::new()
            .name("mousebridge-hooks".to_owned())
            .spawn(move || {
                STATE.with(|slot| {
                    *slot.borrow_mut() = Some(Rc::new(HookState {
                        sink,
                        shared: Arc::clone(&shared),
                        last_position: Cell::new(POINT { x: 0, y: 0 }),
                        have_last_position: Cell::new(false),
                    }));
                });

                // SAFETY: a null module handle is correct for a low-level hook
                // whose procedure lives in this process, and thread id 0 installs
                // it for every thread in the desktop, which is what a global hook
                // requires.
                let hooks = unsafe {
                    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
                    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0);
                    (mouse, keyboard)
                };

                let (Ok(mouse_hook), Ok(keyboard_hook)) = hooks else {
                    // Clean up whichever half succeeded, or the surviving hook
                    // keeps intercepting input for a capture that never started.
                    // SAFETY: each handle is either valid or an error we skip.
                    unsafe {
                        if let Ok(h) = hooks.0 {
                            let _ = UnhookWindowsHookEx(h);
                        }
                        if let Ok(h) = hooks.1 {
                            let _ = UnhookWindowsHookEx(h);
                        }
                    }
                    let _ = tx.send(Err(InputError::OsCall {
                        api: "SetWindowsHookExW",
                        detail: "could not install the low-level input hooks".to_owned(),
                    }));
                    return;
                };

                // SAFETY: no preconditions; returns this thread's identifier.
                shared
                    .thread_id
                    .store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
                shared.running.store(true, Ordering::SeqCst);
                let _ = tx.send(Ok(()));

                pump_messages();

                shared.running.store(false, Ordering::SeqCst);
                shared.thread_id.store(0, Ordering::SeqCst);
                // SAFETY: both handles came from successful `SetWindowsHookExW`
                // calls on this thread and have not been unhooked yet.
                unsafe {
                    let _ = UnhookWindowsHookEx(mouse_hook);
                    let _ = UnhookWindowsHookEx(keyboard_hook);
                }
                STATE.with(|slot| {
                    *slot.borrow_mut() = None;
                });
            })
            .map_err(|e| InputError::OsCall {
                api: "thread::spawn",
                detail: e.to_string(),
            })?;

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
                    api: "hook thread",
                    detail: "the capture thread exited before reporting".to_owned(),
                })
            }
        }
    }

    fn stop(&mut self) -> Result<(), InputError> {
        let thread_id = self.shared.thread_id.load(Ordering::SeqCst);
        if thread_id != 0 {
            // SAFETY: posting `WM_QUIT` to a thread identifier is safe whether or
            // not the thread is still pumping; a stale id simply fails.
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
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
        // Windows does not disable a hook that can be re-enabled in place: once
        // it exceeds `LowLevelHooksTimeout` it is skipped, and recovery means
        // reinstalling it. That is a full restart of the capture thread, which
        // the supervisor performs via `stop` then `start`.
        Err(InputError::Unsupported {
            what: "re-arming a Windows hook in place; restart capture instead",
        })
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        // A surviving hook would keep swallowing input after the process exits.
        let _ = self.stop();
    }
}

/// Runs the message loop until `WM_QUIT`.
fn pump_messages() {
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is a valid, properly initialised MSG. A null window
        // handle retrieves messages for the whole thread, which is what a
        // hook-owning thread needs.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            // Zero is WM_QUIT; negative is an error. Both end the loop.
            break;
        }
        // SAFETY: `message` was just filled in by `GetMessageW`.
        unsafe {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}
