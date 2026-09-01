//! Splitting large payloads across frames, and putting them back together.
//!
//! Frames are capped at [`crate::frame::MAX_FRAME_LEN`], which is far smaller
//! than a clipboard image or a file. Anything larger travels as a sequence of
//! chunks preceded by an offer describing what is coming.
//!
//! # Reassembly is the attack surface
//!
//! A receiver reassembling attacker-controlled chunks is being asked to allocate
//! memory on the strength of a claim. Every bound here is therefore checked
//! against a local limit rather than against what the sender said:
//!
//! * the declared total is refused before a single byte is buffered,
//! * chunks that would push the running total past the declared size are
//!   refused, so a lying offer cannot be exceeded either,
//! * duplicate and out-of-order chunks are rejected rather than accumulated,
//! * and the assembled bytes are checked against the declared hash, so a
//!   truncated or corrupted transfer never surfaces as valid content.
//!
//! The last one matters most. Without it a partial clipboard would silently
//! become the user's clipboard, and a partial file would silently become a file.

use crate::error::ProtocolError;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Payload bytes per chunk.
///
/// Comfortably inside the frame limit, leaving room for the message envelope,
/// and large enough that a multi-megabyte payload does not become thousands of
/// round trips.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// Largest payload that may be offered.
///
/// Sixteen mebibytes, matching the clipboard limit. File transfer raises this
/// deliberately in milestone 12; the reassembler takes its limit as a parameter
/// so the two cannot drift apart by accident.
pub const DEFAULT_MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// Describes a payload about to be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkedOffer {
    /// Identifies this transfer within the session.
    pub id: u32,
    /// Total size in bytes.
    ///
    /// A claim, not a fact. Checked against the local limit before anything is
    /// allocated, and against the bytes actually delivered at the end.
    pub total_len: u64,
    /// SHA-256 of the complete payload.
    pub hash: [u8; 32],
}

/// One piece of a payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// Which transfer this belongs to.
    pub id: u32,
    /// Position in the sequence, starting at zero.
    pub index: u32,
    /// The bytes.
    pub data: Vec<u8>,
}

/// Splits a payload into an offer and its chunks.
#[must_use]
pub fn split(id: u32, payload: &[u8]) -> (ChunkedOffer, Vec<Chunk>) {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);

    let chunks = payload
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(index, data)| Chunk {
            id,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            data: data.to_vec(),
        })
        .collect();

    (
        ChunkedOffer {
            id,
            total_len: payload.len() as u64,
            hash,
        },
        chunks,
    )
}

/// Reassembles a chunked payload.
#[derive(Debug)]
pub struct Reassembler {
    offer: ChunkedOffer,
    buffer: Vec<u8>,
    next_index: u32,
    max_payload: usize,
}

impl Reassembler {
    /// Accepts an offer and prepares to receive it.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::FrameTooLarge`] if the declared size exceeds
    /// `max_payload`. Checked here, before anything is allocated: this is the
    /// point at which a hostile offer would otherwise cost memory.
    pub fn new(offer: ChunkedOffer, max_payload: usize) -> Result<Self, ProtocolError> {
        let declared = usize::try_from(offer.total_len).unwrap_or(usize::MAX);
        if declared > max_payload {
            return Err(ProtocolError::FrameTooLarge {
                claimed: declared,
                limit: max_payload,
            });
        }
        Ok(Self {
            offer,
            // Reserving the declared size is safe only because it has just been
            // bounded above.
            buffer: Vec::with_capacity(declared),
            next_index: 0,
            max_payload,
        })
    }

