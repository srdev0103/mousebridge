//! A running connection: pumping messages, driving heartbeats, and shutting down
//! without leaving input stuck.
//!
//! # Split send paths, on purpose
//!
//! Pointer motion is sent **synchronously and without awaiting**, straight onto a
//! QUIC datagram. That is what lets the input path call it: `send_motion` cannot
//! block, cannot allocate a task, and cannot apply backpressure. If the send
//! buffer is full the packet is dropped, which is correct — the next position
//! supersedes it.
//!
//! Everything else goes through a bounded channel to the writer task, because it
//! must not be dropped. A lost key-up is a key held down on a machine the user
//! has walked away from.
//!
//! # Shutdown always releases
//!
//! However a session ends — clean disconnect, timeout, transport failure, or the
//! peer vanishing — the final event is always [`SessionEvent::Closed`]. The
//! receiving side treats that as an unconditional instruction to release
//! everything it is holding. That is the entire reason this type owns the
//! lifecycle rather than leaving it to the caller.

use crate::error::NetError;
use crate::heartbeat::{HeartbeatMonitor, Liveness};
use mb_protocol::frame::{FrameDecode, decode_frame};
use mb_protocol::message::{self, ControlMessage, DisconnectReason, InputMessage, SessionMessage};
use mb_protocol::motion::Motion;
use mb_types::DeviceId;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Outbound queue depth for reliable messages.
///
/// Deep enough to absorb a burst of key events while the writer is busy, shallow
/// enough that a wedged peer is noticed in milliseconds rather than after the
/// process has buffered megabytes of keystrokes nobody will ever receive.
const OUTBOUND_CAPACITY: usize = 256;

/// Inbound event queue depth.
const INBOUND_CAPACITY: usize = 1024;

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosedReason {
    /// The peer said goodbye.
    PeerDisconnected(DisconnectReason),
    /// The peer stopped answering heartbeats.
    Unresponsive,
    /// The transport failed.
    TransportFailed(String),
    /// The peer violated the protocol.
    ProtocolViolation(String),
    /// This side closed the session.
    LocalShutdown,
}

/// Something that happened on a session.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Pointer motion arrived.
    Motion(Motion),
    /// An input event arrived.
    Input(InputMessage),
    /// A control message arrived.
    Control(ControlMessage),
    /// Heartbeats are going unanswered, but the session is still usable.
    Degraded {
        /// Consecutive missed probes.
        missed: u32,
    },
    /// Heartbeats are being answered again.
    Recovered,
    /// The session has ended.
    ///
    /// Always the last event, whatever the cause. The receiver **must** release
    /// all held input on seeing it.
    Closed(ClosedReason),
}

/// Something to send.
#[derive(Debug)]
enum Outbound {
    Message(SessionMessage),
    Shutdown(DisconnectReason),
}

/// The sending half of a session.
///
/// Cheap to clone, and safe to hold on the input path.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    connection: quinn::Connection,
    outbound: mpsc::Sender<Outbound>,
    device: DeviceId,
}

