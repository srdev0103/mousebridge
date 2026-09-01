//! A QUIC endpoint that both accepts and initiates connections.
//!
//! One socket serves both roles. That is what makes the peer-symmetric design
//! practical: every machine can be the one holding the keyboard, so every machine
//! must be able to connect *and* be connected to, without a second port or a
//! second firewall rule.

use crate::error::NetError;
use crate::tls;
use mb_protocol::frame::{FrameDecode, decode_frame};
use mb_protocol::message::{self, ControlMessage, Hello, SessionMessage};
use mb_protocol::version::{Version, VersionRange};
use mb_security::Identity;
use mb_security::pinning::SharedTrust;
use mb_types::{DeviceId, DeviceName};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// How long a peer has to complete the application handshake.
///
/// Deliberately short. A connection that has completed TLS but sends no `Hello`
/// is a port scanner, a stalled peer, or a half-open socket, and holding
/// resources for it is exactly what an attacker would want.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often to probe an otherwise idle connection.
///
/// Also what keeps NAT and firewall state alive on the path.
const KEEP_ALIVE: Duration = Duration::from_secs(1);

/// How long without any packet before the connection is considered dead.
///
/// Three missed keep-alives. Short because the cost of being wrong is bounded —
/// reconnection is automatic — while the cost of being slow is the user watching
/// a dead cursor.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Server name presented in the TLS handshake.
///
/// Not used for verification — identity comes from the pinned certificate — but
/// QUIC requires a name, and a fixed internal one is clearer in a capture than a
/// hostname implying it means something.
const SERVER_NAME: &str = "mousebridge.local";

/// A connected, authenticated peer.
///
/// Carries the control stream opened during the handshake. Keeping it rather
/// than reopening one later matters: a second stream would need its own
/// authentication story, and there would be a window in which the peer is
/// connected but not yet reachable.
#[derive(Debug)]
pub struct PeerConnection {
    connection: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    device: DeviceId,
    name: DeviceName,
    version: Version,
    certificate: rustls::pki_types::CertificateDer<'static>,
}

impl PeerConnection {
    /// The peer's verified identity.
    ///
    /// Established by certificate pinning during the TLS handshake, not by
    /// anything the peer asserted about itself.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.device
    }

    /// The peer's display name, as it advertised it.
    #[must_use]
    pub const fn name(&self) -> &DeviceName {
        &self.name
    }

    /// The protocol version both peers agreed on.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The peer's address.
    #[must_use]
    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// The underlying QUIC connection.
    #[must_use]
    pub const fn quic(&self) -> &quinn::Connection {
        &self.connection
    }

    /// The certificate the peer presented and proved it holds the key for.
    ///
    /// Needed for pairing: the verification code is derived from both
    /// certificates, and taking this from the authenticated handshake rather
    /// than from a message means a peer cannot show the user a code for a
    /// certificate it does not actually hold.
    #[must_use]
    pub const fn certificate(&self) -> &rustls::pki_types::CertificateDer<'static> {
        &self.certificate
    }

    /// Splits the connection into its parts, for the session layer.
    #[must_use]
    pub fn into_parts(self) -> (quinn::Connection, quinn::SendStream, quinn::RecvStream) {
        (self.connection, self.send, self.recv)
    }

    /// Closes the connection.
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"closing");
    }
}

/// A QUIC endpoint bound to one UDP socket.
#[derive(Debug, Clone)]
pub struct Endpoint {
    inner: quinn::Endpoint,
    identity: Arc<Identity>,
    local_name: DeviceName,
}

impl Endpoint {
    /// Binds an endpoint that can both accept and initiate connections.
    ///
    /// # Errors
    ///
    /// [`NetError::TlsConfig`] if the identity is unusable, or
    /// [`NetError::Bind`] if the socket could not be bound — most often because
    /// another instance is already running.
    pub fn bind(
        addr: SocketAddr,
        identity: Arc<Identity>,
        local_name: DeviceName,
        trust: SharedTrust,
    ) -> Result<Self, NetError> {
        let server_crypto = tls::server_config(&identity, Arc::clone(&trust))?;
        let client_crypto = tls::client_config(&identity, trust)?;
        Self::assemble(addr, identity, local_name, server_crypto, client_crypto)
    }

    /// Binds an endpoint that accepts any peer, for pairing only.
    ///
    /// The connections this produces must never carry input. Their sole purpose
    /// is to let two devices that have never met exchange nonces so their users
    /// can compare a verification code.
    ///
    /// # Errors
    ///
    /// As [`Endpoint::bind`].
    pub fn bind_for_pairing(
        addr: SocketAddr,
        identity: Arc<Identity>,
        local_name: DeviceName,
    ) -> Result<Self, NetError> {
        let server_crypto = tls::pairing_server_config(&identity)?;
        let client_crypto = tls::pairing_client_config(&identity)?;
        Self::assemble(addr, identity, local_name, server_crypto, client_crypto)
    }

