//! An in-memory input backend for tests and headless development.
//!
//! This is the piece that lets the whole pipeline — capture, routing,
//! suppression, injection, state recovery — be exercised on one machine with no
//! second computer, no privacy grants and no physical hardware. Without it, every
//! routing or stuck-key bug would need two machines and a human to reproduce.
//!
//! It is a *simulation*, not a stub. [`VirtualInjector`] maintains a cursor
//! position and a real [`InputStateTracker`], so a test can assert what a remote
//! machine would actually be holding after a sequence, rather than merely that
//! some function was called.

use crate::capture::{Disposition, InputCapture, InputSink};
use crate::error::InputError;
use crate::event::InputEvent;
use crate::inject::InputInject;
use crate::state::InputStateTracker;
use std::sync::{Arc, Mutex};

/// Counters describing what a [`VirtualCapture`] has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureStats {
    /// Events delivered to the sink.
    pub emitted: usize,
    /// Events the sink asked to suppress.
    pub suppressed: usize,
    /// Events the sink allowed through to the local machine.
    pub passed_through: usize,
}

/// A programmable capture source.
pub struct VirtualCapture {
    sink: Option<Arc<dyn InputSink>>,
    running: bool,
    start_error: Option<InputError>,
    disabled_reason: Option<&'static str>,
    stats: CaptureStats,
}

impl Default for VirtualCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualCapture {
    /// A capture backend that starts successfully.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sink: None,
            running: false,
            start_error: None,
            disabled_reason: None,
            stats: CaptureStats {
                emitted: 0,
                suppressed: 0,
                passed_through: 0,
            },
        }
    }

    /// A backend that refuses to start for want of a permission.
    ///
    /// Models the macOS case where Accessibility has not been granted, so the
    /// permission-gating path can be tested without revoking a real grant.
    #[must_use]
    pub fn denying_permission(permission: &'static str) -> Self {
        Self {
            start_error: Some(InputError::PermissionDenied {
                operation: "start capture",
                permission,
            }),
            ..Self::new()
        }
    }

    /// Feeds an event through the sink, returning what the sink decided.
    ///
    /// # Errors
    ///
    /// [`InputError::NotRunning`] before `start`, or
    /// [`InputError::CaptureDisabled`] after [`VirtualCapture::simulate_os_disable`].
    pub fn emit(&mut self, event: &InputEvent) -> Result<Disposition, InputError> {
        if let Some(reason) = self.disabled_reason {
            return Err(InputError::CaptureDisabled { reason });
        }
        let sink = self.sink.as_ref().ok_or(InputError::NotRunning)?;
        if !self.running {
            return Err(InputError::NotRunning);
        }

        let disposition = sink.on_event(event);
        self.stats.emitted += 1;
        match disposition {
            Disposition::Suppress => self.stats.suppressed += 1,
            Disposition::PassThrough => self.stats.passed_through += 1,
        }
        Ok(disposition)
    }

    /// Simulates the operating system revoking capture.
    ///
    /// macOS disables an event tap whose callback ran too slowly; Windows drops a
    /// hook that exceeded `LowLevelHooksTimeout`. Both happen without notice, and
    /// an application that does not detect and re-arm keeps running while
    /// capturing nothing. Reproducing that here is the only practical way to test
    /// the recovery path.
    pub fn simulate_os_disable(&mut self, reason: &'static str) {
        self.disabled_reason = Some(reason);
    }

    /// Counters for assertions.
    #[must_use]
    pub const fn stats(&self) -> CaptureStats {
        self.stats
    }
}

impl InputCapture for VirtualCapture {
    fn start(&mut self, sink: Arc<dyn InputSink>) -> Result<(), InputError> {
        if let Some(e) = self.start_error.clone() {
            return Err(e);
        }
        self.sink = Some(sink);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), InputError> {
        self.running = false;
        self.sink = None;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running && self.disabled_reason.is_none()
    }

    fn rearm(&mut self) -> Result<(), InputError> {
        if self.sink.is_none() {
            return Err(InputError::NotRunning);
        }
        self.disabled_reason = None;
        self.running = true;
        Ok(())
    }
}

/// A simulated machine being controlled.
///
/// Tracks a cursor position and held input the same way a real machine would, so
/// assertions can be about observable state — "the remote is holding nothing",
/// "the cursor is at the left edge" — rather than about call sequences.
#[derive(Debug)]
pub struct VirtualInjector {
    events: Vec<InputEvent>,
    tracker: InputStateTracker,
    cursor: (f64, f64),
    fail_next: Option<InputError>,
    release_all_calls: usize,
}

