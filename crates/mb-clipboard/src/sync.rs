//! Breaking the clipboard synchronisation loop.
//!
//! # Why content identity, and not a flag or a timer
//!
//! The obvious fixes for the loop described in the crate documentation both
//! fail, and it is worth being explicit about why, because both look reasonable:
//!
//! * **A suppression flag** — "ignore the next local change, it is mine". The
//!   platforms do not guarantee that writing the clipboard produces exactly one
//!   change notification. macOS coalesces; Windows can deliver several for one
//!   write. A flag consumed by the first notification leaves the rest to loop,
//!   and a flag left set swallows the user's *next* genuine copy.
//! * **A time window** — "ignore local changes for 200 ms after writing". Too
//!   short on a loaded machine and the loop resumes; too long and a fast typist
//!   copying twice loses the second copy. There is no value that is right, only
//!   values that are wrong in different conditions.
//!
//! Remembering *what* was written has neither problem. However many
//! notifications arrive, and whenever they arrive, a change matching what was
//! just applied is not news. And a genuine new copy never matches, so nothing
//! the user does is ever swallowed.
//!
//! # The one case this deliberately suppresses
//!
//! Copying the identical text twice in a row, when the first copy came from a
//! peer. The second is not forwarded — but forwarding it would be a no-op, since
//! the peer's clipboard already holds exactly that. Nothing is lost.

use crate::content::{ClipboardContent, ClipboardError, ContentHash};

/// What should happen to a clipboard change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// Send it to peers.
    Send,
    /// Apply it to the local clipboard.
    Apply,
    /// Do nothing: this is the echo of something already synchronised.
    Ignore,
    /// Do nothing: the content is unusable.
    Reject(ClipboardError),
}

/// Statistics, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncStats {
    /// Local changes forwarded to peers.
    pub sent: u64,
    /// Remote updates applied locally.
    pub applied: u64,
    /// Changes recognised as echoes and dropped.
    ///
    /// A healthy session shows roughly one of these per applied update. A count
    /// climbing without bound means the loop is not being broken.
    pub echoes_suppressed: u64,
    /// Changes rejected as unusable.
    pub rejected: u64,
}

/// Decides what to do with clipboard changes on one machine.
#[derive(Debug, Default)]
pub struct ClipboardSync {
    /// What was last written to the local clipboard on a peer's behalf.
    ///
    /// The whole loop-breaking mechanism. See the module documentation.
    last_applied: Option<ContentHash>,
    /// What was last sent to peers, so an unchanged clipboard is not re-sent.
    last_sent: Option<ContentHash>,
    stats: SyncStats,
}

impl ClipboardSync {
    /// Builds a synchroniser with no history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counters for diagnostics.
    #[must_use]
    pub const fn stats(&self) -> SyncStats {
        self.stats
    }

    /// Decides what to do about a change to the local clipboard.
    pub fn on_local_change(&mut self, content: &ClipboardContent) -> SyncDecision {
        if let Err(e) = content.validate() {
            self.stats.rejected += 1;
            return SyncDecision::Reject(e);
        }

        let hash = content.hash();

        // The echo of something a peer sent us. Not news.
        if self.last_applied == Some(hash) {
            self.stats.echoes_suppressed += 1;
            return SyncDecision::Ignore;
        }

        // Already sent. Platforms emit spurious change notifications, and
        // re-sending unchanged content wastes a round trip for no effect.
        if self.last_sent == Some(hash) {
            self.stats.echoes_suppressed += 1;
            return SyncDecision::Ignore;
        }

        self.last_sent = Some(hash);
        self.stats.sent += 1;
        SyncDecision::Send
    }

    /// Decides what to do about content arriving from a peer.
    ///
    /// Recording the hash *here*, before the content is written to the clipboard,
    /// is what makes the mechanism work: the local watcher will fire as a
    /// consequence of that write, and by then the synchroniser already knows
    /// what to expect.
    pub fn on_remote_update(&mut self, content: &ClipboardContent) -> SyncDecision {
        if let Err(e) = content.validate() {
            self.stats.rejected += 1;
            return SyncDecision::Reject(e);
        }

        let hash = content.hash();

        // Already holding exactly this. Writing it again would produce a change
        // notification for no reason, and on some platforms would disturb the
        // user's paste history.
        if self.last_applied == Some(hash) || self.last_sent == Some(hash) {
            self.stats.echoes_suppressed += 1;
            return SyncDecision::Ignore;
        }

        self.last_applied = Some(hash);
        self.stats.applied += 1;
        SyncDecision::Apply
    }