    /// The transfer this is reassembling.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.offer.id
    }

    /// Bytes received so far.
    #[must_use]
    pub fn received(&self) -> usize {
        self.buffer.len()
    }

    /// Total bytes expected.
    #[must_use]
    pub const fn expected(&self) -> u64 {
        self.offer.total_len
    }

    /// Progress from zero to one.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.offer.total_len == 0 {
            return 1.0;
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "payloads are far below 2^24 bytes of precision loss"
        )]
        {
            (self.received() as f32 / self.offer.total_len as f32).clamp(0.0, 1.0)
        }
    }

    /// Adds a chunk.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Invalid`] for a chunk belonging to another transfer, one
    /// arriving out of order, or one that would exceed the declared size.
    ///
    /// Chunks travel on a reliable ordered stream, so out-of-order arrival means
    /// the peer is malfunctioning or hostile — not that the network reordered
    /// something. Buffering to repair it would be doing an attacker's memory
    /// allocation for them.
    pub fn push(&mut self, chunk: &Chunk) -> Result<(), ProtocolError> {
        if chunk.id != self.offer.id {
            return Err(ProtocolError::Invalid {
                field: "chunk.id",
                reason: "chunk belongs to a different transfer",
            });
        }
        if chunk.index != self.next_index {
            return Err(ProtocolError::Invalid {
                field: "chunk.index",
                reason: "chunk arrived out of order on an ordered stream",
            });
        }

        let would_be = self.buffer.len().saturating_add(chunk.data.len());
        let declared = usize::try_from(self.offer.total_len).unwrap_or(usize::MAX);
        if would_be > declared || would_be > self.max_payload {
            // A sender that offered one size and is delivering another. Refusing
            // here means a lying offer cannot be exceeded either.
            return Err(ProtocolError::Invalid {
                field: "chunk.data",
                reason: "chunks exceed the size the offer declared",
            });
        }

        self.buffer.extend_from_slice(&chunk.data);
        self.next_index = self.next_index.saturating_add(1);
        Ok(())
    }

    /// True when every byte has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.buffer.len() as u64 == self.offer.total_len
    }

    /// Verifies and returns the assembled payload.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Invalid`] if bytes are missing, or if the content does
    /// not match the declared hash. Both are fatal for the transfer: a partial
    /// clipboard would silently become the user's clipboard, and a partial file
    /// would silently become a file.
    pub fn finish(self) -> Result<Vec<u8>, ProtocolError> {
        if !self.is_complete() {
            return Err(ProtocolError::Invalid {
                field: "transfer",
                reason: "transfer ended before every byte arrived",
            });
        }

        let mut hasher = Sha256::new();
        hasher.update(&self.buffer);
        let digest = hasher.finalize();

        if digest.as_slice() != self.offer.hash {
            return Err(ProtocolError::Invalid {
                field: "transfer",
                reason: "assembled content does not match the declared hash",
            });
        }
        Ok(self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn reassemble(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let (offer, chunks) = split(1, data);
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD)?;
        for chunk in &chunks {
            reassembler.push(chunk)?;
        }
        reassembler.finish()
    }

    #[test]
    fn a_small_payload_round_trips() {
        let data = payload(100);
        assert_eq!(reassemble(&data).expect("reassembles"), data);
    }

    #[test]
    fn a_payload_spanning_many_chunks_round_trips() {
        let data = payload(CHUNK_SIZE * 3 + 17);
        let (_, chunks) = split(1, &data);
        assert_eq!(chunks.len(), 4);
        assert_eq!(reassemble(&data).expect("reassembles"), data);
    }

    #[test]
    fn a_payload_exactly_one_chunk_long_round_trips() {
        let data = payload(CHUNK_SIZE);
        let (_, chunks) = split(1, &data);
        assert_eq!(
            chunks.len(),
            1,
            "an exact multiple produced a trailing chunk"
        );
        assert_eq!(reassemble(&data).expect("reassembles"), data);
    }

    #[test]
    fn an_empty_payload_round_trips() {
        let (offer, chunks) = split(1, &[]);
        assert!(chunks.is_empty());
        let reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        assert!(reassembler.is_complete());
        assert!(reassembler.finish().expect("finishes").is_empty());
    }

    #[test]
    fn an_oversized_offer_is_refused_before_allocating() {
        // The point at which a hostile offer would otherwise cost memory.
        let offer = ChunkedOffer {
            id: 1,
            total_len: u64::MAX,
            hash: [0; 32],
        };
        assert!(matches!(
            Reassembler::new(offer, DEFAULT_MAX_PAYLOAD),
            Err(ProtocolError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn a_lying_offer_cannot_be_exceeded() {
        // Declaring ten bytes and then sending a megabyte. Without the running
        // check, the offer limit would be meaningless.
        let offer = ChunkedOffer {
            id: 1,
            total_len: 10,
            hash: [0; 32],
        };
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        let result = reassembler.push(&Chunk {
            id: 1,
            index: 0,
            data: payload(1_000_000),
        });
        assert!(matches!(result, Err(ProtocolError::Invalid { .. })));
        assert_eq!(
            reassembler.received(),
            0,
            "the oversized chunk was buffered"
        );
    }

    #[test]
    fn a_chunk_from_another_transfer_is_refused() {
        let (offer, _) = split(1, &payload(100));
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        let result = reassembler.push(&Chunk {
            id: 99,
            index: 0,
            data: vec![1, 2, 3],
        });
        assert!(matches!(result, Err(ProtocolError::Invalid { .. })));
    }

    #[test]
    fn out_of_order_chunks_are_refused_rather_than_buffered() {
        // These travel on a reliable ordered stream, so disorder means the peer
        // is malfunctioning or hostile. Buffering to repair it would be doing an
        // attacker's memory allocation for them.
        let data = payload(CHUNK_SIZE * 2);
        let (offer, chunks) = split(1, &data);
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");

        assert!(
            reassembler.push(&chunks[1]).is_err(),
            "accepted chunk 1 first"
        );
        reassembler.push(&chunks[0]).expect("chunk 0");
        assert!(
            reassembler.push(&chunks[0]).is_err(),
            "accepted a duplicate"
        );
    }

    #[test]
    fn a_truncated_transfer_never_becomes_valid_content() {
        // Without this a partial clipboard would silently become the user's
        // clipboard, and a partial file would silently become a file.
        let data = payload(CHUNK_SIZE * 2);
        let (offer, chunks) = split(1, &data);
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        reassembler.push(&chunks[0]).expect("chunk 0");

        assert!(!reassembler.is_complete());
        assert!(matches!(
            reassembler.finish(),
            Err(ProtocolError::Invalid { .. })
        ));
    }

    #[test]
    fn corrupted_content_is_caught_by_the_hash() {
        let data = payload(1000);
        let (offer, mut chunks) = split(1, &data);
        chunks[0].data[500] ^= 0xFF;

        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        for chunk in &chunks {
            reassembler.push(chunk).expect("pushes");
        }
        assert!(
            reassembler.is_complete(),
            "the right number of bytes arrived"
        );
        assert!(
            matches!(reassembler.finish(), Err(ProtocolError::Invalid { .. })),
            "corrupted content was accepted"
        );
    }

    #[test]
    fn a_forged_hash_is_caught() {
        // A sender declaring the hash of something else entirely.
        let data = payload(1000);
        let (mut offer, chunks) = split(1, &data);
        offer.hash = [0xAB; 32];

        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");
        for chunk in &chunks {
            reassembler.push(chunk).expect("pushes");
        }
        assert!(reassembler.finish().is_err());
    }

    #[test]
    fn progress_advances_monotonically_to_one() {
        let data = payload(CHUNK_SIZE * 4);
        let (offer, chunks) = split(1, &data);
        let mut reassembler = Reassembler::new(offer, DEFAULT_MAX_PAYLOAD).expect("accepted");

        let mut previous = 0.0;
        for chunk in &chunks {
            reassembler.push(chunk).expect("pushes");
            let progress = reassembler.progress();
            assert!(progress >= previous, "progress went backwards");
            assert!((0.0..=1.0).contains(&progress));
            previous = progress;
        }
        assert!((previous - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_lower_limit_is_respected() {
        // File transfer raises the limit; the clipboard does not. Passing it in
        // stops the two drifting apart.
        let data = payload(2000);
        let (offer, _) = split(1, &data);
        assert!(Reassembler::new(offer, 1000).is_err());
    }
}