impl Default for VirtualInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualInjector {
    /// A machine with the cursor at the origin and nothing held.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            events: Vec::new(),
            tracker: InputStateTracker::new(),
            cursor: (0.0, 0.0),
            fail_next: None,
            release_all_calls: 0,
        }
    }

    /// Every event injected, in order.
    #[must_use]
    pub fn events(&self) -> &[InputEvent] {
        &self.events
    }

    /// The simulated cursor position.
    #[must_use]
    pub const fn cursor(&self) -> (f64, f64) {
        self.cursor
    }

    /// What this machine currently believes it is holding.
    #[must_use]
    pub const fn tracker(&self) -> &InputStateTracker {
        &self.tracker
    }

    /// True when nothing is held — the state a machine must always be able to
    /// reach after a disconnect.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.tracker.is_clean()
    }

    /// Number of times [`InputInject::release_all`] was called.
    #[must_use]
    pub const fn release_all_calls(&self) -> usize {
        self.release_all_calls
    }

    /// Makes the next injection fail, to exercise error handling.
    pub fn fail_next(&mut self, error: InputError) {
        self.fail_next = Some(error);
    }

    /// Discards the recorded event log, keeping cursor and held state.
    pub fn clear_log(&mut self) {
        self.events.clear();
    }
}

impl InputInject for VirtualInjector {
    fn inject(&mut self, event: &InputEvent) -> Result<(), InputError> {
        if let Some(e) = self.fail_next.take() {
            // Recorded before returning: a real OS call may well have taken
            // effect before reporting failure, and tests should see that.
            self.events.push(event.clone());
            return Err(e);
        }

        match event {
            InputEvent::MouseMove { dx, dy } => {
                self.cursor.0 += dx;
                self.cursor.1 += dy;
            }
            InputEvent::MouseMoveTo { x, y } => self.cursor = (*x, *y),
            _ => {}
        }
        self.tracker.apply(event);
        self.events.push(event.clone());
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        self.release_all_calls += 1;
        for e in self.tracker.release_events() {
            self.events.push(e);
        }
        self.tracker.clear();
        Ok(())
    }
}

/// Where input is currently going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// This machine. Events pass through untouched.
    Local,
    /// Another machine. Events are suppressed locally and forwarded.
    Remote,
}

/// A capture sink that forwards to a simulated remote machine.
///
/// Models the routing decision at the heart of the product: when control is
/// local, events pass through; when it is remote, they are suppressed here and
/// injected there.
///
/// # Note on the mutex
///
/// [`InputSink::on_event`] must not lock in production — the real implementation
/// pushes into a lock-free bounded queue. A mutex is acceptable here precisely
/// because this type is single-threaded test scaffolding, and using one keeps the
/// harness small enough to be obviously correct.
pub struct RoutingSink {
    destination: Mutex<Destination>,
    remote: Arc<Mutex<VirtualInjector>>,
}

impl RoutingSink {
    /// Builds a sink that starts by routing to the local machine.
    #[must_use]
    pub fn new(remote: Arc<Mutex<VirtualInjector>>) -> Self {
        Self {
            destination: Mutex::new(Destination::Local),
            remote,
        }
    }

    /// Switches where input goes.
    pub fn set_destination(&self, destination: Destination) {
        if let Ok(mut guard) = self.destination.lock() {
            *guard = destination;
        }
    }
}

impl InputSink for RoutingSink {
    fn on_event(&self, event: &InputEvent) -> Disposition {
        let destination = self.destination.lock().map_or(Destination::Local, |g| *g);

        match destination {
            Destination::Local => Disposition::PassThrough,
            Destination::Remote => {
                if let Ok(mut remote) = self.remote.lock() {
                    // A failed injection must not change the suppression
                    // decision: passing the event through locally instead would
                    // type into the wrong computer.
                    let _ = remote.inject(event);
                }
                Disposition::Suppress
            }
        }
    }
}

/// A complete capture-to-injection pipeline in one process.
pub struct Loopback {
    /// The capture source. Drive it with [`VirtualCapture::emit`].
    pub capture: VirtualCapture,
    /// The routing sink. Switch destination with [`RoutingSink::set_destination`].
    pub sink: Arc<RoutingSink>,
    /// The simulated remote machine.
    pub remote: Arc<Mutex<VirtualInjector>>,
}

impl Loopback {
    /// Builds and starts a pipeline routing to the local machine.
    ///
    /// # Errors
    ///
    /// Propagates a failure from [`InputCapture::start`].
    pub fn start() -> Result<Self, InputError> {
        let remote = Arc::new(Mutex::new(VirtualInjector::new()));
        let sink = Arc::new(RoutingSink::new(Arc::clone(&remote)));
        let mut capture = VirtualCapture::new();
        capture.start(Arc::clone(&sink) as Arc<dyn InputSink>)?;
        Ok(Self {
            capture,
            sink,
            remote,
        })
    }

