//! The set of peers currently believed to be reachable.
//!
//! Pure logic with the clock passed in, so ageing and deduplication are tested
//! without waiting on wall time.

use mb_types::{DeviceId, DeviceName, OsKind};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How a peer was found.
///
/// Ordered by preference. When the same device is seen through several
/// mechanisms the most reliable one wins, because a manually entered address is
/// something the user asserted and must not be overwritten by an automatic
/// discovery that may be reporting a stale or wrong interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PeerSource {
    /// Heard on the UDP broadcast fallback.
    Broadcast,
    /// Advertised over mDNS-SD.
    Mdns,
    /// Entered by the user.
    Manual,
}

/// How long a peer survives without being heard from again.
///
/// Beacons go out roughly every few seconds, so this tolerates several
/// consecutive losses before a machine disappears from the list. Removing a
/// device the moment one packet is lost would make the list flicker, which reads
/// as unreliability even when everything is working.
pub const DEFAULT_EXPIRY: Duration = Duration::from_secs(30);

/// A peer seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    /// Identity claimed in the announcement.
    ///
    /// **Unverified.** Anyone can broadcast any identity; this is only useful
    /// for grouping announcements and for showing something in the list. Trust
    /// is established by the certificate pinning in the TLS handshake, never by
    /// what a beacon claims.
    pub device: DeviceId,
    /// Display name from the announcement.
    pub name: DeviceName,
    /// Operating system family.
    pub os: OsKind,
    /// Where to connect.
    pub address: SocketAddr,
    /// How this peer was found.
    pub source: PeerSource,
    /// When it was last heard from.
    pub last_seen: Instant,
}

/// Tracks reachable peers, deduplicating across discovery mechanisms.
#[derive(Debug)]
pub struct PeerRegistry {
    peers: HashMap<DeviceId, DiscoveredPeer>,
    expiry: Duration,
}

impl Default for PeerRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_EXPIRY)
    }
}

impl PeerRegistry {
    /// Builds a registry with a given expiry window.
    #[must_use]
    pub fn new(expiry: Duration) -> Self {
        Self {
            peers: HashMap::new(),
            expiry,
        }
    }

    /// Records a sighting.
    ///
    /// Returns true if this is a peer that was not previously known, so the
    /// caller can decide whether to show a notification.
    pub fn observe(&mut self, peer: DiscoveredPeer) -> bool {
        match self.peers.get_mut(&peer.device) {
            Some(existing) => {
                // A less reliable mechanism must not overwrite a better one's
                // address. A user-entered address in particular is an assertion
                // that must survive an automatic discovery reporting a stale or
                // wrong interface.
                if peer.source >= existing.source {
                    existing.address = peer.address;
                    existing.source = peer.source;
                }
                existing.name = peer.name;
                existing.os = peer.os;
                existing.last_seen = peer.last_seen;
                false
            }
            None => {
                self.peers.insert(peer.device, peer);
                true
            }
        }
    }

    /// Removes peers not heard from within the expiry window.
    ///
    /// Returns the devices that were dropped. Manually entered peers are never
    /// aged out: the user said this machine exists, and hiding it because it is
    /// currently asleep would be second-guessing them.
    pub fn expire(&mut self, now: Instant) -> Vec<DeviceId> {
        let expiry = self.expiry;
        let mut dropped = Vec::new();
        self.peers.retain(|device, peer| {
            let keep =
                peer.source == PeerSource::Manual || now.duration_since(peer.last_seen) < expiry;
            if !keep {
                dropped.push(*device);
            }
            keep
        });
        dropped
    }

    /// Forgets a peer.
    pub fn remove(&mut self, device: DeviceId) -> bool {
        self.peers.remove(&device).is_some()
    }

    /// Returns a peer.
    #[must_use]
    pub fn get(&self, device: DeviceId) -> Option<&DiscoveredPeer> {
        self.peers.get(&device)
    }

