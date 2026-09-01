//! Deciding where captured input goes, and applying what arrives.
//!
//! This is the join between the two halves of the product: the input layer, which
//! knows how to capture and inject, and the transport, which knows how to reach
//! another machine.
//!
//! # The capture thread must not block
//!
//! [`RouterSink::on_event`] runs on the operating system's input thread — a
//! CoreGraphics tap callback or a Win32 hook procedure. Both platforms disable
//! capture if that call is slow. The sink therefore does exactly three things:
//! read an atomic, push into a bounded queue, and return. It never awaits, never
//! locks a contended mutex, and never touches the network.
//!
//! Everything else happens on [`ForwardTask`], which owns the connection.
//!
//! # What happens when the queue is full
//!
//! Motion is dropped, because the next position supersedes it. A key or button
//! transition is **not** dropped: losing one desynchronises the two machines and
//! leaves input held. When the queue cannot take a transition, the session is
//! torn down instead — both sides then release, which is a bad outcome but a
//! recoverable one, unlike a silently stuck modifier.

use mb_input::capture::{Disposition, InputSink};
use mb_input::{InputEvent, InputStateTracker};
use mb_net::session::SessionHandle;
use mb_protocol::message::InputMessage;
use mb_protocol::motion::Motion;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Queue depth between the capture thread and the forwarding task.
///
/// Sized to absorb a burst of motion at a high polling rate while a slow network
/// send completes. Beyond this, motion is dropped rather than queued: delivering
/// a coordinate from half a second ago is worse than skipping it.
const QUEUE_CAPACITY: usize = 512;

/// Where captured input is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// This machine. Events pass through untouched.
    Local,
    /// A connected peer. Events are suppressed here and forwarded.
    Remote,
}

/// Counters describing what the router has done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouterStats {
    /// Motion events dropped because the queue was full.
    pub dropped_motion: u64,
    /// Events forwarded to a peer.
    pub forwarded: u64,
    /// Times a state transition could not be queued.
    ///
    /// Any value above zero means a session was torn down to avoid leaving input
    /// held, and is worth surfacing in diagnostics.
    pub lost_transitions: u64,
}

/// State shared between the capture thread and the forwarding task.
#[derive(Debug)]
struct Shared {
    /// True when input should be forwarded rather than delivered locally.
    remote: AtomicBool,
    dropped_motion: AtomicU64,
    forwarded: AtomicU64,
    lost_transitions: AtomicU64,
}

/// The capture-side half of the router.
///
/// Cloneable and safe to hand to a platform capture backend.
#[derive(Debug, Clone)]
pub struct Router {
    shared: Arc<Shared>,
    queue: mpsc::Sender<InputEvent>,
}