    fn assemble(
        addr: SocketAddr,
        identity: Arc<Identity>,
        local_name: DeviceName,
        server_crypto: rustls::ServerConfig,
        client_crypto: rustls::ClientConfig,
    ) -> Result<Self, NetError> {
        let server_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
            .map_err(|e| NetError::TlsConfig {
                side: "server",
                detail: e.to_string(),
            })?;
        let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
            .map_err(|e| NetError::TlsConfig {
                side: "client",
                detail: e.to_string(),
            })?;

        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(KEEP_ALIVE));
        transport.max_idle_timeout(Some(IDLE_TIMEOUT.try_into().map_err(|_| {
            NetError::TlsConfig {
                side: "transport",
                detail: "idle timeout out of range".to_owned(),
            }
        })?));
        // Motion travels as datagrams; without a receive buffer QUIC negotiates
        // datagram support off and every pointer packet is silently discarded.
        transport.datagram_receive_buffer_size(Some(64 * 1024));
        transport.datagram_send_buffer_size(64 * 1024);
        let transport = Arc::new(transport);

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
        server_config.transport_config(Arc::clone(&transport));

        let mut client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
        client_config.transport_config(transport);

        let mut inner =
            quinn::Endpoint::server(server_config, addr).map_err(|e| NetError::Bind {
                addr: addr.to_string(),
                detail: e.to_string(),
            })?;
        inner.set_default_client_config(client_config);

