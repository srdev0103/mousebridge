//! The set of computers this machine is connected to.
//!
//! # Why this is more than a map
//!
//! Going from one peer to several introduces a failure mode that does not exist
//! with one: the machine **currently receiving input** can disappear while the
//! user is typing into it. The cursor is then on a screen that no longer exists,
//! belonging to a device that is no longer reachable, and every keystroke is
//! going nowhere.
//!
//! Recovering from that is the main thing this module does. A disconnection is
//! not a bookkeeping update — it is a decision about where the user's hands are
//! pointing, and it has to be made immediately.
//!
//! # The layout is derived, never stored
//!
//! Screens belong to peers, and peers come and go. Keeping a separate layout in
//! sync with the peer set would mean two sources of truth that can disagree —
//! and the disagreement would show up as a cursor stuck on a screen that is not
//! there. The layout is rebuilt from the connected peers instead.

use mb_net::session::SessionHandle;
use mb_topology::layout::{Layout, LayoutError, PlacedScreen};
use mb_types::{DeviceId, DeviceName, GlobalScreenId};
use std::collections::BTreeMap;

/// How a peer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Connected and answering.
    Connected,
    /// Connected, but heartbeats are going unanswered.
    ///
    /// Still used: dropping a peer over one lost packet would be worse for the
    /// user than a brief warning.
    Degraded {
        /// Consecutive missed probes.
        missed: u32,
    },
}

impl PeerState {
    /// Whether input may still be sent to this peer.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Connected | Self::Degraded { .. })
    }
}

/// A connected computer.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Display name, as advertised.
    pub name: DeviceName,
    /// Current health.
    pub state: PeerState,
    /// The peer's screens, placed in the shared virtual space.
    pub screens: Vec<PlacedScreen>,
    /// How to send to it.
    ///
    /// Optional so the peer set can be exercised without a network.
    handle: Option<SessionHandle>,
}

impl Peer {
    /// The session handle, if this peer has one.
    #[must_use]
    pub const fn handle(&self) -> Option<&SessionHandle> {
        self.handle.as_ref()
    }
}

/// Why control was taken back from a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimReason {
    /// The peer disconnected while it was receiving input.
    PeerLost,
    /// The peer's screens were removed from the layout.
    ScreensGone,
}

/// What the caller must do after a change to the peer set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerChange {
    /// Nothing beyond rebuilding the layout.
    LayoutOnly,
    /// Control must return to this machine immediately.
    ///
    /// The user is typing into a computer that is no longer there. The caller
    /// must route input locally again, re-place the cursor, and release
    /// everything it believed the departed peer was holding — otherwise a
    /// modifier stays down on a machine nobody can reach.
    ReclaimControl {
        /// The device that was receiving input.
        from: DeviceId,
        /// Why.
        reason: ReclaimReason,
    },
}

/// Errors from peer bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    /// The device is not connected.
    #[error("device {device} is not connected")]
    NotConnected {
        /// The device asked for.
        device: String,
    },
    /// The resulting screen arrangement was not valid.
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

/// Every computer this machine is currently sharing input with.
#[derive(Debug)]
pub struct PeerSet {
    local: DeviceId,
    local_screens: Vec<PlacedScreen>,
    /// Ordered so the device list and the derived layout are stable across
    /// rebuilds. A list that reshuffles on every reconnect is unusable.
    peers: BTreeMap<DeviceId, Peer>,
    /// Which device is currently receiving input, if not this one.
    active: Option<DeviceId>,
}

impl PeerSet {
    /// Builds a set containing only this machine.
    #[must_use]
    pub fn new(local: DeviceId, local_screens: Vec<PlacedScreen>) -> Self {
        Self {
            local,
            local_screens,
            peers: BTreeMap::new(),
            active: None,
        }
    }

    /// This machine's identity.
    #[must_use]
    pub const fn local(&self) -> DeviceId {
        self.local
    }

