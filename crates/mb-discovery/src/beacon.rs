//! UDP announcements.
//!
//! The fallback for networks where multicast does not survive. A short datagram
//! is broadcast periodically; peers that hear it learn where to connect.
//!
//! # What this reveals, and to whom
//!
//! Every machine on the network sees these, including ones that will never be
//! paired. An announcement therefore carries only what a stranger may safely
//! learn: a display name, an OS family, a port, an accepted version range, and
//! the device's public identity fingerprint.
//!
//! That fingerprint is a **stable public identifier**, comparable to a hostname:
//! it lets anyone on the same network recognise this machine again. It is
//! included because peers need it to deduplicate announcements arriving through
//! several mechanisms at once. Users who would rather not advertise can turn
//! discovery off — `network.discovery_enabled` — and use manual addressing,
//! which is why that setting exists.
//!
//! What is never included: key material, the set of devices this computer trusts,
//! the logged-in user, or anything about what is on screen.

use mb_protocol::version::VersionRange;
use mb_types::{DeviceId, DeviceName, OsKind};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Port announcements are sent to and listened for.
///
/// Distinct from the transport port so a machine can listen for peers without
/// exposing its QUIC endpoint, and so a stray beacon never reaches the transport
/// parser.
pub const BEACON_PORT: u16 = 25471;

/// Four-byte magic prefix.
///
/// Broadcast ports attract unrelated traffic. Rejecting on the first four bytes
/// keeps other protocols' packets out of the deserialiser entirely.
pub const MAGIC: [u8; 4] = *b"MBRD";

/// Largest announcement accepted.
///
/// Announcements are a couple of hundred bytes. The cap is generous enough for
/// a long device name and small enough that a flood of maximum-size packets
/// costs nothing to discard.
pub const MAX_BEACON_LEN: usize = 512;

/// What a peer broadcasts about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    /// Public identity fingerprint.
    ///
    /// **Unverified on receipt.** Anyone can broadcast any value. It is used to
    /// group announcements, never to make a trust decision — that happens during
    /// the TLS handshake, against a pinned certificate.
    pub device: DeviceId,
    /// Display name.
    pub name: DeviceName,
    /// Operating system family.
    pub os: OsKind,
    /// Port the sender's QUIC endpoint is listening on.
    pub port: u16,
    /// Protocol versions the sender accepts.
    ///
    /// Included so an incompatible peer can be shown as "needs updating" in the
    /// device list, rather than appearing normal and failing at connect time.
    pub versions: VersionRange,
}

/// Errors from encoding or decoding an announcement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BeaconError {
    /// The datagram did not begin with [`MAGIC`].
    ///
    /// Expected and common on a shared broadcast port; not worth logging.
    #[error("not a MouseBridge announcement")]
    NotOurs,
    /// The datagram exceeded [`MAX_BEACON_LEN`].
    #[error("announcement of {size} bytes exceeds the {MAX_BEACON_LEN} byte limit")]
    TooLarge {
        /// Size received.
        size: usize,
    },
    /// The payload did not deserialise.
    #[error("malformed announcement")]
    Malformed,
}