        Ok(Self {
            inner,
            identity,
            local_name,
        })
    }

    /// The address actually bound, with any ephemeral port resolved.
    ///
    /// # Errors
    ///
    /// [`NetError::Bind`] if the socket address cannot be read back.
    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        self.inner.local_addr().map_err(|e| NetError::Bind {
            addr: "local".to_owned(),
            detail: e.to_string(),
        })
    }

    /// This device's identity.
    #[must_use]
    pub fn device(&self) -> DeviceId {
        self.identity.device_id()
    }

    /// The underlying QUIC endpoint.
    ///
    /// Exposed so the session layer can open additional streams, and so tests
    /// can exercise transport behaviour that the application handshake would
    /// otherwise hide.
    #[must_use]
    pub const fn quic(&self) -> &quinn::Endpoint {
        &self.inner
    }

    /// Connects to a peer and completes the application handshake.
    ///
    /// # Errors
    ///
    /// [`NetError::Connect`] if the transport handshake failed — which includes
    /// the peer rejecting our certificate, the most common cause on a healthy
    /// network — or [`NetError::HandshakeTimeout`] if the peer accepted the
    /// connection but never identified itself.
    pub async fn connect(&self, addr: SocketAddr) -> Result<PeerConnection, NetError> {
        let connecting = self
            .inner
            .connect(addr, SERVER_NAME)
            .map_err(|e| NetError::Connect {
                addr: addr.to_string(),
                detail: e.to_string(),
            })?;

        let connection = connecting.await.map_err(|e| NetError::Connect {
            addr: addr.to_string(),
            detail: e.to_string(),
        })?;
        debug!(%addr, "transport handshake complete");

        // The initiator speaks first, so the responder never has to guess whether
        // a silent connection is slow or hostile.
        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| NetError::Stream {
            operation: "open control stream",
            detail: e.to_string(),
        })?;

        let hello = self.hello();
        let mut buffer = Vec::new();
        message::encode(
            &SessionMessage::Control(ControlMessage::Hello(hello)),
            "hello",
            &mut buffer,
        )?;
        send.write_all(&buffer)
            .await
            .map_err(|e| NetError::Stream {
                operation: "send hello",
                detail: e.to_string(),
            })?;

        let reply = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_message(&mut recv))
            .await
            .map_err(|_| NetError::HandshakeTimeout {
                seconds: HANDSHAKE_TIMEOUT.as_secs(),
            })??;

        let ControlMessage::HelloAck {
            version,
            device,
            name,
        } = reply
        else {
            return Err(NetError::Protocol(mb_protocol::ProtocolError::Invalid {
                field: "handshake",
                reason: "expected HelloAck",
            }));
        };

        let peer = self.finish(connection, send, recv, device, name, version)?;
        info!(peer = %peer.device(), name = %peer.name(), %version, "connected to peer");
        Ok(peer)
    }

    /// Accepts the next incoming connection and completes the handshake.
    ///
    /// Returns `None` when the endpoint is closed.
    ///
    /// # Errors
    ///
    /// As [`Endpoint::connect`], for the accepting side.
    pub async fn accept(&self) -> Option<Result<PeerConnection, NetError>> {
        let incoming = self.inner.accept().await?;
        Some(self.handshake_incoming(incoming).await)
    }

    async fn handshake_incoming(
        &self,
        incoming: quinn::Incoming,
    ) -> Result<PeerConnection, NetError> {
        let remote = incoming.remote_address();
        let connection = incoming.await.map_err(|e| NetError::Connect {
            addr: remote.to_string(),
            detail: e.to_string(),
        })?;

        let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
            .await
            .map_err(|_| NetError::HandshakeTimeout {
                seconds: HANDSHAKE_TIMEOUT.as_secs(),
            })?
            .map_err(|e| NetError::Stream {
                operation: "accept control stream",
                detail: e.to_string(),
            })?;

        let request = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_message(&mut recv))
            .await
            .map_err(|_| NetError::HandshakeTimeout {
                seconds: HANDSHAKE_TIMEOUT.as_secs(),
            })??;

        let ControlMessage::Hello(hello) = request else {
            return Err(NetError::Protocol(mb_protocol::ProtocolError::Invalid {
                field: "handshake",
                reason: "expected Hello",
            }));
        };

        // Negotiate before replying, so an incompatible peer gets a specific
        // error instead of a connection that fails later for no visible reason.
        let version = VersionRange::current().negotiate(hello.versions)?;

        let mut buffer = Vec::new();
        message::encode(
            &SessionMessage::Control(ControlMessage::HelloAck {
                version,
                device: self.identity.device_id(),
                name: self.local_name.clone(),
            }),
            "hello_ack",
            &mut buffer,
        )?;
        send.write_all(&buffer)
            .await
            .map_err(|e| NetError::Stream {
                operation: "send hello ack",
                detail: e.to_string(),
            })?;

        let peer = self.finish(connection, send, recv, hello.device, hello.name, version)?;
        info!(peer = %peer.device(), name = %peer.name(), %version, "accepted peer");
        Ok(peer)
    }

    /// Builds the connection record, checking the peer's claimed identity.
    #[allow(
        clippy::too_many_arguments,
        reason = "these are the parts of one handshake result, not independent knobs"
    )]
    fn finish(
        &self,
        connection: quinn::Connection,
        send: quinn::SendStream,
        recv: quinn::RecvStream,
        claimed: DeviceId,
        name: DeviceName,
        version: Version,
    ) -> Result<PeerConnection, NetError> {
        // The authoritative identity is the pinned certificate the TLS layer
        // already verified. The `Hello` merely *claims* one, and a peer that
        // claims a different identity than it authenticated as is either
        // confused or probing — either way the claim is discarded and the
        // verified value is used.
        let certificate = self.peer_certificate(&connection)?;
        let verified = mb_security::fingerprint(&certificate);
        if verified != claimed {
            warn!(
                %verified,
                %claimed,
                "peer claimed a different identity than its certificate; using the verified one"
            );
        }

        Ok(PeerConnection {
            connection,
            send,
            recv,
            device: verified,
            name,
            version,
            certificate,
        })
    }

    /// Extracts the certificate TLS actually authenticated.
    fn peer_certificate(
        &self,
        connection: &quinn::Connection,
    ) -> Result<rustls::pki_types::CertificateDer<'static>, NetError> {
        let identity = connection.peer_identity().ok_or_else(|| {
            // Unreachable while client authentication is mandatory, but asserted
            // rather than assumed: if that ever changed, this is what stops an
            // unauthenticated peer being treated as a known device.
            NetError::Security(mb_security::SecurityError::UntrustedDevice {
                fingerprint: "none presented".to_owned(),
            })
        })?;

        let chain = identity
            .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
            .map_err(|_| {
                NetError::Security(mb_security::SecurityError::UntrustedDevice {
                    fingerprint: "unrecognised identity type".to_owned(),
                })
            })?;

        let end_entity = chain.first().ok_or_else(|| {
            NetError::Security(mb_security::SecurityError::UntrustedDevice {
                fingerprint: "empty certificate chain".to_owned(),
            })
        })?;

        Ok(end_entity.clone())
    }

    fn hello(&self) -> Hello {
        Hello {
            versions: VersionRange::current(),
            device: self.identity.device_id(),
            name: self.local_name.clone(),
            os: mb_types::OsKind::current(),
            arch: mb_types::Arch::current(),
        }
    }

    /// Closes the endpoint and waits for connections to drain.
    pub async fn shutdown(&self) {
        self.inner.close(0u32.into(), b"shutting down");
        self.inner.wait_idle().await;
    }
}

/// Reads one length-prefixed message from a stream.
///
/// Reads incrementally rather than to end-of-stream: the stream stays open for
/// the rest of the session, so waiting for it to close would hang forever.
async fn read_message(recv: &mut quinn::RecvStream) -> Result<ControlMessage, NetError> {
    let mut buffer = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];

    loop {
        match decode_frame(&buffer)? {
            FrameDecode::Complete { payload, .. } => {
                let decoded: SessionMessage = message::decode(payload, "control")?;
                decoded.validate()?;
                let SessionMessage::Control(control) = decoded else {
                    return Err(NetError::Protocol(mb_protocol::ProtocolError::Invalid {
                        field: "handshake",
                        reason: "expected a control message",
                    }));
                };
                return Ok(control);
            }
            FrameDecode::Incomplete { .. } => {}
        }

        let read = recv.read(&mut chunk).await.map_err(|e| NetError::Stream {
            operation: "read control stream",
            detail: e.to_string(),
        })?;

        match read {
            Some(0) | None => {
                return Err(NetError::ConnectionLost {
                    detail: "peer closed the control stream mid-message".to_owned(),
                });
            }
            Some(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
}