impl Router {
    /// Builds a router and the task that drains it.
    #[must_use]
    pub fn new() -> (Self, mpsc::Receiver<InputEvent>) {
        let (queue, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let shared = Arc::new(Shared {
            remote: AtomicBool::new(false),
            dropped_motion: AtomicU64::new(0),
            forwarded: AtomicU64::new(0),
            lost_transitions: AtomicU64::new(0),
        });
        (Self { shared, queue }, receiver)
    }

    /// Sets where input goes.
    pub fn set_destination(&self, destination: Destination) {
        self.shared
            .remote
            .store(destination == Destination::Remote, Ordering::Release);
    }

    /// Where input is currently going.
    #[must_use]
    pub fn destination(&self) -> Destination {
        if self.shared.remote.load(Ordering::Acquire) {
            Destination::Remote
        } else {
            Destination::Local
        }
    }

    /// Counters for diagnostics.
    #[must_use]
    pub fn stats(&self) -> RouterStats {
        RouterStats {
            dropped_motion: self.shared.dropped_motion.load(Ordering::Relaxed),
            forwarded: self.shared.forwarded.load(Ordering::Relaxed),
            lost_transitions: self.shared.lost_transitions.load(Ordering::Relaxed),
        }
    }

    /// Returns a read-only view of the router's counters.
    ///
    /// Deliberately not the [`Router`] itself. The router owns the *sending* half
    /// of the input queue, so a forwarding task holding one would keep its own
    /// input channel alive forever and never observe the capture side going
    /// away — the connection would outlive the thing feeding it.
    #[must_use]
    pub fn monitor(&self) -> RouterMonitor {
        RouterMonitor {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Returns this router as a capture sink.
    #[must_use]
    pub fn as_sink(&self) -> Arc<dyn InputSink> {
        Arc::new(RouterSink {
            shared: Arc::clone(&self.shared),
            queue: self.queue.clone(),
        })
    }
}

/// A read-only view of a router's counters.
///
/// Carries no ability to enqueue, so holding one cannot keep a queue alive.
#[derive(Debug, Clone)]
pub struct RouterMonitor {
    shared: Arc<Shared>,
}

impl RouterMonitor {
    /// Counters for diagnostics.
    #[must_use]
    pub fn stats(&self) -> RouterStats {
        RouterStats {
            dropped_motion: self.shared.dropped_motion.load(Ordering::Relaxed),
            forwarded: self.shared.forwarded.load(Ordering::Relaxed),
            lost_transitions: self.shared.lost_transitions.load(Ordering::Relaxed),
        }
    }
}

/// The object handed to a platform capture backend.
#[derive(Debug)]
struct RouterSink {
    shared: Arc<Shared>,
    queue: mpsc::Sender<InputEvent>,
}

impl InputSink for RouterSink {
    fn on_event(&self, event: &InputEvent) -> Disposition {
        if !self.shared.remote.load(Ordering::Acquire) {
            return Disposition::PassThrough;
        }

        match self.queue.try_send(event.clone()) {
            Ok(()) => {
                self.shared.forwarded.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                if dropped.is_droppable() {
                    self.shared.dropped_motion.fetch_add(1, Ordering::Relaxed);
                } else {
                    // A lost transition desynchronises the two machines. The
                    // forwarding task notices this counter and tears the session
                    // down, so both sides release rather than one being left
                    // holding a key nobody will ever lift.
                    self.shared.lost_transitions.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // The forwarding task is gone. Suppressing anyway is deliberate:
                // passing input through locally at this moment would type into
                // whatever happens to be focused here.
            }
        }

        Disposition::Suppress
    }
}

/// Converts a captured event into something the wire can carry.
///
/// Motion is handled separately by the caller, because it needs the accumulated
/// position rather than the delta.
#[must_use]
pub fn to_input_message(event: &InputEvent) -> Option<InputMessage> {
    match event {
        InputEvent::MouseButton { button, pressed } => Some(InputMessage::Button {
            button: *button,
            pressed: *pressed,
        }),
        InputEvent::MouseWheel { delta } => Some(InputMessage::Wheel { delta: *delta }),
        InputEvent::Key {
            key,
            pressed,
            repeat,
        } => Some(InputMessage::Key {
            key: *key,
            pressed: *pressed,
            repeat: *repeat,
        }),
        InputEvent::ReleaseAll | InputEvent::Leave => Some(InputMessage::ReleaseAll),
        // Motion is a datagram; Enter is a control message.
        InputEvent::MouseMove { .. }
        | InputEvent::MouseMoveTo { .. }
        | InputEvent::Enter { .. } => None,
    }
}

/// Converts a received input message back into an event for injection.
#[must_use]
pub fn to_input_event(message: &InputMessage) -> InputEvent {
    match message {
        InputMessage::Button { button, pressed } => InputEvent::MouseButton {
            button: *button,
            pressed: *pressed,
        },
        InputMessage::Wheel { delta } => InputEvent::MouseWheel { delta: *delta },
        InputMessage::Key {
            key,
            pressed,
            repeat,
        } => InputEvent::Key {
            key: *key,
            pressed: *pressed,
            repeat: *repeat,
        },
        InputMessage::ReleaseAll => InputEvent::ReleaseAll,
    }
}

/// Accumulates relative motion into a position for the target machine.
///
/// The wire carries an absolute position rather than a delta, so a dropped
/// datagram costs one frame instead of permanently desynchronising the two
/// cursors. See `mb_protocol::motion`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MotionAccumulator {
    x: f64,
    y: f64,
    seq: u16,
}

impl MotionAccumulator {
    /// Starts accumulating from a position.
    #[must_use]
    pub const fn starting_at(x: f64, y: f64) -> Self {
        Self { x, y, seq: 0 }
    }

    /// Applies a delta and returns the datagram to send.
    pub fn accumulate(&mut self, dx: f64, dy: f64, timestamp_us: u32) -> Motion {
        self.x += dx;
        self.y += dy;
        self.seq = self.seq.wrapping_add(1);
        Motion {
            seq: self.seq,
            timestamp_us,
            // Rounded at the boundary, once. Accumulating in floating point and
            // rounding only on send keeps sub-pixel trackpad movement from being
            // discarded a fraction at a time until slow motion stops entirely.
            x: clamp_to_i32(self.x),
            y: clamp_to_i32(self.y),
        }
    }

    /// Repositions without emitting anything, after a handoff.
    pub const fn reset(&mut self, x: f64, y: f64) {
        self.x = x;
        self.y = y;
    }

    /// The accumulated position.
    #[must_use]
    pub const fn position(&self) -> (f64, f64) {
        (self.x, self.y)
    }
}

/// Rounds and clamps, so a non-finite or absurd accumulation cannot reach the wire.
fn clamp_to_i32(value: f64) -> i32 {
    if value.is_nan() {
        return 0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped into range immediately before the cast"
    )]
    {
        value
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

/// Forwards queued events to a connected peer.
///
/// Returns when the queue closes or the session fails.
pub async fn forward(
    mut events: mpsc::Receiver<InputEvent>,
    session: SessionHandle,
    monitor: RouterMonitor,
) {
    let mut motion = MotionAccumulator::default();
    let start = std::time::Instant::now();

    while let Some(event) = events.recv().await {
        // A transition that never made it into the queue means the two machines
        // no longer agree about what is held. Tearing the session down makes both
        // sides release, which is recoverable; carrying on is not. One is enough
        // to act on, so there is no counter to compare against.
        let lost = monitor.stats().lost_transitions;
        if lost > 0 {
            warn!(
                lost,
                "dropped an input transition; ending the session so both sides release"
            );
            let _ = session.shutdown(mb_protocol::message::DisconnectReason::ProtocolError);
            return;
        }

        let result = match &event {
            InputEvent::MouseMove { dx, dy } => {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "wraps every 71 minutes, which is immaterial for a single hop"
                )]
                let stamp = start.elapsed().as_micros() as u32;
                session.send_motion(motion.accumulate(*dx, *dy, stamp))
            }
            other => match to_input_message(other) {
                Some(message) => session.send_input(message),
                None => Ok(()),
            },
        };

        if let Err(e) = result {
            debug!(error = %e, "forwarding stopped");
            return;
        }
    }
}

/// Forwards queued input to whichever peer is currently receiving it.
///
/// Unlike [`forward`], the destination can change mid-stream: the user crosses a
/// screen edge and a different machine starts receiving. Looking the session up
/// per event rather than capturing one at the start is what makes that possible.
///
/// Returns when the queue closes.
pub async fn forward_dynamic(
    mut events: mpsc::Receiver<InputEvent>,
    sessions: Arc<dyn crate::engine::ActiveSession>,
    monitor: RouterMonitor,
) {
    let start = std::time::Instant::now();
    let mut sequence: u16 = 0;
    let mut warned = false;

    while let Some(event) = events.recv().await {
        if monitor.stats().lost_transitions > 0 && !warned {
            warned = true;
            warn!("dropped an input transition; the peer is not keeping up");
        }

        // No active peer means the cursor is on this machine, and the router
        // should not have queued anything. Dropping is the safe response.
        let Some(session) = sessions.active() else {
            continue;
        };

        let result = match &event {
            InputEvent::MouseMove { .. } => {
                // The topology already applied this delta and knows where the
                // cursor now sits on the target screen. Sending that position is
                // the whole point: accumulating deltas from an arbitrary origin
                // produces coordinates unrelated to the receiver's screen, and a
                // pointer that goes nowhere useful.
                let Some((nx, ny)) = sessions.cursor_normalized() else {
                    continue;
                };
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "wraps every 71 minutes, immaterial for one hop"
                )]
                let stamp = start.elapsed().as_micros() as u32;
                sequence = sequence.wrapping_add(1);
                session.send_motion(Motion::from_normalized(sequence, stamp, nx, ny))
            }
            other => match to_input_message(other) {
                Some(message) => session.send_input(message),
                None => Ok(()),
            },
        };

        if let Err(e) = result {
            debug!(error = %e, "forwarding to the active peer failed");
        }
    }
}

