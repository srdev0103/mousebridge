//! The pointer-motion datagram.
//!
//! Sixteen bytes, fixed layout, little-endian. Encoded and decoded without
//! allocation, without `serde`, and without a length prefix, because this runs on
//! the input path where a serialiser's overhead is latency a user can feel.
//!
//! ```text
//!  0        1        2      3        4                7
//! ┌────────┬────────┬────────────────┬─────────────────┐
//! │  kind  │ flags  │   sequence     │  timestamp_us   │
//! ├────────┴────────┴────────────────┼─────────────────┤
//! │           x  (i32)               │    y  (i32)     │
//! └──────────────────────────────────┴─────────────────┘
//!  8                              11  12             15
//! ```
//!
//! # Why an absolute position, on an unreliable channel
//!
//! Motion travels as a QUIC datagram, which may be dropped or reordered. Sending
//! deltas over such a channel is unsound: every lost packet is permanent drift
//! between the two cursors, with nothing to correct it. Sending the accumulated
//! position in the *target's* coordinate space makes loss self-healing — a
//! dropped packet costs a single frame, and the next one is authoritative again.
//!
//! It also avoids double pointer acceleration on injection. See
//! `docs/adr/0001-display-coordinate-space.md`.

use crate::error::ProtocolError;

/// Encoded size of a motion datagram.
pub const MOTION_LEN: usize = 16;

/// Discriminant byte identifying a motion datagram.
///
/// Datagrams carry no length prefix, so the first byte is the only thing
/// distinguishing this from a future datagram type.
pub const MOTION_KIND: u8 = 0x01;

/// Largest value of a normalised motion coordinate.
///
/// Positions travel as a fraction of the target screen scaled to this range,
/// which is the same convention `SendInput` uses for absolute positioning.
pub const MOTION_SCALE: i32 = 65535;

/// A pointer position within the screen the cursor is on.
///
/// # The coordinates are normalised, not pixels
///
/// `x` and `y` are the position *within the target screen*, as a fraction from
/// `0` to [`MOTION_SCALE`].
///
/// Sending pixels does not work, and getting this wrong produced a pointer that
/// went nowhere useful: the sender knows its own screen in its own units, and the
/// receiver's screen is a different size at a different scale factor. Only the
/// receiver knows how to turn a position into one of its own pixels. Normalising
/// puts that conversion where the information is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    /// Wrapping counter, for discarding reordered packets.
    pub seq: u16,
    /// Sender's monotonic clock in microseconds, truncated to 32 bits.
    ///
    /// Used only for latency measurement, never for ordering — the two machines'
    /// clocks are unrelated. It wraps about every 71 minutes, which is
    /// immaterial for measuring a single hop.
    pub timestamp_us: u32,
    /// Horizontal position across the target screen, `0..=`[`MOTION_SCALE`].
    pub x: i32,
    /// Vertical position down the target screen, `0..=`[`MOTION_SCALE`].
    pub y: i32,
}

impl Motion {
    /// Builds a datagram from a normalised position.
    ///
    /// Values outside `0.0..=1.0` are clamped: a position off the edge of the
    /// target screen is not a place the pointer can go.
    #[must_use]
    pub fn from_normalized(seq: u16, timestamp_us: u32, nx: f64, ny: f64) -> Self {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "clamped to 0..=65535 before conversion"
        )]
        let scale = |value: f64| {
            if value.is_nan() {
                return 0;
            }
            (value.clamp(0.0, 1.0) * f64::from(MOTION_SCALE)).round() as i32
        };
        Self {
            seq,
            timestamp_us,
            x: scale(nx),
            y: scale(ny),
        }
    }

    /// The position as a fraction of the target screen.
    #[must_use]
    pub fn normalized(&self) -> (f64, f64) {
        let scale = f64::from(MOTION_SCALE);
        (
            (f64::from(self.x) / scale).clamp(0.0, 1.0),
            (f64::from(self.y) / scale).clamp(0.0, 1.0),
        )
    }
}

