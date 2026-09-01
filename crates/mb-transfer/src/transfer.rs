//! The lifecycle of one incoming file.
//!
//! # Why a state machine
//!
//! A transfer can end in more ways than it can succeed: the user cancels, the
//! sender cancels, the connection drops mid-file, the disk fills, the hash does
//! not match. Each leaves a partial file on disk that must not be mistaken for a
//! complete one.
//!
//! Making the states explicit means the rule that matters — **a partial file is
//! never presented as finished** — is enforced by the type rather than by
//! remembering to check.
//!
//! # Consent is per transfer, not per peer
//!
//! Being paired means a peer may send input. It does not mean it may write files
//! to the user's disk unannounced. Every transfer starts in
//! [`TransferState::Offered`] and moves no further until it is accepted.

use mb_protocol::ProtocolError;
use mb_protocol::chunking::{Chunk, ChunkedOffer, Reassembler};
use std::path::PathBuf;

/// Largest file that will be accepted.
///
/// Two gibibytes. High enough to be useful, bounded so a peer cannot fill the
/// user's disk with a single offer, and well inside the range where a `u64`
/// length is unambiguous.
pub const MAX_FILE_LEN: usize = 2 * 1024 * 1024 * 1024;

/// Where a transfer has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    /// The peer has offered a file. Nothing has been written.
    Offered,
    /// The user accepted; bytes are arriving.
    Receiving,
    /// Every byte arrived and the content matched its hash.
    Complete {
        /// Where the file was written.
        path: PathBuf,
    },
    /// The transfer ended without completing.
    ///
    /// Whatever was written is a partial file and must not be presented as the
    /// finished one.
    Failed {
        /// Why.
        reason: TransferError,
    },
}

impl TransferState {
    /// True when the transfer has finished, either way.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self, Self::Complete { .. } | Self::Failed { .. })
    }

    /// True when bytes may still be accepted.
    #[must_use]
    pub const fn is_receiving(&self) -> bool {
        matches!(self, Self::Receiving)
    }
}

/// Why a transfer ended without completing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    /// The user declined the offer.
    #[error("declined")]
    Declined,
    /// The user cancelled after accepting.
    #[error("cancelled")]
    CancelledLocally,
    /// The sender cancelled.
    #[error("the sender cancelled the transfer")]
    CancelledByPeer,
    /// The connection went away mid-transfer.
    #[error("the connection was lost before the file finished")]
    ConnectionLost,
    /// The offered file was larger than [`MAX_FILE_LEN`].
    #[error("the offered file is {size} bytes, above the {MAX_FILE_LEN} byte limit")]
    TooLarge {
        /// Size offered.
        size: u64,
    },
    /// The name could not be used.
    #[error(transparent)]
    Path(#[from] crate::path::PathError),
    /// The content did not match, or arrived malformed.
    #[error("the file did not arrive intact: {reason}")]
    Corrupt {
        /// What went wrong.
        reason: String,
    },
    /// Writing failed.
    #[error("could not write the file: {detail}")]
    Io {
        /// Underlying cause.
        detail: String,
    },
}

/// Something the caller should act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    /// Bytes arrived; the caller may update a progress display.
    Progress,
    /// The transfer finished successfully.
    Completed {
        /// Where the file was written.
        path: PathBuf,
    },
    /// The transfer ended without completing.
    ///
    /// Any partial file the caller wrote must be removed: leaving it behind
    /// means the user finds something that looks like their file and is not.
    Failed {
        /// Why.
        reason: TransferError,
    },
}

/// One incoming file.
#[derive(Debug)]
pub struct Transfer {
    id: u32,
    /// Name exactly as the peer sent it, for display in the prompt.
    ///
    /// Never used to build a path — [`crate::path::safe_file_name`] does that.
    offered_name: String,
    total_len: u64,
    state: TransferState,
    reassembler: Option<Reassembler>,
    destination: Option<PathBuf>,
}

impl Transfer {
    /// Records an offer from a peer.
    ///
    /// The size is checked here, before the user is even prompted: there is no
    /// point asking about a file that would be refused anyway.
    #[must_use]
    pub fn offered(id: u32, name: String, total_len: u64) -> Self {
        let state = if total_len > MAX_FILE_LEN as u64 {
            TransferState::Failed {
                reason: TransferError::TooLarge { size: total_len },
            }
        } else {
            TransferState::Offered
        };

        Self {
            id,
            offered_name: name,
            total_len,
            state,
            reassembler: None,
            destination: None,
        }
    }

    /// The transfer's identifier within the session.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// The name the peer offered, for display.
    #[must_use]
    pub fn offered_name(&self) -> &str {
        &self.offered_name
    }