/// Applies received input to a machine, keeping track of what is held.
///
/// The tracker is the recovery mechanism: [`ReceiveState::release_all`] produces
/// exactly the events needed to let go of everything, and is called whenever a
/// session ends for any reason.
#[derive(Debug, Default)]
pub struct ReceiveState {
    tracker: InputStateTracker,
}

impl ReceiveState {
    /// Builds an empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an event that is about to be injected.
    pub fn observe(&mut self, event: &InputEvent) {
        self.tracker.apply(event);
    }

    /// True when nothing is held.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.tracker.is_clean()
    }

    /// The events needed to release everything, clearing the tracker.
    ///
    /// Called on **every** session ending, however it ended. A clean disconnect,
    /// a timeout and a vanished peer are indistinguishable from the perspective
    /// of a key that is still down.
    #[must_use]
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let events = self.tracker.release_events();
        self.tracker.clear();
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_input::MouseButton;
    use mb_input::keycode::keys;

    fn key(pressed: bool) -> InputEvent {
        InputEvent::Key {
            key: keys::A,
            pressed,
            repeat: false,
        }
    }

    #[test]
    fn local_input_passes_through_and_is_not_queued() {
        let (router, mut queue) = Router::new();
        let sink = router.as_sink();

        assert_eq!(sink.on_event(&key(true)), Disposition::PassThrough);
        assert!(queue.try_recv().is_err(), "local input reached the queue");
    }

    #[test]
    fn remote_input_is_suppressed_and_queued() {
        let (router, mut queue) = Router::new();
        router.set_destination(Destination::Remote);
        let sink = router.as_sink();

        assert_eq!(sink.on_event(&key(true)), Disposition::Suppress);
        assert_eq!(queue.try_recv().expect("queued"), key(true));
    }

    #[test]
    fn switching_destination_takes_effect_immediately() {
        let (router, _queue) = Router::new();
        let sink = router.as_sink();

        assert_eq!(router.destination(), Destination::Local);
        router.set_destination(Destination::Remote);
        assert_eq!(router.destination(), Destination::Remote);
        assert_eq!(sink.on_event(&key(true)), Disposition::Suppress);

        router.set_destination(Destination::Local);
        assert_eq!(sink.on_event(&key(false)), Disposition::PassThrough);
    }

    #[test]
    fn a_full_queue_drops_motion_but_counts_lost_transitions() {
        let (router, _queue) = Router::new();
        router.set_destination(Destination::Remote);
        let sink = router.as_sink();

        // Fill the queue.
        for _ in 0..QUEUE_CAPACITY {
            sink.on_event(&InputEvent::MouseMove { dx: 1.0, dy: 0.0 });
        }
        assert_eq!(router.stats().dropped_motion, 0, "nothing dropped yet");

        // Motion beyond capacity is dropped, which is correct.
        sink.on_event(&InputEvent::MouseMove { dx: 1.0, dy: 0.0 });
        assert_eq!(router.stats().dropped_motion, 1);
        assert_eq!(router.stats().lost_transitions, 0);

        // A key transition is not droppable, and must be counted separately so
        // the session can be torn down rather than silently desynchronised.
        sink.on_event(&key(true));
        assert_eq!(router.stats().lost_transitions, 1);
    }

    #[test]
    fn input_is_suppressed_even_when_the_queue_is_gone() {
        // Passing through here would type into whatever is focused on *this*
        // machine, which is the one the user is not looking at.
        let (router, queue) = Router::new();
        router.set_destination(Destination::Remote);
        let sink = router.as_sink();
        drop(queue);

        assert_eq!(sink.on_event(&key(true)), Disposition::Suppress);
    }

    #[test]
    fn events_and_messages_round_trip() {
        let events = vec![
            InputEvent::MouseButton {
                button: MouseButton::Middle,
                pressed: true,
            },
            InputEvent::MouseWheel {
                delta: mb_input::ScrollDelta::lines(0.0, 3.0),
            },
            key(true),
            InputEvent::ReleaseAll,
        ];
        for event in events {
            let message = to_input_message(&event).expect("convertible");
            assert_eq!(to_input_event(&message), event);
        }
    }

    #[test]
    fn leave_is_carried_as_a_release() {
        // The receiving machine has no concept of the sender's topology; what it
        // needs to know is that it should let go of everything.
        assert_eq!(
            to_input_message(&InputEvent::Leave),
            Some(InputMessage::ReleaseAll)
        );
    }

    #[test]
    fn motion_accumulates_rather_than_truncating_each_delta() {
        // The bug this prevents: rounding every delta discards sub-pixel
        // trackpad movement a fraction at a time until slow motion stops moving
        // the cursor at all.
        let mut accumulator = MotionAccumulator::default();
        for _ in 0..10 {
            accumulator.accumulate(0.4, 0.0, 0);
        }
        let (x, _) = accumulator.position();
        assert!((x - 4.0).abs() < 1e-9, "accumulated to {x}");

        let motion = accumulator.accumulate(0.0, 0.0, 0);
        assert_eq!(motion.x, 4);
    }

    #[test]
    fn motion_sequence_numbers_advance_and_wrap() {
        let mut accumulator = MotionAccumulator::default();
        assert_eq!(accumulator.accumulate(1.0, 0.0, 0).seq, 1);
        assert_eq!(accumulator.accumulate(1.0, 0.0, 0).seq, 2);

        accumulator.seq = u16::MAX;
        assert_eq!(accumulator.accumulate(1.0, 0.0, 0).seq, 0, "must wrap");
    }

    #[test]
    fn absurd_accumulation_cannot_reach_the_wire() {
        // Coordinates come from deltas that ultimately come from hardware, and a
        // faulty device or a hostile peer should not produce a NaN position.
        let mut accumulator = MotionAccumulator::default();
        let motion = accumulator.accumulate(f64::NAN, 0.0, 0);
        assert_eq!(motion.x, 0, "NaN reached the wire");

        let mut accumulator = MotionAccumulator::starting_at(f64::MAX, 0.0);
        let motion = accumulator.accumulate(f64::MAX, 0.0, 0);
        assert_eq!(motion.x, i32::MAX, "overflow was not clamped");
    }

    #[test]
    fn a_receiver_can_always_be_returned_to_clean() {
        let mut state = ReceiveState::new();
        state.observe(&InputEvent::Key {
            key: keys::LEFT_CTRL,
            pressed: true,
            repeat: false,
        });
        state.observe(&InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });
        assert!(!state.is_clean());

        let releases = state.release_all();
        assert_eq!(releases.len(), 2);
        assert!(state.is_clean());

        // And doing it again is harmless, which matters because it is called
        // defensively on every session ending.
        assert!(state.release_all().is_empty());
        assert!(state.is_clean());
    }
}
