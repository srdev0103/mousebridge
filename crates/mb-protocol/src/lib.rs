//! The MouseBridge wire protocol.
//!
//! # Two encodings, on purpose
//!
//! **Pointer motion** uses a hand-written fixed-layout binary encoding — see
//! [`motion`]. It is sixteen bytes with no length prefix, no varints and no
//! allocation, because it is produced and consumed on the input path where a
//! serialiser's overhead is latency the user can feel.
//!
//! **Everything else** uses `serde` + `postcard` — see [`message`]. Buttons,
//! keys, clipboard and file transfer are low-rate and structurally varied, and
//! hand-rolling twenty message types would trade a real risk of encoding bugs for
//! a saving that does not exist off the hot path.
//!
//! # Delivery
//!
//! | Channel | Delivery | Carries |
//! |---|---|---|
//! | QUIC datagram | unreliable, unordered | [`motion`] only |
//! | Control stream | reliable, ordered | handshake, heartbeat, cursor handoff |
//! | Input stream | reliable, ordered | buttons, keys, wheel, release-all |
//!
//! Motion tolerates loss because each packet carries an absolute position that
//! supersedes the last: a dropped packet costs one frame. A key or button
//! transition never tolerates loss — a lost key-up is a key stuck down on a
//! machine the user has walked away from — so those go on a reliable stream.
//!
//! # Untrusted input
//!
//! Every decoder here parses data from the network. Decoding must never panic,
//! never allocate on an attacker-chosen length, and never loop unboundedly. The
//! property tests in `tests/` assert this over arbitrary byte strings.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod chunking;
pub mod error;
pub mod frame;
pub mod message;
pub mod motion;
pub mod version;

pub use chunking::{CHUNK_SIZE, Chunk, ChunkedOffer, Reassembler, split};
pub use error::ProtocolError;
pub use frame::{MAX_FRAME_LEN, decode_frame, encode_frame};
pub use message::{ControlMessage, Hello, InputMessage, ScreenDescriptor, SessionMessage};
pub use motion::{MOTION_LEN, Motion};
pub use version::{PROTOCOL_VERSION, Version, VersionRange};
