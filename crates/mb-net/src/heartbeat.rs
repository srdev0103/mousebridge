//! Liveness detection.
//!
//! QUIC has its own idle timeout, but it answers a different question: whether
//! the *transport* is alive. This layer answers whether the peer's application
//! is still processing input. A machine that has locked, slept, or wedged can
//! keep a QUIC connection open while doing nothing with what it receives — and
//! the user, whose keystrokes are vanishing into it, deserves to be told.
//!
//! # Why this is a state machine, not a loop with sleeps
//!
//! The decisions here — how many misses before a connection is degraded, when to
//! give up entirely, what the round trip is — are exactly the ones worth testing,
//! and testing them against wall-clock sleeps would make the suite slow and
//! flaky. [`HeartbeatMonitor`] takes the current time as a parameter, so an
//! entire minute of failure modes runs in microseconds.

use std::time::Duration;

/// How often to send a probe.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

/// Misses before the connection is considered degraded.
///
/// Three. Wi-Fi routinely loses a single packet, so reacting to one miss would
/// mean constant spurious warnings; waiting for ten would mean the user stares
/// at a dead cursor. Three is roughly the point where loss stops looking like
/// noise.
pub const DEFAULT_MAX_MISSED: u32 = 3;

/// The monitor's view of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Probes are being answered.
    Healthy,
    /// Probes are going unanswered, but not yet enough to give up.
    ///
    /// The UI should say so. Input is still being forwarded, because dropping a
    /// working connection over one lost packet is worse than a brief warning.
    Degraded {
        /// Consecutive probes with no reply.
        missed: u32,
    },
    /// The peer is not responding and the connection should be torn down.
    ///
    /// The caller **must** release all held input on this transition. A key that
    /// was down when the peer stopped answering is otherwise held forever on a
    /// machine that is no longer listening to us.
    Dead,
}

impl Liveness {
    /// True when input should still be forwarded.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded { .. })
    }
}

/// Tracks probe replies and decides whether the peer is alive.
#[derive(Debug, Clone)]
pub struct HeartbeatMonitor {
    interval: Duration,
    max_missed: u32,
    next_seq: u32,
    /// Sequence number of the probe awaiting a reply, if any.
    outstanding: Option<u32>,
    missed: u32,
    /// Most recent measured round trip.
    rtt: Option<Duration>,
    /// Smoothed round trip, for display.
    smoothed_rtt: Option<Duration>,
}

impl Default for HeartbeatMonitor {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL, DEFAULT_MAX_MISSED)
    }
}

impl HeartbeatMonitor {
    /// Builds a monitor.
    #[must_use]
    pub const fn new(interval: Duration, max_missed: u32) -> Self {
        Self {
            interval,
            max_missed,
            next_seq: 0,
            outstanding: None,
            missed: 0,
            rtt: None,
            smoothed_rtt: None,
        }
    }

    /// How often probes should be sent.
    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// Prepares the next probe, returning its sequence number.
    ///
    /// An unanswered previous probe counts as missed at this point rather than on
    /// a timer: the next probe is due, so the last one has had a full interval to
    /// arrive.
    pub const fn next_probe(&mut self) -> u32 {
        if self.outstanding.is_some() {
            self.missed += 1;
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.outstanding = Some(seq);
        seq
    }

    /// Records a reply.
    ///
    /// Replies to anything other than the outstanding probe are ignored. A late
    /// reply to a probe already counted as missed must not clear the miss
    /// counter, or a peer answering every third probe would look healthy.
    pub fn on_reply(&mut self, seq: u32, round_trip: Duration) {
        if self.outstanding != Some(seq) {
            return;
        }
        self.outstanding = None;
        self.missed = 0;
        self.rtt = Some(round_trip);
        self.smoothed_rtt = Some(match self.smoothed_rtt {
            // Exponentially weighted, 1/8 on the new sample — the same shape TCP
            // uses. A single slow probe should nudge the displayed figure, not
            // redefine it.
            Some(previous) => (previous * 7 + round_trip) / 8,
            None => round_trip,
        });
    }

    /// The monitor's current verdict.
    #[must_use]
    pub const fn liveness(&self) -> Liveness {
        if self.missed >= self.max_missed {
            Liveness::Dead
        } else if self.missed > 0 {
            Liveness::Degraded {
                missed: self.missed,
            }
        } else {
            Liveness::Healthy
        }
    }

    /// The most recent round trip.
    #[must_use]
    pub const fn last_rtt(&self) -> Option<Duration> {
        self.rtt
    }

    /// The smoothed round trip, for display.
    #[must_use]
    pub const fn smoothed_rtt(&self) -> Option<Duration> {
        self.smoothed_rtt
    }

    /// Resets after a reconnection.
    pub const fn reset(&mut self) {
        self.outstanding = None;
        self.missed = 0;
        // Round trip history is deliberately kept: it describes the path, which
        // a reconnection over the same network has not changed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_fresh_monitor_is_healthy() {
        let monitor = HeartbeatMonitor::default();
        assert_eq!(monitor.liveness(), Liveness::Healthy);
        assert!(monitor.last_rtt().is_none());
    }

    #[test]
    fn answered_probes_keep_it_healthy() {
        let mut monitor = HeartbeatMonitor::default();
        for _ in 0..100 {
            let seq = monitor.next_probe();
            monitor.on_reply(seq, ms(2));
            assert_eq!(monitor.liveness(), Liveness::Healthy);
        }
    }

    #[test]
    fn unanswered_probes_degrade_then_die() {
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, 3);

        monitor.next_probe(); // first probe, nothing outstanding before it
        assert_eq!(monitor.liveness(), Liveness::Healthy);

        monitor.next_probe(); // previous went unanswered
        assert_eq!(monitor.liveness(), Liveness::Degraded { missed: 1 });

        monitor.next_probe();
        assert_eq!(monitor.liveness(), Liveness::Degraded { missed: 2 });

        monitor.next_probe();
        assert_eq!(monitor.liveness(), Liveness::Dead);
    }