    /// Number of known peers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// True when nothing has been found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Peers in a stable order, sorted by name then identity.
    ///
    /// Sorted because the underlying map is not, and a device list that
    /// reshuffles on every refresh is unusable.
    #[must_use]
    pub fn sorted(&self) -> Vec<&DiscoveredPeer> {
        let mut peers: Vec<&DiscoveredPeer> = self.peers.values().collect();
        peers.sort_by(|a, b| {
            a.name
                .as_str()
                .cmp(b.name.as_str())
                .then_with(|| a.device.cmp(&b.device))
        });
        peers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn peer(byte: u8, name: &str, source: PeerSource, at: Instant) -> DiscoveredPeer {
        DiscoveredPeer {
            device: device(byte),
            name: DeviceName::new(name).expect("valid"),
            os: OsKind::MacOs,
            address: format!("192.168.1.{byte}:25470").parse().expect("valid"),
            source,
            last_seen: at,
        }
    }

    #[test]
    fn a_new_peer_is_reported_as_new() {
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        assert!(registry.observe(peer(1, "Mac", PeerSource::Mdns, now)));
        assert!(!registry.observe(peer(1, "Mac", PeerSource::Mdns, now)));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_same_device_seen_twice_is_not_duplicated() {
        // A device announcing over both mDNS and broadcast must appear once.
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        registry.observe(peer(1, "Mac", PeerSource::Mdns, now));
        registry.observe(peer(1, "Mac", PeerSource::Broadcast, now));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_manual_address_is_not_overwritten_by_discovery() {
        // The user asserted this address. An automatic mechanism reporting a
        // stale or wrong interface must not silently replace it.
        let now = Instant::now();
        let mut registry = PeerRegistry::default();

        let mut manual = peer(1, "Mac", PeerSource::Manual, now);
        manual.address = "10.0.0.5:25470".parse().expect("valid");
        registry.observe(manual);

        registry.observe(peer(1, "Mac", PeerSource::Mdns, now));

        let stored = registry.get(device(1)).expect("present");
        assert_eq!(stored.address.to_string(), "10.0.0.5:25470");
        assert_eq!(stored.source, PeerSource::Manual);
    }

    #[test]
    fn a_better_source_does_replace_a_worse_one() {
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        registry.observe(peer(1, "Mac", PeerSource::Broadcast, now));
        registry.observe(peer(1, "Mac", PeerSource::Mdns, now));
        assert_eq!(
            registry.get(device(1)).expect("present").source,
            PeerSource::Mdns
        );
    }

    #[test]
    fn a_rename_is_always_picked_up() {
        // Even from a less preferred source: the name is cosmetic, and showing a
        // stale one is worse than accepting an update.
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        registry.observe(peer(1, "Old Name", PeerSource::Manual, now));
        registry.observe(peer(1, "New Name", PeerSource::Broadcast, now));
        assert_eq!(
            registry.get(device(1)).expect("present").name.as_str(),
            "New Name"
        );
    }

    #[test]
    fn silent_peers_expire() {
        let start = Instant::now();
        let mut registry = PeerRegistry::new(Duration::from_secs(30));
        registry.observe(peer(1, "Mac", PeerSource::Mdns, start));

        assert!(registry.expire(start + Duration::from_secs(29)).is_empty());
        assert_eq!(registry.len(), 1);

        let dropped = registry.expire(start + Duration::from_secs(31));
        assert_eq!(dropped, vec![device(1)]);
        assert!(registry.is_empty());
    }

    #[test]
    fn manual_peers_never_expire() {
        // The user said this machine exists. Hiding it because it is asleep
        // would be second-guessing them, and they would have to re-add it.
        let start = Instant::now();
        let mut registry = PeerRegistry::new(Duration::from_secs(30));
        registry.observe(peer(1, "Office PC", PeerSource::Manual, start));

        assert!(
            registry
                .expire(start + Duration::from_secs(86_400))
                .is_empty()
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_re_announcement_refreshes_the_expiry() {
        let start = Instant::now();
        let mut registry = PeerRegistry::new(Duration::from_secs(30));
        registry.observe(peer(1, "Mac", PeerSource::Mdns, start));
        registry.observe(peer(
            1,
            "Mac",
            PeerSource::Mdns,
            start + Duration::from_secs(20),
        ));

        assert!(registry.expire(start + Duration::from_secs(40)).is_empty());
    }

    #[test]
    fn listing_is_stable_across_calls() {
        // A device list that reshuffles on every refresh is unusable.
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        for (byte, name) in [(3, "Charlie"), (1, "Alpha"), (2, "Bravo")] {
            registry.observe(peer(byte, name, PeerSource::Mdns, now));
        }
        let names: Vec<&str> = registry.sorted().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Bravo", "Charlie"]);
        assert_eq!(registry.sorted(), registry.sorted());
    }

    #[test]
    fn removing_a_peer_works_once() {
        let now = Instant::now();
        let mut registry = PeerRegistry::default();
        registry.observe(peer(1, "Mac", PeerSource::Mdns, now));
        assert!(registry.remove(device(1)));
        assert!(!registry.remove(device(1)));
        assert!(registry.is_empty());
    }
}
