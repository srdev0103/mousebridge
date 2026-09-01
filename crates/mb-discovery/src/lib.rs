//! Finding peers on the local network.
//!
//! # Three mechanisms, all shipped
//!
//! Discovery is where software KVMs fail most often in the field, because the
//! mechanisms that work in a lab are routinely broken on real networks.
//!
//! 1. **mDNS-SD** — the pleasant path. No port scanning, works with `.local`.
//! 2. **UDP broadcast** — the fallback, because multicast is filtered far more
//!    often than people expect. AP client isolation, IGMP snooping and
//!    multicast-to-unicast conversion all silently eat mDNS on ordinary consumer
//!    Wi-Fi, and the user sees "no computers found" with no explanation.
//! 3. **Manual address entry** — always available, never removed. It is the only
//!    thing that works across VLANs and on locked-down corporate networks, and
//!    treating it as a fallback for advanced users rather than a first-class
//!    feature is how a product becomes unusable in an office.
//!
//! # What a beacon may contain
//!
//! Announcements are broadcast to every machine on the network, including ones
//! that will never be paired. They therefore carry only what a stranger may
//! safely learn: a display name, an OS family, a port, a supported version
//! range, and a **short fingerprint** — never a full key, and nothing about what
//! this computer is paired with.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod beacon;
pub mod manual;
pub mod mdns;
pub mod registry;
pub mod socket;

pub use beacon::{Announcement, BEACON_PORT, MAX_BEACON_LEN};
pub use manual::parse_peer_address;
pub use mdns::{MdnsError, MdnsResponder, SERVICE_TYPE};
pub use registry::{DiscoveredPeer, PeerRegistry, PeerSource};
pub use socket::{BeaconSocket, SocketError};