    /// Runs a closure against the remote machine.
    ///
    /// Returns `None` if the mutex was poisoned by a panic on another thread.
    /// Poisoning is reported rather than propagated as a panic: this type is test
    /// scaffolding, and a second panic would bury the first one's message.
    pub fn with_remote<T>(&self, f: impl FnOnce(&VirtualInjector) -> T) -> Option<T> {
        let guard = self.remote.lock().ok()?;
        Some(f(&guard))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::keys;
    use crate::modifiers::MouseButton;

    fn press(key: crate::KeyCode) -> InputEvent {
        InputEvent::Key {
            key,
            pressed: true,
            repeat: false,
        }
    }

    #[test]
    fn capture_refuses_to_emit_before_it_starts() {
        let mut capture = VirtualCapture::new();
        assert!(matches!(
            capture.emit(&InputEvent::Leave),
            Err(InputError::NotRunning)
        ));
    }

    #[test]
    fn a_denied_permission_prevents_starting() {
        let mut capture = VirtualCapture::denying_permission("Accessibility");
        let remote = Arc::new(Mutex::new(VirtualInjector::new()));
        let sink = Arc::new(RoutingSink::new(remote));
        let err = capture
            .start(sink as Arc<dyn InputSink>)
            .expect_err("must not start");
        assert!(err.needs_user_action());
        assert!(!capture.is_running());
    }

    #[test]
    fn local_input_passes_through_and_never_reaches_the_remote() {
        let mut lb = Loopback::start().expect("starts");
        let d = lb.capture.emit(&press(keys::A)).expect("emitted");
        assert_eq!(d, Disposition::PassThrough);
        lb.with_remote(|r| assert!(r.events().is_empty(), "leaked to the remote"))
            .expect("remote available");
        assert_eq!(lb.capture.stats().passed_through, 1);
    }

    #[test]
    fn remote_input_is_suppressed_locally_and_injected_remotely() {
        let mut lb = Loopback::start().expect("starts");
        lb.sink.set_destination(Destination::Remote);

        let d = lb.capture.emit(&press(keys::A)).expect("emitted");
        assert_eq!(d, Disposition::Suppress, "would type on the wrong computer");
        lb.with_remote(|r| {
            assert_eq!(r.events().len(), 1);
            assert_eq!(r.tracker().keys(), &[keys::A]);
        })
        .expect("remote available");
    }

    #[test]
    fn a_full_crossing_leaves_nothing_held_on_either_side() {
        // The end-to-end statement of the requirement, exercised headlessly.
        let mut lb = Loopback::start().expect("starts");

        // Drag begins locally, then the pointer crosses to the other machine.
        lb.capture.emit(&press(keys::LEFT_SHIFT)).expect("emitted");
        lb.sink.set_destination(Destination::Remote);
        lb.capture
            .emit(&InputEvent::Enter {
                screen: mb_types::ScreenId(0),
                position: (0.0, 0.4),
                held: crate::HeldInput {
                    keys: vec![],
                    modifiers: crate::Modifiers::LEFT_SHIFT,
                    buttons: crate::MouseButtons::NONE.with(MouseButton::Left),
                },
            })
            .expect("emitted");

        lb.with_remote(|r| {
            assert!(
                !r.is_clean(),
                "the remote should have adopted the held state"
            );
            assert!(r.tracker().modifiers().shift());
        });

        // Connection dies. The remote must recover on its own.
        {
            let mut remote = lb.remote.lock().expect("lock");
            remote.release_all().expect("release");
        }
        lb.with_remote(|r| {
            assert!(r.is_clean(), "the remote is stuck holding input");
            assert_eq!(r.release_all_calls(), 1);
        })
        .expect("remote available");
    }

    #[test]
    fn cursor_position_accumulates_and_can_be_set_absolutely() {
        let mut injector = VirtualInjector::new();
        injector
            .inject(&InputEvent::MouseMove { dx: 10.0, dy: -4.0 })
            .expect("injected");
        injector
            .inject(&InputEvent::MouseMove { dx: 2.5, dy: 1.0 })
            .expect("injected");
        assert_eq!(injector.cursor(), (12.5, -3.0));

        injector
            .inject(&InputEvent::MouseMoveTo { x: 0.0, y: 0.0 })
            .expect("injected");
        assert_eq!(
            injector.cursor(),
            (0.0, 0.0),
            "absolute must not accumulate"
        );
    }

    #[test]
    fn an_os_disable_is_reported_and_recoverable_by_rearming() {
        let mut lb = Loopback::start().expect("starts");
        lb.capture.simulate_os_disable("timeout");

        let err = lb.capture.emit(&press(keys::A)).expect_err("disabled");
        assert!(matches!(err, InputError::CaptureDisabled { .. }));
        assert!(err.is_recoverable());
        assert!(!lb.capture.is_running(), "must not claim to be running");

        lb.capture.rearm().expect("re-arms");
        assert!(lb.capture.is_running());
        lb.capture.emit(&press(keys::A)).expect("works again");
    }

    #[test]
    fn stopping_capture_stops_delivery() {
        let mut lb = Loopback::start().expect("starts");
        lb.capture.stop().expect("stops");
        assert!(!lb.capture.is_running());
        assert!(lb.capture.emit(&press(keys::A)).is_err());
    }

    #[test]
    fn injection_failure_does_not_change_the_suppression_decision() {
        // If the remote injection fails, passing the event through locally would
        // type into the wrong computer. Suppression must be unconditional.
        let mut lb = Loopback::start().expect("starts");
        lb.sink.set_destination(Destination::Remote);
        lb.remote
            .lock()
            .expect("lock")
            .fail_next(InputError::OsCall {
                api: "test",
                detail: "boom".to_owned(),
            });

        let d = lb.capture.emit(&press(keys::A)).expect("emitted");
        assert_eq!(d, Disposition::Suppress);
    }
}