impl Motion {
    /// Writes this datagram into a fixed buffer.
    ///
    /// Takes an exact-size array rather than a slice so the length is checked at
    /// compile time and the encoder cannot fail at runtime.
    pub const fn encode(&self, out: &mut [u8; MOTION_LEN]) {
        out[0] = MOTION_KIND;
        out[1] = 0; // flags: reserved, must be zero on send
        let seq = self.seq.to_le_bytes();
        out[2] = seq[0];
        out[3] = seq[1];
        let ts = self.timestamp_us.to_le_bytes();
        out[4] = ts[0];
        out[5] = ts[1];
        out[6] = ts[2];
        out[7] = ts[3];
        let x = self.x.to_le_bytes();
        out[8] = x[0];
        out[9] = x[1];
        out[10] = x[2];
        out[11] = x[3];
        let y = self.y.to_le_bytes();
        out[12] = y[0];
        out[13] = y[1];
        out[14] = y[2];
        out[15] = y[3];
    }

    /// Returns this datagram as an owned array.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; MOTION_LEN] {
        let mut out = [0u8; MOTION_LEN];
        self.encode(&mut out);
        out
    }

    /// Parses a motion datagram.
    ///
    /// Unknown bits in the flags byte are ignored rather than rejected, so a
    /// future version can add a flag without older peers dropping the packet.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Truncated`] if the buffer is short, or
    /// [`ProtocolError::UnknownType`] if the first byte is not [`MOTION_KIND`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < MOTION_LEN {
            return Err(ProtocolError::Truncated {
                what: "motion datagram",
                needed: MOTION_LEN,
                available: bytes.len(),
            });
        }
        if bytes[0] != MOTION_KIND {
            return Err(ProtocolError::UnknownType { found: bytes[0] });
        }
        Ok(Self {
            seq: u16::from_le_bytes([bytes[2], bytes[3]]),
            timestamp_us: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            x: i32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            y: i32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        })
    }
}

