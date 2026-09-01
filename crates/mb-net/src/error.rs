//! Networking errors.

/// A failure establishing or running a connection.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// The TLS configuration could not be built.
    #[error("could not build the {side} TLS configuration: {detail}")]
    TlsConfig {
        /// Which side failed.
        side: &'static str,
        /// Underlying cause.
        detail: String,
    },

    /// The UDP socket could not be bound.
    #[error("could not bind {addr}: {detail}")]
    Bind {
        /// Address that was attempted.
        addr: String,
        /// Underlying cause.
        detail: String,
    },

    /// The connection attempt failed.
    ///
    /// Includes the case where the peer rejected our certificate, which is by far
    /// the most common cause on a healthy network and is what the UI should
    /// explain first.
    #[error("could not connect to {addr}: {detail}")]
    Connect {
        /// Peer address.
        addr: String,
        /// Underlying cause.
        detail: String,
    },

    /// An established connection failed.
    #[error("connection lost: {detail}")]
    ConnectionLost {
        /// Underlying cause.
        detail: String,
    },

    /// A stream operation failed.
    #[error("{operation} failed: {detail}")]
    Stream {
        /// What was attempted.
        operation: &'static str,
        /// Underlying cause.
        detail: String,
    },

    /// The peer violated the protocol.
    #[error(transparent)]
    Protocol(#[from] mb_protocol::ProtocolError),

    /// The peer is not trusted, or its identity did not match.
    #[error(transparent)]
    Security(#[from] mb_security::SecurityError),

    /// The peer did not complete the handshake in time.
    ///
    /// Distinct from a generic connection failure because it is what a port
    /// scanner or a half-open connection looks like, and it must not be retried
    /// as aggressively as a transient network fault.
    #[error("the peer did not finish the handshake within {seconds}s")]
    HandshakeTimeout {
        /// The budget that elapsed.
        seconds: u64,
    },
}
