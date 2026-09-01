//! The UDP socket announcements travel over.
//!
//! # Socket options that are not optional
//!
//! * `SO_REUSEADDR` (and `SO_REUSEPORT` where it exists) — several processes may
//!   need to listen on the discovery port at once: two user accounts on one
//!   machine, or a running instance while a developer starts another. Without it
//!   the second bind fails with "address in use" and discovery appears broken.
//! * `SO_BROADCAST` — sending to a broadcast address is refused outright without
//!   it, and the error surfaces at send time rather than at bind, which makes it
//!   look like a network fault.

use crate::beacon::{Announcement, BeaconError, MAX_BEACON_LEN};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::net::UdpSocket;
use tracing::{debug, trace};

/// Address announcements are broadcast to.
pub const BROADCAST_ADDRESS: Ipv4Addr = Ipv4Addr::BROADCAST;

/// Errors from the discovery socket.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// The socket could not be created or bound.
    #[error("could not bind the discovery socket on port {port}: {detail}")]
    Bind {
        /// Port attempted.
        port: u16,
        /// Underlying cause.
        detail: String,
    },
    /// An announcement could not be sent.
    #[error("could not send an announcement to {to}: {detail}")]
    Send {
        /// Intended destination.
        to: String,
        /// Underlying cause.
        detail: String,
    },
    /// The socket failed while receiving.
    #[error("discovery socket failed while receiving: {detail}")]
    Receive {
        /// Underlying cause.
        detail: String,
    },
    /// An announcement could not be built.
    #[error(transparent)]
    Beacon(#[from] BeaconError),
}

/// A bound UDP socket for sending and receiving announcements.
#[derive(Debug)]
pub struct BeaconSocket {
    socket: UdpSocket,
}

impl BeaconSocket {
    /// Binds the discovery socket on `port`.
    ///
    /// Pass `0` to let the OS choose, which is what tests want and what a second
    /// instance falls back to.
    ///
    /// # Errors
    ///
    /// [`SocketError::Bind`] if the socket could not be created, configured or
    /// bound.
    pub fn bind(port: u16) -> Result<Self, SocketError> {
        let fail = |detail: String| SocketError::Bind { port, detail };

        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| fail(e.to_string()))?;

        socket
            .set_reuse_address(true)
            .map_err(|e| fail(format!("SO_REUSEADDR: {e}")))?;

        // Not available on every platform; where it is missing, SO_REUSEADDR
        // alone is enough for the case that matters.
        #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
        socket
            .set_reuse_port(true)
            .map_err(|e| fail(format!("SO_REUSEPORT: {e}")))?;

        socket
            .set_broadcast(true)
            .map_err(|e| fail(format!("SO_BROADCAST: {e}")))?;

        socket
            .set_nonblocking(true)
            .map_err(|e| fail(format!("O_NONBLOCK: {e}")))?;

        let address = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        socket
            .bind(&address.into())
            .map_err(|e| fail(e.to_string()))?;

        let socket = UdpSocket::from_std(std::net::UdpSocket::from(socket))
            .map_err(|e| fail(e.to_string()))?;