    /// The device currently receiving input, or `None` for this one.
    #[must_use]
    pub const fn active(&self) -> Option<DeviceId> {
        self.active
    }

    /// Connected peers, in a stable order.
    pub fn peers(&self) -> impl Iterator<Item = (&DeviceId, &Peer)> {
        self.peers.iter()
    }

    /// Number of connected peers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// True when nothing is connected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Looks up a peer.
    #[must_use]
    pub fn get(&self, device: DeviceId) -> Option<&Peer> {
        self.peers.get(&device)
    }

    /// Records a newly connected peer.
    ///
    /// Replaces any existing entry, which is what a reconnection looks like.
    pub fn connect(
        &mut self,
        device: DeviceId,
        name: DeviceName,
        screens: Vec<PlacedScreen>,
        handle: Option<SessionHandle>,
    ) -> PeerChange {
        self.peers.insert(
            device,
            Peer {
                name,
                state: PeerState::Connected,
                screens,
                handle,
            },
        );
        PeerChange::LayoutOnly
    }

    /// Records a peer going away.
    ///
    /// Returns [`PeerChange::ReclaimControl`] when the departed peer was the one
    /// receiving input, which the caller must act on immediately.
    pub fn disconnect(&mut self, device: DeviceId) -> PeerChange {
        self.peers.remove(&device);

        if self.active == Some(device) {
            // The user is typing into a machine that is no longer there.
            self.active = None;
            return PeerChange::ReclaimControl {
                from: device,
                reason: ReclaimReason::PeerLost,
            };
        }
        PeerChange::LayoutOnly
    }

    /// Updates a peer's health.
    ///
    /// Degradation deliberately does **not** reclaim control: a peer that has
    /// missed a probe or two is usually still there, and yanking the cursor back
    /// mid-sentence is worse than a moment of uncertainty.
    pub fn set_state(&mut self, device: DeviceId, state: PeerState) {
        if let Some(peer) = self.peers.get_mut(&device) {
            peer.state = state;
        }
    }

    /// Replaces a peer's screens.
    ///
    /// Returns [`PeerChange::ReclaimControl`] if the peer was receiving input and
    /// has no screens left — a display unplugged on the other machine.
    pub fn set_screens(&mut self, device: DeviceId, screens: Vec<PlacedScreen>) -> PeerChange {
        let empty = screens.is_empty();
        if let Some(peer) = self.peers.get_mut(&device) {
            peer.screens = screens;
        }

        if empty && self.active == Some(device) {
            self.active = None;
            return PeerChange::ReclaimControl {
                from: device,
                reason: ReclaimReason::ScreensGone,
            };
        }
        PeerChange::LayoutOnly
    }

    /// Replaces this machine's screens.
    pub fn set_local_screens(&mut self, screens: Vec<PlacedScreen>) {
        self.local_screens = screens;
    }

    /// Directs input at a device, or back at this machine.
    ///
    /// # Errors
    ///
    /// [`PeerError::NotConnected`] if the device is unknown or unusable. Refusing
    /// here is what stops a crossing towards a machine that has just gone away:
    /// the cursor stays put rather than disappearing into nothing.
    pub fn set_active(&mut self, device: Option<DeviceId>) -> Result<(), PeerError> {
        match device {
            None => {
                self.active = None;
                Ok(())
            }
            Some(device) if device == self.local => {
                self.active = None;
                Ok(())
            }
            Some(device) => {
                let usable = self
                    .peers
                    .get(&device)
                    .is_some_and(|peer| peer.state.is_usable());
                if !usable {
                    return Err(PeerError::NotConnected {
                        device: device.short(),
                    });
                }
                self.active = Some(device);
                Ok(())
            }
        }
    }

    /// The session handle for the device currently receiving input.
    #[must_use]
    pub fn active_handle(&self) -> Option<&SessionHandle> {
        self.active
            .and_then(|device| self.peers.get(&device))
            .and_then(Peer::handle)
    }