    #[test]
    fn input_is_still_forwarded_while_degraded() {
        // Dropping a working connection over one lost packet is worse for the
        // user than a brief warning.
        assert!(Liveness::Healthy.is_usable());
        assert!(Liveness::Degraded { missed: 2 }.is_usable());
        assert!(!Liveness::Dead.is_usable());
    }

    #[test]
    fn a_reply_clears_the_miss_counter() {
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, 3);
        monitor.next_probe();
        monitor.next_probe();
        assert_eq!(monitor.liveness(), Liveness::Degraded { missed: 1 });

        let seq = monitor.next_probe();
        monitor.on_reply(seq, ms(5));
        assert_eq!(monitor.liveness(), Liveness::Healthy);
    }

    #[test]
    fn a_stale_reply_does_not_clear_a_miss() {
        // A peer answering only every third probe must not look healthy. The
        // reply that matters is the one to the probe still outstanding.
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, 3);
        let first = monitor.next_probe();
        monitor.next_probe(); // first is now missed
        assert_eq!(monitor.liveness(), Liveness::Degraded { missed: 1 });

        monitor.on_reply(first, ms(900));
        assert_eq!(
            monitor.liveness(),
            Liveness::Degraded { missed: 1 },
            "a late reply to an already-missed probe cleared the counter"
        );
    }

    #[test]
    fn an_unsolicited_reply_is_ignored() {
        let mut monitor = HeartbeatMonitor::default();
        monitor.on_reply(9999, ms(1));
        assert!(monitor.last_rtt().is_none(), "accepted a reply never sent");
    }

    #[test]
    fn round_trip_is_smoothed_not_replaced() {
        let mut monitor = HeartbeatMonitor::default();
        for _ in 0..20 {
            let seq = monitor.next_probe();
            monitor.on_reply(seq, ms(4));
        }
        let steady = monitor.smoothed_rtt().expect("measured");
        assert!((steady.as_millis() as i64 - 4).abs() <= 1, "{steady:?}");

        // One slow probe should nudge the figure, not redefine it.
        let seq = monitor.next_probe();
        monitor.on_reply(seq, ms(400));
        let after = monitor.smoothed_rtt().expect("measured");
        assert!(after < ms(100), "a single outlier dominated: {after:?}");
        assert!(after > steady, "the outlier had no effect at all");
        assert_eq!(
            monitor.last_rtt(),
            Some(ms(400)),
            "raw sample still visible"
        );
    }

    #[test]
    fn reset_recovers_after_a_reconnection() {
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, 3);
        for _ in 0..5 {
            monitor.next_probe();
        }
        assert_eq!(monitor.liveness(), Liveness::Dead);

        monitor.reset();
        assert_eq!(monitor.liveness(), Liveness::Healthy);
    }

    #[test]
    fn sequence_numbers_are_unique_and_wrap_safely() {
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, u32::MAX);
        let first = monitor.next_probe();
        let second = monitor.next_probe();
        assert_ne!(first, second);

        // Force the counter to the top and confirm it wraps rather than panics.
        monitor.next_seq = u32::MAX;
        let last = monitor.next_probe();
        let wrapped = monitor.next_probe();
        assert_eq!(last, u32::MAX);
        assert_eq!(wrapped, 0);
    }

    #[test]
    fn a_max_missed_of_one_dies_on_the_first_miss() {
        // Configurable aggressiveness must actually work at the boundary.
        let mut monitor = HeartbeatMonitor::new(DEFAULT_INTERVAL, 1);
        monitor.next_probe();
        assert_eq!(monitor.liveness(), Liveness::Healthy);
        monitor.next_probe();
        assert_eq!(monitor.liveness(), Liveness::Dead);
    }
}