impl SessionHandle {
    /// The peer's verified identity.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.device
    }

    /// Sends pointer motion, without awaiting.
    ///
    /// Safe to call from the input path: it never blocks and never allocates a
    /// task. A full send buffer drops the packet, which is the right outcome —
    /// the next position supersedes this one, and blocking the capture thread to
    /// deliver a stale coordinate would be worse than losing it.
    ///
    /// # Errors
    ///
    /// [`NetError::ConnectionLost`] if the connection is gone. A full buffer is
    /// *not* an error; it returns `Ok`.
    pub fn send_motion(&self, motion: Motion) -> Result<(), NetError> {
        let bytes = bytes::Bytes::copy_from_slice(&motion.to_bytes());
        match self.connection.send_datagram(bytes) {
            Ok(()) => Ok(()),
            Err(quinn::SendDatagramError::ConnectionLost(e)) => Err(NetError::ConnectionLost {
                detail: e.to_string(),
            }),
            // Too large, unsupported, or the buffer is full. The first two are
            // impossible for a sixteen-byte packet on a negotiated connection;
            // the third is ordinary congestion.
            Err(_) => Ok(()),
        }
    }

    /// Queues an input event for reliable delivery.
    ///
    /// # Errors
    ///
    /// [`NetError::ConnectionLost`] if the session has ended or the outbound
    /// queue is full. A full queue is fatal here, unlike for motion: dropping a
    /// key transition would desynchronise the two machines, and the only safe
    /// response is to tear the session down so both sides release.
    pub fn send_input(&self, input: InputMessage) -> Result<(), NetError> {
        self.enqueue(Outbound::Message(SessionMessage::Input(input)))
    }

    /// Queues a control message.
    ///
    /// # Errors
    ///
    /// As [`SessionHandle::send_input`].
    pub fn send_control(&self, control: ControlMessage) -> Result<(), NetError> {
        self.enqueue(Outbound::Message(SessionMessage::Control(control)))
    }

    /// Ends the session, telling the peer why.
    ///
    /// # Errors
    ///
    /// [`NetError::ConnectionLost`] if the session has already ended.
    pub fn shutdown(&self, reason: DisconnectReason) -> Result<(), NetError> {
        self.enqueue(Outbound::Shutdown(reason))
    }

    fn enqueue(&self, item: Outbound) -> Result<(), NetError> {
        self.outbound.try_send(item).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => NetError::ConnectionLost {
                detail: "the peer is not keeping up with reliable input".to_owned(),
            },
            mpsc::error::TrySendError::Closed(_) => NetError::ConnectionLost {
                detail: "the session has ended".to_owned(),
            },
        })
    }
}

/// A session that has not yet been driven.
pub struct Session {
    connection: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    outbound: mpsc::Receiver<Outbound>,
    events: mpsc::Sender<SessionEvent>,
    heartbeat: HeartbeatMonitor,
    device: DeviceId,
}

impl Session {
    /// Splits a connected peer into a handle, an event stream, and the session
    /// to drive.
    ///
    /// The caller spawns [`Session::run`]. Keeping that explicit rather than
    /// spawning internally means the session's lifetime is the caller's to
    /// manage, which matters when it has to be torn down on sleep or lock.
    ///
    /// # The handle and the event stream are both keep-alives
    ///
    /// Dropping **either** ends the session. That is deliberate — a session no
    /// part of the application can reach or hear from should not keep a
    /// connection open — but it is easy to trip over when a peer is only being
    /// listened to and never sent to. Hold the handle for as long as the session
    /// should live, even if nothing is ever sent through it.
    #[must_use]
    pub fn split(
        peer: crate::endpoint::PeerConnection,
    ) -> (SessionHandle, mpsc::Receiver<SessionEvent>, Self) {
        let device = peer.device();
        let (connection, send, recv) = peer.into_parts();

        let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (events_tx, events_rx) = mpsc::channel(INBOUND_CAPACITY);

        let handle = SessionHandle {
            connection: connection.clone(),
            outbound: outbound_tx,
            device,
        };

        let session = Self {
            connection,
            send,
            recv,
            outbound: outbound_rx,
            events: events_tx,
            heartbeat: HeartbeatMonitor::default(),
            device,
        };

        (handle, events_rx, session)
    }

    /// Drives the session until it ends.
    ///
    /// Always emits exactly one [`SessionEvent::Closed`] before returning, so a
    /// receiver can rely on it to release held input.
    pub async fn run(mut self) {
        let reason = self.pump().await;

        match &reason {
            ClosedReason::LocalShutdown | ClosedReason::PeerDisconnected(_) => {
                info!(peer = %self.device, ?reason, "session ended");
            }
            _ => warn!(peer = %self.device, ?reason, "session ended unexpectedly"),
        }

        // Best effort: the receiver may already be gone, which is fine. What
        // matters is that a receiver still listening always gets this.
        let _ = self.events.send(SessionEvent::Closed(reason)).await;
        self.connection.close(0u32.into(), b"session ended");
    }