    /// Forgets the history.
    ///
    /// Called when a peer disconnects: with nobody to echo to, retaining the
    /// history would suppress the user's next copy of the same content for no
    /// reason.
    pub fn reset(&mut self) {
        self.last_applied = None;
        self.last_sent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> ClipboardContent {
        ClipboardContent::text(value).expect("valid")
    }

    /// Two machines, each with its own synchroniser, wired to each other.
    ///
    /// Runs the loop for real rather than asserting on one side in isolation —
    /// the failure being guarded against is an interaction, not a rule.
    struct Pair {
        a: ClipboardSync,
        b: ClipboardSync,
    }

    impl Pair {
        fn new() -> Self {
            Self {
                a: ClipboardSync::new(),
                b: ClipboardSync::new(),
            }
        }

        /// Copies on A and lets the two machines settle, returning how many
        /// messages crossed the wire in total.
        fn copy_on_a(&mut self, content: &ClipboardContent) -> usize {
            let mut messages = 0;
            let mut pending = Some((true, content.clone()));

            // Bounded so a genuine loop fails the test rather than hanging it.
            for _ in 0..100 {
                let Some((from_a, item)) = pending.take() else {
                    return messages;
                };

                let decision = if from_a {
                    self.a.on_local_change(&item)
                } else {
                    self.b.on_local_change(&item)
                };

                if decision != SyncDecision::Send {
                    return messages;
                }
                messages += 1;

                // The peer applies it, which changes *its* clipboard, which its
                // own watcher will report as a local change.
                let applied = if from_a {
                    self.b.on_remote_update(&item)
                } else {
                    self.a.on_remote_update(&item)
                };

                if applied == SyncDecision::Apply {
                    pending = Some((!from_a, item));
                }
            }
            panic!("clipboard synchronisation did not settle: {messages} messages and counting");
        }
    }

    #[test]
    fn a_copy_crosses_once_and_stops() {
        // The central property. Without loop breaking this never terminates.
        let mut pair = Pair::new();
        let messages = pair.copy_on_a(&text("hello"));
        assert_eq!(messages, 1, "the content crossed {messages} times");
    }

    #[test]
    fn several_copies_each_cross_once() {
        let mut pair = Pair::new();
        for value in ["first", "second", "third"] {
            assert_eq!(pair.copy_on_a(&text(value)), 1, "on {value}");
        }
        assert_eq!(pair.a.stats().sent, 3);
        assert_eq!(pair.b.stats().applied, 3);
    }

    #[test]
    fn copying_in_both_directions_settles() {
        let mut pair = Pair::new();
        assert_eq!(pair.copy_on_a(&text("from a")), 1);

        // Now B copies something of its own.
        let content = text("from b");
        assert_eq!(pair.b.on_local_change(&content), SyncDecision::Send);
        assert_eq!(pair.a.on_remote_update(&content), SyncDecision::Apply);
        // A's watcher fires as a consequence, and must recognise the echo.
        assert_eq!(pair.a.on_local_change(&content), SyncDecision::Ignore);
    }

    #[test]
    fn the_echo_of_an_applied_update_is_suppressed() {
        let mut sync = ClipboardSync::new();
        let content = text("from the peer");

        assert_eq!(sync.on_remote_update(&content), SyncDecision::Apply);
        // Writing the clipboard makes the local watcher fire.
        assert_eq!(sync.on_local_change(&content), SyncDecision::Ignore);
        assert_eq!(sync.stats().echoes_suppressed, 1);
    }

    #[test]
    fn repeated_notifications_for_one_write_are_all_suppressed() {
        // Windows can deliver several change notifications for a single write,
        // and macOS coalesces unpredictably. A suppression flag consumed by the
        // first notification would leave the rest to loop.
        let mut sync = ClipboardSync::new();
        let content = text("written once");
        sync.on_remote_update(&content);

        for _ in 0..10 {
            assert_eq!(sync.on_local_change(&content), SyncDecision::Ignore);
        }
        assert_eq!(sync.stats().sent, 0, "an echo escaped");
    }

    #[test]
    fn a_genuine_new_copy_is_never_swallowed() {
        // The failure mode of a suppression flag left set: the user's next real
        // copy disappears.
        let mut sync = ClipboardSync::new();
        sync.on_remote_update(&text("from the peer"));
        sync.on_local_change(&text("from the peer"));

        assert_eq!(
            sync.on_local_change(&text("something the user copied")),
            SyncDecision::Send
        );
    }

    #[test]
    fn the_same_content_is_not_sent_twice() {
        // Platforms emit spurious change notifications; re-sending unchanged
        // content wastes a round trip for no effect.
        let mut sync = ClipboardSync::new();
        let content = text("unchanged");
        assert_eq!(sync.on_local_change(&content), SyncDecision::Send);
        assert_eq!(sync.on_local_change(&content), SyncDecision::Ignore);
        assert_eq!(sync.stats().sent, 1);
    }

    #[test]
    fn a_peer_echoing_our_own_content_back_is_ignored() {
        // A peer that has not implemented loop breaking, or an older version.
        // This side must not apply its own content back over itself.
        let mut sync = ClipboardSync::new();
        let content = text("ours");
        assert_eq!(sync.on_local_change(&content), SyncDecision::Send);
        assert_eq!(sync.on_remote_update(&content), SyncDecision::Ignore);
    }

    #[test]
    fn images_and_text_are_tracked_independently() {
        let mut sync = ClipboardSync::new();
        let text_content = text("PNG");
        let image = ClipboardContent::image(crate::content::ImageFormat::Png, b"PNG".to_vec())
            .expect("valid");

        assert_eq!(sync.on_local_change(&text_content), SyncDecision::Send);
        assert_eq!(
            sync.on_local_change(&image),
            SyncDecision::Send,
            "an image was mistaken for text with the same bytes"
        );
    }

    #[test]
    fn unusable_content_is_rejected_not_forwarded() {
        let mut sync = ClipboardSync::new();
        let empty = ClipboardContent::Text(mb_types::Redacted::new(String::new()));

        assert!(matches!(
            sync.on_local_change(&empty),
            SyncDecision::Reject(ClipboardError::Empty)
        ));
        assert!(matches!(
            sync.on_remote_update(&empty),
            SyncDecision::Reject(ClipboardError::Empty)
        ));
        assert_eq!(sync.stats().rejected, 2);
        assert_eq!(sync.stats().sent, 0);
    }

    #[test]
    fn resetting_lets_the_same_content_be_copied_again() {
        // After a disconnect there is nobody to echo to, so retaining the history
        // would suppress the user's next copy of the same thing for no reason.
        let mut sync = ClipboardSync::new();
        let content = text("same again");
        assert_eq!(sync.on_local_change(&content), SyncDecision::Send);
        assert_eq!(sync.on_local_change(&content), SyncDecision::Ignore);

        sync.reset();
        assert_eq!(sync.on_local_change(&content), SyncDecision::Send);
    }

    #[test]
    fn a_three_way_exchange_settles() {
        // Three machines, each applying and re-broadcasting. The loop has more
        // paths to travel, and must still close.
        let mut machines = [
            ClipboardSync::new(),
            ClipboardSync::new(),
            ClipboardSync::new(),
        ];
        let content = text("broadcast");

        assert_eq!(machines[0].on_local_change(&content), SyncDecision::Send);
        for machine in &mut machines[1..] {
            assert_eq!(machine.on_remote_update(&content), SyncDecision::Apply);
            // Each one's watcher fires, and must not re-broadcast.
            assert_eq!(machine.on_local_change(&content), SyncDecision::Ignore);
        }
        // And the originator must not re-apply its own content.
        assert_eq!(machines[0].on_remote_update(&content), SyncDecision::Ignore);
    }
}
