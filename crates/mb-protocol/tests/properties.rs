//! Property tests for the wire protocol.
//!
//! Everything decoded here arrives from the network, so the properties asserted
//! are the ones a hostile peer would try to violate: no panic, no unbounded
//! allocation, no reading past the end of a buffer, and no claim to have consumed
//! more than was provided.

// Integration tests are a separate crate, so the library's test-only allow
// does not reach here. Panicking helpers are the clearest way to express a
// fixture that cannot fail.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_protocol::frame::{FrameDecode, MAX_FRAME_LEN, decode_frame, encode_frame};
use mb_protocol::message::{ControlMessage, InputMessage, decode, encode};
use mb_protocol::motion::{MOTION_LEN, Motion, is_newer};
use mb_protocol::version::{Version, VersionRange};
use proptest::prelude::*;

proptest! {
    /// Arbitrary bytes must never panic a decoder.
    ///
    /// This is the single most important property in the crate: a panic here is
    /// a remote denial of service that costs the attacker one UDP packet.
    #[test]
    fn arbitrary_bytes_never_panic_any_decoder(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = Motion::decode(&bytes);
        let _ = decode_frame(&bytes);
        let _ = decode::<ControlMessage>(&bytes, "control");
        let _ = decode::<InputMessage>(&bytes, "input");
    }

    /// Motion survives any value the encoder can produce.
    #[test]
    fn motion_round_trips(
        seq in any::<u16>(),
        timestamp_us in any::<u32>(),
        x in any::<i32>(),
        y in any::<i32>(),
    ) {
        let motion = Motion { seq, timestamp_us, x, y };
        let bytes = motion.to_bytes();
        prop_assert_eq!(bytes.len(), MOTION_LEN);
        prop_assert_eq!(Motion::decode(&bytes), Ok(motion));
    }

    /// Any prefix of a valid datagram is rejected rather than misread.
    #[test]
    fn truncated_motion_is_always_rejected(
        seq in any::<u16>(),
        x in any::<i32>(),
        y in any::<i32>(),
        cut in 0usize..MOTION_LEN,
    ) {
        let bytes = Motion { seq, timestamp_us: 0, x, y }.to_bytes();
        prop_assert!(Motion::decode(&bytes[..cut]).is_err());
    }

    /// Framing round-trips, and never over-consumes.
    #[test]
    fn frames_round_trip(payload in prop::collection::vec(any::<u8>(), 0..4096)) {
        let mut buf = Vec::new();
        encode_frame(&payload, &mut buf).expect("within the limit");

        match decode_frame(&buf) {
            Ok(FrameDecode::Complete { payload: got, consumed }) => {
                prop_assert_eq!(got, payload.as_slice());
                prop_assert_eq!(consumed, buf.len());
            }
            other => prop_assert!(false, "expected a complete frame, got {:?}", other),
        }
    }

    /// A decoder must never report consuming more than it was given.
    ///
    /// Callers advance a buffer by `consumed`; over-reporting would slice out of
    /// bounds or silently skip unparsed data.
    #[test]
    fn frame_decoding_never_over_consumes(bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        if let Ok(FrameDecode::Complete { consumed, payload }) = decode_frame(&bytes) {
            prop_assert!(consumed <= bytes.len());
            prop_assert!(payload.len() < consumed, "payload must exclude the prefix");
        }
    }

    /// An incomplete frame always asks for strictly more than is present.
    ///
    /// Returning a `needed` that is already satisfied would spin a read loop.
    #[test]
    fn incomplete_frames_always_request_progress(
        bytes in prop::collection::vec(any::<u8>(), 0..1024)
    ) {
        if let Ok(FrameDecode::Incomplete { needed }) = decode_frame(&bytes) {
            prop_assert!(needed > bytes.len(), "needed {} with {} available", needed, bytes.len());
            prop_assert!(needed <= MAX_FRAME_LEN + 4);
        }
    }

    /// Any four-byte prefix is either rejected or asks for a bounded amount.
    ///
    /// The memory-exhaustion guard: no length a peer can write may translate into
    /// an unbounded request.
    #[test]
    fn no_length_prefix_can_request_unbounded_memory(len in any::<u32>()) {
        match decode_frame(&len.to_le_bytes()) {
            Ok(FrameDecode::Incomplete { needed }) => {
                prop_assert!(needed <= MAX_FRAME_LEN + 4, "requested {}", needed);
            }
            Err(_) => {}
            Ok(other) => prop_assert!(false, "unexpected {:?}", other),
        }
    }

    /// Encoded control messages survive a round trip through framing.
    #[test]
    fn input_messages_round_trip_through_frames(
        pressed in any::<bool>(),
        repeat in any::<bool>(),
        usage in any::<u16>(),
    ) {
        let message = InputMessage::Key {
            key: mb_input::KeyCode::keyboard(usage),
            pressed,
            repeat,
        };
        let mut buf = Vec::new();
        encode(&message, "input", &mut buf).expect("encodes");

        match decode_frame(&buf).expect("frames") {
            FrameDecode::Complete { payload, .. } => {
                let back: InputMessage = decode(payload, "input").expect("decodes");
                prop_assert_eq!(back, message);
            }
            other => prop_assert!(false, "expected a complete frame, got {:?}", other),
        }
    }

    /// Version negotiation agrees regardless of which side computes it.
    ///
    /// If the two sides disagreed, each would encode for a different layout and
    /// the session would corrupt silently instead of failing.
    #[test]
    fn version_negotiation_is_symmetric_and_in_range(
        a_min in 1u16..50, a_span in 0u16..50,
        b_min in 1u16..50, b_span in 0u16..50,
    ) {
        let a = VersionRange::new(Version(a_min), Version(a_min + a_span)).expect("valid");
        let b = VersionRange::new(Version(b_min), Version(b_min + b_span)).expect("valid");

        match (a.negotiate(b), b.negotiate(a)) {
            (Ok(x), Ok(y)) => {
                prop_assert_eq!(x, y);
                prop_assert!(a.accepts(x) && b.accepts(x), "{:?} accepted by neither", x);
            }
            (Err(_), Err(_)) => {}
            (x, y) => prop_assert!(false, "asymmetric outcome: {:?} vs {:?}", x, y),
        }
    }

    /// Sequence comparison is antisymmetric everywhere except the antipode.
    ///
    /// The wraparound bug this guards would freeze the remote cursor for minutes
    /// each time the counter rolled over.
    #[test]
    fn sequence_ordering_is_antisymmetric(reference in any::<u16>(), offset in 1u16..32_768) {
        let candidate = reference.wrapping_add(offset);
        prop_assert!(is_newer(candidate, reference));
        prop_assert!(!is_newer(reference, candidate));
    }
}
