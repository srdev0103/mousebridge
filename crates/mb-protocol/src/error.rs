//! Protocol errors.

/// A failure decoding or encoding protocol data.
///
/// Every variant is reachable from hostile input, so each carries enough detail
/// to diagnose without echoing the payload back into a log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The buffer ended before the value did.
    #[error("truncated {what}: need {needed} bytes, have {available}")]
    Truncated {
        /// What was being decoded.
        what: &'static str,
        /// Bytes required.
        needed: usize,
        /// Bytes present.
        available: usize,
    },

    /// A length prefix exceeded the permitted maximum.
    ///
    /// Kept distinct because it is the classic remote memory-exhaustion vector:
    /// a four-byte prefix claiming four gigabytes costs the sender nothing.
    #[error("frame length {claimed} exceeds the {limit} byte limit")]
    FrameTooLarge {
        /// Length the peer claimed.
        claimed: usize,
        /// Largest length accepted.
        limit: usize,
    },

    /// The message type byte is not one this build knows.
    #[error("unknown message type {found:#04x}")]
    UnknownType {
        /// The unrecognised discriminant.
        found: u8,
    },

    /// The payload did not deserialise.
    #[error("malformed {what} payload")]
    Malformed {
        /// Which message failed.
        what: &'static str,
    },

    /// No protocol version is supported by both peers.
    #[error("no common protocol version: this build supports {ours}, the peer supports {theirs}")]
    VersionMismatch {
        /// Range this build accepts.
        ours: crate::version::VersionRange,
        /// Range the peer advertised.
        theirs: crate::version::VersionRange,
    },

    /// A value was structurally valid but semantically impossible.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Field at fault.
        field: &'static str,
        /// Why it was rejected.
        reason: &'static str,
    },
}