/// Returns true if `candidate` is newer than `reference`, allowing for wraparound.
///
/// Sequence numbers are 16 bits and wrap roughly every 65,000 packets — a few
/// minutes of continuous motion. A naive `>` comparison would therefore discard
/// every packet for the next half-cycle each time the counter wrapped, freezing
/// the remote cursor for minutes.
///
/// The comparison is RFC 1982 serial arithmetic: `candidate` is newer when the
/// forward distance to it is less than half the number space.
#[must_use]
pub const fn is_newer(candidate: u16, reference: u16) -> bool {
    candidate != reference && candidate.wrapping_sub(reference) < 0x8000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Motion {
        Motion {
            seq: 1234,
            timestamp_us: 987_654,
            x: -1920,
            y: 1080,
        }
    }

    #[test]
    fn round_trips() {
        let motion = sample();
        let bytes = motion.to_bytes();
        assert_eq!(bytes.len(), MOTION_LEN);
        assert_eq!(Motion::decode(&bytes), Ok(motion));
    }

    #[test]
    fn negative_coordinates_survive() {
        // A display left of the primary one has negative coordinates. Encoding
        // these unsigned would teleport the cursor to the far edge.
        let motion = Motion {
            seq: 0,
            timestamp_us: 0,
            x: -3840,
            y: -2160,
        };
        assert_eq!(Motion::decode(&motion.to_bytes()), Ok(motion));
    }

    #[test]
    fn extreme_values_survive() {
        for (x, y) in [(i32::MIN, i32::MAX), (i32::MAX, i32::MIN), (0, 0)] {
            let motion = Motion {
                seq: u16::MAX,
                timestamp_us: u32::MAX,
                x,
                y,
            };
            assert_eq!(Motion::decode(&motion.to_bytes()), Ok(motion));
        }
    }

    #[test]
    fn truncated_input_is_rejected_not_read_past() {
        let bytes = sample().to_bytes();
        for len in 0..MOTION_LEN {
            let err = Motion::decode(&bytes[..len]).expect_err("must reject");
            assert!(matches!(err, ProtocolError::Truncated { .. }), "len {len}");
        }
    }

    #[test]
    fn a_wrong_discriminant_is_rejected() {
        let mut bytes = sample().to_bytes();
        bytes[0] = 0xFF;
        assert_eq!(
            Motion::decode(&bytes),
            Err(ProtocolError::UnknownType { found: 0xFF })
        );
    }

    #[test]
    fn unknown_flag_bits_are_ignored_for_forward_compatibility() {
        // A future version adding a flag must not make this build drop the
        // packet; the position is still perfectly readable.
        let mut bytes = sample().to_bytes();
        bytes[1] = 0b1010_1010;
        assert_eq!(Motion::decode(&bytes), Ok(sample()));
    }

    #[test]
    fn trailing_bytes_are_ignored() {
        // Datagrams may be padded by a lower layer; extra bytes are not an error.
        let mut bytes = sample().to_bytes().to_vec();
        bytes.extend_from_slice(&[0xAA; 32]);
        assert_eq!(Motion::decode(&bytes), Ok(sample()));
    }

    #[test]
    fn normalised_positions_round_trip() {
        for (nx, ny) in [(0.0, 0.0), (0.5, 0.25), (1.0, 1.0), (0.999, 0.001)] {
            let motion = Motion::from_normalized(1, 0, nx, ny);
            let (bx, by) = motion.normalized();
            assert!((bx - nx).abs() < 1e-4, "{nx} -> {bx}");
            assert!((by - ny).abs() < 1e-4, "{ny} -> {by}");
        }
    }

    #[test]
    fn positions_off_the_target_screen_are_clamped() {
        // A position outside the screen is not somewhere the pointer can go, and
        // an unclamped value would be injected as a wild jump.
        let motion = Motion::from_normalized(1, 0, -5.0, 12.0);
        assert_eq!(motion.x, 0);
        assert_eq!(motion.y, MOTION_SCALE);

        let nan = Motion::from_normalized(1, 0, f64::NAN, 0.5);
        assert_eq!(nan.x, 0, "a NaN reached the wire");
    }

    #[test]
    fn the_extremes_map_to_the_full_range() {
        let far = Motion::from_normalized(1, 0, 1.0, 1.0);
        assert_eq!((far.x, far.y), (MOTION_SCALE, MOTION_SCALE));
    }

    #[test]
    fn sequence_comparison_handles_the_ordinary_case() {
        assert!(is_newer(5, 4));
        assert!(!is_newer(4, 5));
        assert!(!is_newer(4, 4), "equal is not newer");
    }

    #[test]
    fn sequence_comparison_survives_wraparound() {
        // The bug this prevents: after the counter wraps, a naive `>` discards
        // every packet for the next 32,000 — minutes of a frozen remote cursor.
        assert!(is_newer(0, u16::MAX), "0 follows 65535");
        assert!(is_newer(5, u16::MAX - 5));
        assert!(!is_newer(u16::MAX - 5, 5));
    }

    #[test]
    fn sequence_comparison_is_antisymmetric_across_the_space() {
        // For any two distinct values exactly one is newer, except at the
        // half-way point where the relation is genuinely ambiguous.
        for reference in [0u16, 1, 30_000, 32_767, 40_000, u16::MAX] {
            for offset in [1u16, 2, 100, 30_000, 32_767] {
                let candidate = reference.wrapping_add(offset);
                assert!(
                    is_newer(candidate, reference),
                    "{candidate} should follow {reference}"
                );
                assert!(
                    !is_newer(reference, candidate),
                    "{reference} should not follow {candidate}"
                );
            }
        }
    }

    #[test]
    fn the_encoded_layout_is_stable() {
        // Guards the wire format against an accidental field reorder, which
        // would silently corrupt every session with an older peer.
        let motion = Motion {
            seq: 0x0201,
            timestamp_us: 0x0605_0403,
            x: 0x0A09_0807,
            y: 0x0E0D_0C0B,
        };
        assert_eq!(
            motion.to_bytes(),
            [
                0x01, 0x00, // kind, flags
                0x01, 0x02, // seq
                0x03, 0x04, 0x05, 0x06, // timestamp
                0x07, 0x08, 0x09, 0x0A, // x
                0x0B, 0x0C, 0x0D, 0x0E, // y
            ]
        );
    }
}