impl Announcement {
    /// Encodes this announcement for transmission.
    ///
    /// # Errors
    ///
    /// [`BeaconError::Malformed`] if serialisation fails, or
    /// [`BeaconError::TooLarge`] if the result exceeds the cap — which in
    /// practice means a device name far longer than validation allows.
    pub fn encode(&self) -> Result<Vec<u8>, BeaconError> {
        let body = postcard::to_stdvec(self).map_err(|_| BeaconError::Malformed)?;
        let total = MAGIC.len() + body.len();
        if total > MAX_BEACON_LEN {
            return Err(BeaconError::TooLarge { size: total });
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decodes an announcement received from the network.
    ///
    /// # Errors
    ///
    /// See [`BeaconError`]. All input here is untrusted and unauthenticated;
    /// nothing decoded may be used for a trust decision.
    pub fn decode(datagram: &[u8]) -> Result<Self, BeaconError> {
        if datagram.len() > MAX_BEACON_LEN {
            return Err(BeaconError::TooLarge {
                size: datagram.len(),
            });
        }
        let body = datagram.strip_prefix(&MAGIC).ok_or(BeaconError::NotOurs)?;

        let (announcement, rest) =
            postcard::take_from_bytes::<Self>(body).map_err(|_| BeaconError::Malformed)?;
        if !rest.is_empty() {
            return Err(BeaconError::Malformed);
        }
        announcement.validate()?;
        Ok(announcement)
    }

    /// Rejects values that are structurally valid but semantically impossible.
    fn validate(&self) -> Result<(), BeaconError> {
        // Port 0 means "let the OS choose", which cannot be connected to.
        if self.port == 0 {
            return Err(BeaconError::Malformed);
        }
        if self.versions.min > self.versions.max {
            return Err(BeaconError::Malformed);
        }
        Ok(())
    }

    /// Whether this build can talk to the announcing peer.
    ///
    /// Lets the UI show "needs updating" instead of an entry that looks fine and
    /// fails only when the user tries to use it.
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        VersionRange::current().negotiate(self.versions).is_ok()
    }

    /// The address to connect to, given where the datagram came from.
    ///
    /// The port is taken from the announcement, the address from the packet
    /// itself. Trusting a sender-supplied address would let any machine on the
    /// network redirect connections to a host of its choosing.
    #[must_use]
    pub fn connect_address(&self, from: SocketAddr) -> SocketAddr {
        SocketAddr::new(from.ip(), self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_protocol::version::Version;
    use std::net::{IpAddr, Ipv4Addr};

    fn sample() -> Announcement {
        Announcement {
            device: DeviceId::from_bytes([5u8; 32]),
            name: DeviceName::new("Studio Mac").expect("valid"),
            os: OsKind::MacOs,
            port: 25470,
            versions: VersionRange::current(),
        }
    }

    #[test]
    fn round_trips() {
        let announcement = sample();
        let bytes = announcement.encode().expect("encodes");
        assert_eq!(Announcement::decode(&bytes), Ok(announcement));
    }

    #[test]
    fn a_real_announcement_is_small() {
        // Broadcast to every machine on the network, several times a minute.
        let bytes = sample().encode().expect("encodes");
        assert!(
            bytes.len() < 128,
            "announcement grew to {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn foreign_traffic_is_rejected_on_the_magic_prefix() {
        // A shared broadcast port attracts unrelated protocols. They must be
        // discarded before reaching the deserialiser.
        assert_eq!(
            Announcement::decode(b"HTTP/1.1 200 OK"),
            Err(BeaconError::NotOurs)
        );
        assert_eq!(Announcement::decode(b""), Err(BeaconError::NotOurs));
        assert_eq!(Announcement::decode(b"MBR"), Err(BeaconError::NotOurs));
    }

    #[test]
    fn an_oversized_datagram_is_rejected_before_parsing() {
        let huge = vec![0u8; MAX_BEACON_LEN + 1];
        assert!(matches!(
            Announcement::decode(&huge),
            Err(BeaconError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_truncated_announcement_is_rejected() {
        let bytes = sample().encode().expect("encodes");
        for len in MAGIC.len()..bytes.len() {
            assert!(
                Announcement::decode(&bytes[..len]).is_err(),
                "accepted a {len}-byte truncation"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = sample().encode().expect("encodes");
        bytes.push(0xFF);
        assert_eq!(Announcement::decode(&bytes), Err(BeaconError::Malformed));
    }

    #[test]
    fn arbitrary_bytes_after_the_magic_never_panic() {
        for seed in 0u16..1000 {
            let mut datagram = MAGIC.to_vec();
            datagram.extend((0..24).map(|i| (seed.wrapping_mul(37) ^ i) as u8));
            let _ = Announcement::decode(&datagram);
        }
    }

    #[test]
    fn a_zero_port_is_rejected() {
        // Port 0 means "let the OS choose"; it cannot be a destination.
        let mut bad = sample();
        bad.port = 0;
        let bytes = bad.encode().expect("encodes");
        assert_eq!(Announcement::decode(&bytes), Err(BeaconError::Malformed));
    }

    #[test]
    fn an_inverted_version_range_is_rejected() {
        let mut bad = sample();
        bad.versions = VersionRange {
            min: Version(9),
            max: Version(2),
        };
        let bytes = bad.encode().expect("encodes");
        assert_eq!(Announcement::decode(&bytes), Err(BeaconError::Malformed));
    }

    #[test]
    fn incompatible_peers_are_identifiable_before_connecting() {
        // So the list can say "needs updating" rather than showing an entry that
        // looks fine and fails only when used.
        assert!(sample().is_compatible());

        let mut future = sample();
        future.versions = VersionRange {
            min: Version(900),
            max: Version(999),
        };
        assert!(!future.is_compatible());
    }

    #[test]
    fn the_connect_address_comes_from_the_packet_not_the_payload() {
        // Trusting a sender-supplied address would let any machine on the network
        // redirect connections to a host of its choosing.
        let announcement = sample();
        let from = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 40000);
        let target = announcement.connect_address(from);

        assert_eq!(target.ip(), from.ip(), "address must come from the packet");
        assert_eq!(target.port(), 25470, "port comes from the announcement");
    }
}
