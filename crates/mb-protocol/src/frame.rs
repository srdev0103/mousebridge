//! Length-prefixed framing for reliable streams.
//!
//! QUIC delivers a stream as an ordered byte sequence, not as messages, so
//! message boundaries have to be carried explicitly. Each frame is a four-byte
//! little-endian length followed by that many bytes of payload.
//!
//! # The length prefix is the attack surface
//!
//! A four-byte prefix claiming four gigabytes costs a hostile peer four bytes and
//! costs the receiver four gigabytes, if the receiver is naive enough to allocate
//! on it. [`MAX_FRAME_LEN`] is checked *before* any allocation, and a frame that
//! exceeds it terminates the connection rather than being skipped — a peer
//! sending one is either broken or hostile, and neither deserves another attempt.

use crate::error::ProtocolError;

/// Bytes of length prefix preceding each frame.
pub const LENGTH_PREFIX_LEN: usize = 4;

/// Largest payload accepted in a single frame.
///
/// One mebibyte. Chosen to be far above any control message or input event —
/// the largest is a `CursorEnter` carrying held keys, well under a kilobyte —
/// while staying small enough that a burst of maximum-size frames cannot exhaust
/// memory. Clipboard payloads and file transfers exceed this and are chunked;
/// that is a deliberate constraint on those features, not an oversight.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// The result of attempting to decode a frame from a stream buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameDecode<'a> {
    /// A complete frame was found.
    Complete {
        /// The payload, borrowed from the input buffer.
        payload: &'a [u8],
        /// Bytes to remove from the front of the buffer, prefix included.
        consumed: usize,
    },
    /// More bytes are needed.
    ///
    /// Not an error: a partially arrived frame is the normal state of a stream.
    Incomplete {
        /// Total bytes needed before decoding can succeed, as far as is known.
        needed: usize,
    },
}

/// Appends a length-prefixed frame to `out`.
///
/// # Errors
///
/// [`ProtocolError::FrameTooLarge`] if the payload exceeds [`MAX_FRAME_LEN`].
/// Checked on the sending side as well so a local bug surfaces here rather than
/// as an unexplained disconnect at the peer.
pub fn encode_frame(payload: &[u8], out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            claimed: payload.len(),
            limit: MAX_FRAME_LEN,
        });
    }
    let len = u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge {
        claimed: payload.len(),
        limit: MAX_FRAME_LEN,
    })?;
    out.reserve(LENGTH_PREFIX_LEN + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// Attempts to decode one frame from the front of `buf`.
///
/// # Errors
///
/// [`ProtocolError::FrameTooLarge`] if the length prefix exceeds
/// [`MAX_FRAME_LEN`]. The caller must close the connection: the stream cannot be
/// resynchronised, because there is no way to know where the claimed frame would
/// have ended.
pub fn decode_frame(buf: &[u8]) -> Result<FrameDecode<'_>, ProtocolError> {
    if buf.len() < LENGTH_PREFIX_LEN {
        return Ok(FrameDecode::Incomplete {
            needed: LENGTH_PREFIX_LEN,
        });
    }

    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    // Checked before the length is used for anything at all, including the
    // arithmetic below, which could otherwise overflow on a 32-bit target.
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            claimed: len,
            limit: MAX_FRAME_LEN,
        });
    }

    let total = LENGTH_PREFIX_LEN + len;
    if buf.len() < total {
        return Ok(FrameDecode::Incomplete { needed: total });
    }

    Ok(FrameDecode::Complete {
        payload: &buf[LENGTH_PREFIX_LEN..total],
        consumed: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut buf = Vec::new();
        encode_frame(b"hello", &mut buf).expect("small payload");
        assert_eq!(
            decode_frame(&buf),
            Ok(FrameDecode::Complete {
                payload: b"hello",
                consumed: 9,
            })
        );
    }

    #[test]
    fn an_empty_payload_is_a_valid_frame() {
        let mut buf = Vec::new();
        encode_frame(b"", &mut buf).expect("empty is fine");
        assert_eq!(buf.len(), LENGTH_PREFIX_LEN);
        assert_eq!(
            decode_frame(&buf),
            Ok(FrameDecode::Complete {
                payload: b"",
                consumed: 4,
            })
        );
    }

    #[test]
    fn several_frames_decode_in_sequence() {
        // The realistic case: a stream read returns several messages at once.
        let mut buf = Vec::new();
        for payload in [&b"one"[..], b"two", b"three"] {
            encode_frame(payload, &mut buf).expect("encodes");
        }

        let mut rest = &buf[..];
        let mut seen = Vec::new();
        while let Ok(FrameDecode::Complete { payload, consumed }) = decode_frame(rest) {
            seen.push(payload.to_vec());
            rest = &rest[consumed..];
            if rest.is_empty() {
                break;
            }
        }
        assert_eq!(
            seen,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn a_partial_frame_is_incomplete_not_an_error() {
        // Streams deliver arbitrary fragments; treating this as an error would
        // drop a connection on every slow packet.
        let mut buf = Vec::new();
        encode_frame(b"payload", &mut buf).expect("encodes");

        for len in 0..buf.len() {
            match decode_frame(&buf[..len]) {
                Ok(FrameDecode::Incomplete { needed }) => {
                    assert!(needed > len, "needed {needed} for {len} available");
                }
                other => panic!("expected Incomplete at len {len}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_partial_prefix_asks_for_the_prefix_first() {
        assert_eq!(
            decode_frame(&[0x01, 0x02]),
            Ok(FrameDecode::Incomplete { needed: 4 })
        );
    }

    #[test]
    fn an_oversized_length_prefix_is_rejected_before_allocating() {
        // Four bytes claiming four gigabytes. The check must happen before the
        // length is used for anything.
        let hostile = [0xFF, 0xFF, 0xFF, 0xFF];
        let err = decode_frame(&hostile).expect_err("must reject");
        assert_eq!(
            err,
            ProtocolError::FrameTooLarge {
                claimed: 0xFFFF_FFFF,
                limit: MAX_FRAME_LEN,
            }
        );
    }

    #[test]
    fn a_length_just_over_the_limit_is_rejected() {
        let over = u32::try_from(MAX_FRAME_LEN + 1).expect("fits");
        assert!(decode_frame(&over.to_le_bytes()).is_err());

        // And exactly at the limit is accepted, so the boundary is not off by one.
        let at = u32::try_from(MAX_FRAME_LEN).expect("fits");
        assert_eq!(
            decode_frame(&at.to_le_bytes()),
            Ok(FrameDecode::Incomplete {
                needed: MAX_FRAME_LEN + LENGTH_PREFIX_LEN
            })
        );
    }

    #[test]
    fn encoding_refuses_an_oversized_payload() {
        // Caught locally rather than surfacing as an unexplained disconnect at
        // the peer.
        let huge = vec![0u8; MAX_FRAME_LEN + 1];
        let mut buf = Vec::new();
        assert!(encode_frame(&huge, &mut buf).is_err());
        assert!(buf.is_empty(), "nothing should be written on failure");
    }

    #[test]
    fn a_maximum_size_frame_round_trips() {
        let payload = vec![0xABu8; MAX_FRAME_LEN];
        let mut buf = Vec::new();
        encode_frame(&payload, &mut buf).expect("at the limit");
        match decode_frame(&buf) {
            Ok(FrameDecode::Complete { payload: got, .. }) => assert_eq!(got.len(), MAX_FRAME_LEN),
            other => panic!("expected a complete frame, got {other:?}"),
        }
    }
}
