//! QUIC transport for MouseBridge.
//!
//! # Why QUIC
//!
//! Four requirements decided this, none of them "QUIC is modern":
//!
//! 1. **File transfer must not stall the pointer.** On a single TCP connection a
//!    large transfer head-of-line blocks input behind it. QUIC gives independent
//!    streams over one connection, one handshake and one firewall hole.
//! 2. **Network changes must survive.** QUIC connection migration carries a
//!    session across a Wi-Fi to Ethernet switch or a DHCP change. TCP cannot.
//! 3. **Motion wants unreliable delivery.** A retransmitted 40 ms-old pointer
//!    position is a visible stutter, not a correction. Datagrams drop stale
//!    motion instead of blocking behind it, and each carries an absolute
//!    position so loss self-heals.
//! 4. **TLS 1.3 is not optional.** There is no plaintext mode to retrofit
//!    encryption onto later, which is why security lands in this milestone
//!    rather than at milestone 10.
//!
//! The cost is a heavier dependency than a raw socket, and UDP being blocked on
//! some hostile networks. Manual peer addressing is the documented fallback.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod endpoint;
pub mod error;
pub mod heartbeat;
pub mod tls;

pub use endpoint::{Endpoint, PeerConnection};
pub use error::NetError;
pub use heartbeat::{HeartbeatMonitor, Liveness};
pub use tls::ALPN;