        Ok(Self { socket })
    }

    /// The address actually bound, with any ephemeral port resolved.
    ///
    /// # Errors
    ///
    /// [`SocketError::Bind`] if the address cannot be read back.
    pub fn local_addr(&self) -> Result<SocketAddr, SocketError> {
        self.socket.local_addr().map_err(|e| SocketError::Bind {
            port: 0,
            detail: e.to_string(),
        })
    }

    /// Sends an announcement to a specific address.
    ///
    /// # Errors
    ///
    /// [`SocketError::Send`] if the datagram could not be sent, or
    /// [`SocketError::Beacon`] if it could not be encoded.
    pub async fn announce_to(
        &self,
        announcement: &Announcement,
        to: SocketAddr,
    ) -> Result<(), SocketError> {
        let bytes = announcement.encode()?;
        self.socket
            .send_to(&bytes, to)
            .await
            .map_err(|e| SocketError::Send {
                to: to.to_string(),
                detail: e.to_string(),
            })?;
        trace!(%to, bytes = bytes.len(), "announced");
        Ok(())
    }

    /// Broadcasts an announcement to the local network.
    ///
    /// # Errors
    ///
    /// As [`BeaconSocket::announce_to`].
    pub async fn broadcast(
        &self,
        announcement: &Announcement,
        port: u16,
    ) -> Result<(), SocketError> {
        self.announce_to(announcement, SocketAddr::from((BROADCAST_ADDRESS, port)))
            .await
    }

    /// Waits for the next valid announcement.
    ///
    /// Datagrams that are not announcements are skipped rather than returned as
    /// errors: a shared broadcast port carries other protocols' traffic, and
    /// surfacing each one would drown the caller in noise it cannot act on.
    ///
    /// # Errors
    ///
    /// [`SocketError::Receive`] only when the socket itself fails, which means
    /// discovery is over until it is rebuilt.
    pub async fn recv(&self) -> Result<(Announcement, SocketAddr), SocketError> {
        // One extra byte, so an oversized datagram is visibly oversized rather
        // than silently truncated to exactly the limit and then parsed.
        let mut buffer = [0u8; MAX_BEACON_LEN + 1];

        loop {
            let (len, from) =
                self.socket
                    .recv_from(&mut buffer)
                    .await
                    .map_err(|e| SocketError::Receive {
                        detail: e.to_string(),
                    })?;

            match Announcement::decode(&buffer[..len]) {
                Ok(announcement) => return Ok((announcement, from)),
                Err(BeaconError::NotOurs) => {
                    // Ordinary on a shared port; not worth a log line per packet.
                }
                Err(e) => {
                    // Malformed *and* wearing our magic prefix. Worth noticing:
                    // it is either a version mismatch or someone probing.
                    debug!(%from, error = %e, "discarding a malformed announcement");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_protocol::version::VersionRange;
    use mb_types::{DeviceId, DeviceName, OsKind};
    use std::time::Duration;

    fn announcement(name: &str) -> Announcement {
        Announcement {
            device: DeviceId::from_bytes([3u8; 32]),
            name: DeviceName::new(name).expect("valid"),
            os: OsKind::MacOs,
            port: 25470,
            versions: VersionRange::current(),
        }
    }

    #[tokio::test]
    async fn an_announcement_survives_a_real_socket_round_trip() {
        let listener = BeaconSocket::bind(0).expect("binds");
        let sender = BeaconSocket::bind(0).expect("binds");

        let mut target = listener.local_addr().expect("bound");
        target.set_ip(Ipv4Addr::LOCALHOST.into());

        let sent = announcement("Studio Mac");
        sender.announce_to(&sent, target).await.expect("sends");

        let (received, from) = tokio::time::timeout(Duration::from_secs(5), listener.recv())
            .await
            .expect("announcement arrived")
            .expect("valid");

        assert_eq!(received, sent);
        assert_eq!(from.ip(), Ipv4Addr::LOCALHOST);
    }

    #[tokio::test]
    async fn foreign_traffic_is_skipped_without_stalling_discovery() {
        // A shared broadcast port carries other protocols. Those packets must not
        // surface as errors, and must not prevent the next real announcement
        // from being delivered.
        let listener = BeaconSocket::bind(0).expect("binds");
        let sender = BeaconSocket::bind(0).expect("binds");

        let mut target = listener.local_addr().expect("bound");
        target.set_ip(Ipv4Addr::LOCALHOST.into());

        sender
            .socket
            .send_to(b"GET / HTTP/1.1\r\n\r\n", target)
            .await
            .expect("sends noise");
        sender
            .socket
            .send_to(&[0xFFu8; 900], target)
            .await
            .expect("sends oversized noise");

        let sent = announcement("Real Peer");
        sender.announce_to(&sent, target).await.expect("sends");

        let (received, _) = tokio::time::timeout(Duration::from_secs(5), listener.recv())
            .await
            .expect("the real announcement still arrived")
            .expect("valid");
        assert_eq!(received, sent);
    }

    #[tokio::test]
    async fn two_sockets_can_share_the_discovery_port() {
        // Two user accounts on one machine, or a developer starting a second
        // instance. Without SO_REUSEADDR the second bind fails and discovery
        // looks broken for no visible reason.
        let first = BeaconSocket::bind(0).expect("binds");
        let port = first.local_addr().expect("bound").port();

        let second = BeaconSocket::bind(port);
        assert!(
            second.is_ok(),
            "a second listener could not share the port: {:?}",
            second.err()
        );
    }

    #[tokio::test]
    async fn broadcast_is_permitted_by_the_socket() {
        // Without SO_BROADCAST this fails at send time, which reads as a network
        // fault rather than a configuration mistake.
        let socket = BeaconSocket::bind(0).expect("binds");
        let result = socket.broadcast(&announcement("Mac"), 25471).await;
        assert!(result.is_ok(), "broadcast refused: {:?}", result.err());
    }
}