    /// The size the peer declared.
    #[must_use]
    pub const fn total_len(&self) -> u64 {
        self.total_len
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> &TransferState {
        &self.state
    }

    /// Where the file is being written, once accepted.
    #[must_use]
    pub fn destination(&self) -> Option<&PathBuf> {
        self.destination.as_ref()
    }

    /// Progress from zero to one.
    #[must_use]
    pub fn progress(&self) -> f32 {
        match &self.state {
            TransferState::Complete { .. } => 1.0,
            _ => self.reassembler.as_ref().map_or(0.0, Reassembler::progress),
        }
    }

    /// Accepts the offer, fixing the destination.
    ///
    /// # Errors
    ///
    /// [`TransferError`] if the transfer is no longer in a state to be accepted,
    /// or if the destination could not be prepared.
    pub fn accept(
        &mut self,
        destination: PathBuf,
        offer: ChunkedOffer,
    ) -> Result<(), TransferError> {
        if self.state != TransferState::Offered {
            return Err(TransferError::Corrupt {
                reason: "transfer is not awaiting acceptance".to_owned(),
            });
        }

        let reassembler = Reassembler::new(offer, MAX_FILE_LEN).map_err(|e: ProtocolError| {
            TransferError::Corrupt {
                reason: e.to_string(),
            }
        })?;

        self.reassembler = Some(reassembler);
        self.destination = Some(destination);
        self.state = TransferState::Receiving;
        Ok(())
    }

    /// Declines the offer.
    pub fn decline(&mut self) {
        if self.state == TransferState::Offered {
            self.state = TransferState::Failed {
                reason: TransferError::Declined,
            };
        }
    }

    /// Accepts a chunk.
    ///
    /// Returns what the caller should do. Chunks arriving after the transfer has
    /// finished are ignored rather than treated as errors: a cancellation and an
    /// in-flight chunk cross on the wire routinely.
    pub fn push(&mut self, chunk: &Chunk) -> Option<TransferEvent> {
        if !self.state.is_receiving() {
            return None;
        }

        let reassembler = self.reassembler.as_mut()?;

        if let Err(e) = reassembler.push(chunk) {
            let reason = TransferError::Corrupt {
                reason: e.to_string(),
            };
            self.state = TransferState::Failed {
                reason: reason.clone(),
            };
            return Some(TransferEvent::Failed { reason });
        }

        if !reassembler.is_complete() {
            return Some(TransferEvent::Progress);
        }

        // Take the reassembler so the hash check consumes it exactly once.
        let reassembler = self.reassembler.take()?;
        match reassembler.finish() {
            Ok(_bytes) => {
                let path = self.destination.clone().unwrap_or_default();
                self.state = TransferState::Complete { path: path.clone() };
                Some(TransferEvent::Completed { path })
            }
            Err(e) => {
                // The bytes arrived but do not match. Deliberately fatal: a file
                // that looks complete and is not is worse than one that failed.
                let reason = TransferError::Corrupt {
                    reason: e.to_string(),
                };
                self.state = TransferState::Failed {
                    reason: reason.clone(),
                };
                Some(TransferEvent::Failed { reason })
            }
        }
    }

    /// Ends the transfer.
    ///
    /// Does nothing to a transfer that has already finished, so a cancellation
    /// crossing a completion on the wire cannot undo a successful transfer.
    pub fn cancel(&mut self, reason: TransferError) -> Option<TransferEvent> {
        if self.state.is_finished() {
            return None;
        }
        self.reassembler = None;
        self.state = TransferState::Failed {
            reason: reason.clone(),
        };
        Some(TransferEvent::Failed { reason })
    }

    /// The assembled bytes, if the transfer completed.
    ///
    /// Returns `None` in every other state. This is the gate: a partial file can
    /// never be presented as a finished one.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.state, TransferState::Complete { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_protocol::chunking::split;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn accepted(data: &[u8]) -> (Transfer, Vec<Chunk>) {
        let (offer, chunks) = split(7, data);
        let mut transfer = Transfer::offered(7, "report.pdf".to_owned(), data.len() as u64);
        transfer
            .accept(PathBuf::from("/downloads/report.pdf"), offer)
            .expect("accepted");
        (transfer, chunks)
    }

    #[test]
    fn a_transfer_starts_awaiting_consent() {
        // Being paired means a peer may send input. It does not mean it may write
        // files to the user's disk unannounced.
        let transfer = Transfer::offered(1, "file.txt".to_owned(), 100);
        assert_eq!(transfer.state(), &TransferState::Offered);
        assert!(transfer.destination().is_none());
        assert_eq!(transfer.progress(), 0.0);
    }