    async fn pump(&mut self) -> ClosedReason {
        let mut ticker = tokio::time::interval(self.heartbeat.interval());
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut inbound = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        let mut probe_sent_at: Option<(u32, Instant)> = None;
        let mut was_degraded = false;

        loop {
            tokio::select! {
                // Reliable stream: control and input.
                read = self.recv.read(&mut chunk) => {
                    match read {
                        Ok(Some(n)) => {
                            inbound.extend_from_slice(&chunk[..n]);
                            match self.drain(&mut inbound, &mut probe_sent_at).await {
                                Ok(Some(reason)) => return reason,
                                Ok(None) => {}
                                Err(reason) => return reason,
                            }
                        }
                        Ok(None) => {
                            return ClosedReason::TransportFailed(
                                "peer closed the session stream".to_owned(),
                            )
                        }
                        Err(e) => return ClosedReason::TransportFailed(e.to_string()),
                    }
                }

                // Unreliable datagrams: pointer motion.
                datagram = self.connection.read_datagram() => {
                    match datagram {
                        Ok(bytes) => match Motion::decode(&bytes) {
                            Ok(motion) => {
                                if self.events.send(SessionEvent::Motion(motion)).await.is_err() {
                                    return ClosedReason::LocalShutdown;
                                }
                            }
                            Err(e) => {
                                // A malformed datagram is dropped rather than
                                // fatal: it costs one frame of motion, and
                                // tearing down a working session over a corrupt
                                // packet would be a worse outcome.
                                debug!(peer = %self.device, error = %e, "discarding a malformed datagram");
                            }
                        },
                        Err(e) => return ClosedReason::TransportFailed(e.to_string()),
                    }
                }

                // Outbound reliable messages.
                item = self.outbound.recv() => {
                    match item {
                        Some(Outbound::Message(message)) => {
                            if let Err(e) = self.write(&message).await {
                                return ClosedReason::TransportFailed(e.to_string());
                            }
                        }
                        Some(Outbound::Shutdown(reason)) => {
                            let goodbye = SessionMessage::Control(
                                ControlMessage::Disconnect { reason },
                            );
                            // Deliberately ignoring the result: we are leaving
                            // either way, and a failure to deliver the courtesy
                            // notice must not stop us closing cleanly.
                            let _ = self.write(&goodbye).await;
                            let _ = self.send.finish();

                            // Wait for the peer to act on it before tearing the
                            // connection down. See GOODBYE_GRACE: without this
                            // the close overtakes the message and the reason is
                            // lost every time.
                            let _ = tokio::time::timeout(
                                GOODBYE_GRACE,
                                self.connection.closed(),
                            )
                            .await;
                            return ClosedReason::LocalShutdown;
                        }
                        None => return ClosedReason::LocalShutdown,
                    }
                }

                // Heartbeat.
                _ = ticker.tick() => {
                    let seq = self.heartbeat.next_probe();

                    match self.heartbeat.liveness() {
                        Liveness::Dead => return ClosedReason::Unresponsive,
                        Liveness::Degraded { missed } => {
                            if !was_degraded {
                                was_degraded = true;
                                let _ = self.events.send(SessionEvent::Degraded { missed }).await;
                            }
                        }
                        Liveness::Healthy => {
                            if was_degraded {
                                was_degraded = false;
                                let _ = self.events.send(SessionEvent::Recovered).await;
                            }
                        }
                    }

                    let now = Instant::now();
                    probe_sent_at = Some((seq, now));
                    let probe = SessionMessage::Control(ControlMessage::Heartbeat {
                        seq,
                        sent_us: micros_since_epochish(now),
                    });
                    if let Err(e) = self.write(&probe).await {
                        return ClosedReason::TransportFailed(e.to_string());
                    }
                }
            }
        }
    }