    /// Builds the layout from this machine plus every connected peer.
    ///
    /// # Errors
    ///
    /// [`PeerError::Layout`] if the combined arrangement is invalid — most often
    /// because two devices' screens overlap, which means the saved arrangement
    /// no longer matches reality.
    pub fn layout(&self) -> Result<Layout, PeerError> {
        let mut screens = self.local_screens.clone();
        for peer in self.peers.values() {
            screens.extend(peer.screens.iter().cloned());
        }
        Ok(Layout::new(screens)?)
    }

    /// Which device owns a screen, if any peer does.
    #[must_use]
    pub fn owner_of(&self, screen: GlobalScreenId) -> Option<DeviceId> {
        if self.local_screens.iter().any(|s| s.id == screen) {
            return Some(self.local);
        }
        self.peers
            .iter()
            .find(|(_, peer)| peer.screens.iter().any(|s| s.id == screen))
            .map(|(device, _)| *device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_types::{LogicalRect, Scale, ScreenId};

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn name(text: &str) -> DeviceName {
        DeviceName::new(text).expect("valid")
    }

    fn screen(byte: u8, index: u32, x: f64, w: f64) -> PlacedScreen {
        PlacedScreen {
            id: GlobalScreenId::new(device(byte), ScreenId(index)),
            bounds: LogicalRect::from_parts(x, 0.0, w, 1080.0).expect("valid"),
            scale: Scale::ONE,
        }
    }

    /// Mac in the middle, a PC either side: Mac — Windows — Mac Mini.
    fn chain() -> PeerSet {
        let mut set = PeerSet::new(device(1), vec![screen(1, 0, 0.0, 1920.0)]);
        set.connect(
            device(2),
            name("Windows PC"),
            vec![screen(2, 0, 1920.0, 1920.0)],
            None,
        );
        set.connect(
            device(3),
            name("Mac Mini"),
            vec![screen(3, 0, 3840.0, 1920.0)],
            None,
        );
        set
    }

    #[test]
    fn a_layout_spans_every_connected_machine() {
        let set = chain();
        let layout = set.layout().expect("valid");
        assert_eq!(layout.screens().len(), 3);
        assert_eq!(layout.bounding_box().max_x(), 5760.0);
    }

    #[test]
    fn the_layout_is_stable_across_rebuilds() {
        // A device list that reshuffles on every reconnect is unusable, and an
        // unstable layout would make the topology editor jump around.
        let set = chain();
        assert_eq!(set.layout().expect("valid"), set.layout().expect("valid"));
    }

    #[test]
    fn screens_are_attributed_to_their_owners() {
        let set = chain();
        assert_eq!(
            set.owner_of(GlobalScreenId::new(device(2), ScreenId(0))),
            Some(device(2))
        );
        assert_eq!(
            set.owner_of(GlobalScreenId::new(device(1), ScreenId(0))),
            Some(device(1)),
            "local screens must be attributed too"
        );
        assert_eq!(
            set.owner_of(GlobalScreenId::new(device(9), ScreenId(0))),
            None
        );
    }

    #[test]
    fn input_can_be_directed_at_any_connected_peer() {
        let mut set = chain();
        set.set_active(Some(device(3))).expect("connected");
        assert_eq!(set.active(), Some(device(3)));

        set.set_active(Some(device(2))).expect("connected");
        assert_eq!(set.active(), Some(device(2)));
    }

    #[test]
    fn directing_input_at_the_local_machine_clears_the_active_peer() {
        let mut set = chain();
        set.set_active(Some(device(2))).expect("connected");
        set.set_active(Some(device(1))).expect("local");
        assert_eq!(set.active(), None);
    }

    #[test]
    fn a_crossing_towards_a_departed_machine_is_refused() {
        // Without this the cursor would vanish into a machine that is not there,
        // and every keystroke after it would go nowhere.
        let mut set = chain();
        set.disconnect(device(3));

        let result = set.set_active(Some(device(3)));
        assert!(matches!(result, Err(PeerError::NotConnected { .. })));
        assert_eq!(set.active(), None, "control must not have moved");
    }

    #[test]
    fn losing_the_active_machine_reclaims_control() {
        // The failure mode that only exists with several peers: the user is
        // typing into a computer that disappears mid-sentence.
        let mut set = chain();
        set.set_active(Some(device(2))).expect("connected");

        let change = set.disconnect(device(2));
        assert_eq!(
            change,
            PeerChange::ReclaimControl {
                from: device(2),
                reason: ReclaimReason::PeerLost,
            }
        );
        assert_eq!(set.active(), None, "control must return to this machine");
    }

    #[test]
    fn losing_an_idle_machine_only_changes_the_layout() {
        let mut set = chain();
        set.set_active(Some(device(2))).expect("connected");

        let change = set.disconnect(device(3));
        assert_eq!(change, PeerChange::LayoutOnly);
        assert_eq!(set.active(), Some(device(2)), "control must not move");
    }

    #[test]
    fn the_layout_shrinks_when_a_peer_leaves() {
        let mut set = chain();
        set.disconnect(device(3));

        let layout = set.layout().expect("valid");
        assert_eq!(layout.screens().len(), 2);
        assert_eq!(layout.bounding_box().max_x(), 3840.0);
    }

    #[test]
    fn a_peer_losing_every_screen_reclaims_control() {
        // A display unplugged on the other machine. The cursor is on a screen
        // that no longer exists.
        let mut set = chain();
        set.set_active(Some(device(2))).expect("connected");

        let change = set.set_screens(device(2), vec![]);
        assert_eq!(
            change,
            PeerChange::ReclaimControl {
                from: device(2),
                reason: ReclaimReason::ScreensGone,
            }
        );
        assert_eq!(set.active(), None);
    }

    #[test]
    fn a_degraded_peer_keeps_control() {
        // A peer that has missed a probe or two is usually still there. Yanking
        // the cursor back mid-sentence is worse than a moment of uncertainty.
        let mut set = chain();
        set.set_active(Some(device(2))).expect("connected");
        set.set_state(device(2), PeerState::Degraded { missed: 2 });

        assert_eq!(set.active(), Some(device(2)));
        assert!(set.get(device(2)).expect("present").state.is_usable());
        // And it can still be selected.
        assert!(set.set_active(Some(device(2))).is_ok());
    }

    #[test]
    fn reconnecting_replaces_the_previous_entry() {
        let mut set = chain();
        set.connect(
            device(2),
            name("Windows PC renamed"),
            vec![screen(2, 0, 1920.0, 1920.0)],
            None,
        );

        assert_eq!(set.len(), 2, "a reconnection must not duplicate the peer");
        assert_eq!(
            set.get(device(2)).expect("present").name.as_str(),
            "Windows PC renamed"
        );
    }

    #[test]
    fn overlapping_screens_are_reported_rather_than_silently_accepted() {
        // Two devices claiming the same region means the saved arrangement no
        // longer matches reality, and a cursor position inside the overlap would
        // belong to two machines at once.
        let mut set = PeerSet::new(device(1), vec![screen(1, 0, 0.0, 1920.0)]);
        set.connect(
            device(2),
            name("Overlapping"),
            vec![screen(2, 0, 1000.0, 1920.0)],
            None,
        );

        assert!(matches!(set.layout(), Err(PeerError::Layout(_))));
    }

    #[test]
    fn a_machine_alone_still_has_a_valid_layout() {
        let set = PeerSet::new(device(1), vec![screen(1, 0, 0.0, 1920.0)]);
        assert!(set.is_empty());
        assert_eq!(set.layout().expect("valid").screens().len(), 1);
    }

    #[test]
    fn disconnecting_an_unknown_peer_is_harmless() {
        let mut set = chain();
        assert_eq!(set.disconnect(device(9)), PeerChange::LayoutOnly);
        assert_eq!(set.len(), 2);
    }
}