    #[test]
    fn a_complete_transfer_reports_where_it_landed() {
        let data = payload(1000);
        let (mut transfer, chunks) = accepted(&data);

        let mut last = None;
        for chunk in &chunks {
            last = transfer.push(chunk);
        }

        assert_eq!(
            last,
            Some(TransferEvent::Completed {
                path: PathBuf::from("/downloads/report.pdf")
            })
        );
        assert!(transfer.is_complete());
        assert!((transfer.progress() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_declined_offer_writes_nothing() {
        let mut transfer = Transfer::offered(1, "unwanted.exe".to_owned(), 100);
        transfer.decline();

        assert_eq!(
            transfer.state(),
            &TransferState::Failed {
                reason: TransferError::Declined
            }
        );
        assert!(transfer.destination().is_none());
        assert!(!transfer.is_complete());
    }

    #[test]
    fn an_oversized_offer_is_refused_before_prompting() {
        // No point asking the user about a file that would be refused anyway.
        let transfer = Transfer::offered(1, "huge.iso".to_owned(), MAX_FILE_LEN as u64 + 1);
        assert!(matches!(
            transfer.state(),
            TransferState::Failed {
                reason: TransferError::TooLarge { .. }
            }
        ));
    }

    #[test]
    fn a_transfer_at_the_limit_is_offered_normally() {
        let transfer = Transfer::offered(1, "big.iso".to_owned(), MAX_FILE_LEN as u64);
        assert_eq!(transfer.state(), &TransferState::Offered);
    }

    #[test]
    fn corrupted_content_fails_rather_than_completing() {
        // A file that looks complete and is not is worse than one that failed.
        let data = payload(1000);
        let (offer, mut chunks) = split(7, &data);
        chunks[0].data[10] ^= 0xFF;

        let mut transfer = Transfer::offered(7, "report.pdf".to_owned(), data.len() as u64);
        transfer
            .accept(PathBuf::from("/downloads/report.pdf"), offer)
            .expect("accepted");

        let mut last = None;
        for chunk in &chunks {
            last = transfer.push(chunk);
        }

        assert!(
            matches!(last, Some(TransferEvent::Failed { .. })),
            "corrupted content completed: {last:?}"
        );
        assert!(!transfer.is_complete());
    }

    #[test]
    fn a_truncated_transfer_never_completes() {
        let data = payload(mb_protocol::chunking::CHUNK_SIZE * 2);
        let (mut transfer, chunks) = accepted(&data);

        assert_eq!(transfer.push(&chunks[0]), Some(TransferEvent::Progress));
        assert!(!transfer.is_complete());

        // The connection drops.
        transfer.cancel(TransferError::ConnectionLost);
        assert!(!transfer.is_complete());
        assert!(transfer.state().is_finished());
    }

    #[test]
    fn out_of_order_chunks_fail_the_transfer() {
        let data = payload(mb_protocol::chunking::CHUNK_SIZE * 2);
        let (mut transfer, chunks) = accepted(&data);

        let event = transfer.push(&chunks[1]);
        assert!(matches!(event, Some(TransferEvent::Failed { .. })));
        assert!(!transfer.is_complete());
    }

    #[test]
    fn a_cancellation_crossing_a_completion_cannot_undo_it() {
        // Routine on a real network: the user cancels just as the last chunk
        // arrives. The completed file must stay completed.
        let data = payload(500);
        let (mut transfer, chunks) = accepted(&data);
        for chunk in &chunks {
            transfer.push(chunk);
        }
        assert!(transfer.is_complete());

        assert_eq!(transfer.cancel(TransferError::CancelledLocally), None);
        assert!(
            transfer.is_complete(),
            "a late cancellation undid a completed file"
        );
    }

    #[test]
    fn chunks_arriving_after_cancellation_are_ignored() {
        // A cancellation and an in-flight chunk cross on the wire routinely.
        let data = payload(mb_protocol::chunking::CHUNK_SIZE * 2);
        let (mut transfer, chunks) = accepted(&data);

        transfer.cancel(TransferError::CancelledByPeer);
        assert_eq!(transfer.push(&chunks[0]), None);
        assert!(!transfer.is_complete());
    }

    #[test]
    fn a_transfer_cannot_be_accepted_twice() {
        let data = payload(100);
        let (offer, _) = split(7, &data);
        let (mut transfer, _) = accepted(&data);

        assert!(transfer.accept(PathBuf::from("/elsewhere"), offer).is_err());
        assert_eq!(
            transfer.destination(),
            Some(&PathBuf::from("/downloads/report.pdf")),
            "the destination was changed after acceptance"
        );
    }

    #[test]
    fn progress_advances_monotonically() {
        let data = payload(mb_protocol::chunking::CHUNK_SIZE * 3);
        let (mut transfer, chunks) = accepted(&data);

        let mut previous = 0.0;
        for chunk in &chunks {
            transfer.push(chunk);
            let progress = transfer.progress();
            assert!(progress >= previous, "progress went backwards");
            assert!((0.0..=1.0).contains(&progress));
            previous = progress;
        }
        assert!((previous - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_offered_name_is_kept_for_display_but_not_used_as_a_path() {
        // The prompt should show what the peer actually sent, so a suspicious
        // name is visible to the user. The path is built separately.
        let transfer = Transfer::offered(1, "../../../etc/passwd".to_owned(), 10);
        assert_eq!(transfer.offered_name(), "../../../etc/passwd");
        assert!(transfer.destination().is_none());
    }
}