    /// Decodes as many complete frames as the buffer holds.
    ///
    /// Returns `Ok(Some(reason))` when the peer asked to disconnect.
    async fn drain(
        &mut self,
        buffer: &mut Vec<u8>,
        probe_sent_at: &mut Option<(u32, Instant)>,
    ) -> Result<Option<ClosedReason>, ClosedReason> {
        loop {
            let (message, consumed) = match decode_frame(buffer) {
                Ok(FrameDecode::Complete { payload, consumed }) => {
                    let decoded: SessionMessage = message::decode(payload, "session")
                        .map_err(|e| ClosedReason::ProtocolViolation(e.to_string()))?;
                    decoded
                        .validate()
                        .map_err(|e| ClosedReason::ProtocolViolation(e.to_string()))?;
                    (decoded, consumed)
                }
                Ok(FrameDecode::Incomplete { .. }) => return Ok(None),
                // An oversized length prefix cannot be resynchronised from:
                // there is no way to know where the claimed frame would end.
                Err(e) => return Err(ClosedReason::ProtocolViolation(e.to_string())),
            };
            buffer.drain(..consumed);

            match message {
                SessionMessage::Control(ControlMessage::Heartbeat { seq, sent_us }) => {
                    let reply =
                        SessionMessage::Control(ControlMessage::HeartbeatAck { seq, sent_us });
                    self.write(&reply)
                        .await
                        .map_err(|e| ClosedReason::TransportFailed(e.to_string()))?;
                }
                SessionMessage::Control(ControlMessage::HeartbeatAck { seq, .. }) => {
                    if let Some((sent_seq, at)) = *probe_sent_at
                        && sent_seq == seq
                    {
                        self.heartbeat.on_reply(seq, at.elapsed());
                        *probe_sent_at = None;
                    }
                }
                SessionMessage::Control(ControlMessage::Disconnect { reason }) => {
                    return Ok(Some(ClosedReason::PeerDisconnected(reason)));
                }
                SessionMessage::Control(control) => {
                    if self
                        .events
                        .send(SessionEvent::Control(control))
                        .await
                        .is_err()
                    {
                        return Ok(Some(ClosedReason::LocalShutdown));
                    }
                }
                SessionMessage::Input(input) => {
                    if self.events.send(SessionEvent::Input(input)).await.is_err() {
                        return Ok(Some(ClosedReason::LocalShutdown));
                    }
                }
            }
        }
    }

    async fn write(&mut self, message: &SessionMessage) -> Result<(), NetError> {
        let mut buffer = Vec::with_capacity(64);
        message::encode(message, "session", &mut buffer)?;
        self.send
            .write_all(&buffer)
            .await
            .map_err(|e| NetError::Stream {
                operation: "write session stream",
                detail: e.to_string(),
            })
    }
}

/// A monotonic microsecond stamp for latency measurement.
///
/// The two machines' clocks are unrelated, so this is only ever compared with
/// itself after a round trip. It is deliberately not a wall clock: a clock
/// adjustment mid-session would otherwise produce negative latencies.
fn micros_since_epochish(now: Instant) -> u64 {
    // `Instant` has no absolute value, so measure from a process-lifetime base.
    use std::sync::OnceLock;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = *BASE.get_or_init(Instant::now);
    now.saturating_duration_since(base).as_micros() as u64
}

/// How long to let a goodbye reach the peer before tearing the connection down.
///
/// Closing a QUIC connection sends `CONNECTION_CLOSE` immediately, which
/// overtakes anything still buffered on a stream. Without this wait, a clean
/// shutdown arrived at the peer as a transport failure **every time**, and the
/// `DisconnectReason` — the entire mechanism for distinguishing "the user quit"
/// from "the cable was pulled" — was never once delivered.
///
/// The wait is bounded and normally instant: the peer closes as soon as it reads
/// the goodbye, which resolves it. The timeout only matters if the peer has
/// already gone, in which case there is nobody left to tell.
const GOODBYE_GRACE: Duration = Duration::from_secs(2);
