//! Certificate pinning verifiers for `rustls`.
//!
//! # Why the public CA system is not used
//!
//! A certificate authority answers "does this key legitimately belong to
//! `example.com`". That question is meaningless here: peers are computers on a
//! local network with no public names, and no CA could vouch for them. Worse,
//! trusting the public roots would mean any CA-issued certificate satisfied the
//! check, turning a strong authentication model into a weak one.
//!
//! Instead both ends **pin**: a connection is accepted only when the peer
//! presents exactly the certificate recorded for it in the [`TrustStore`].
//!
//! # What is deliberately not checked
//!
//! Expiry, name matching, and chain building are all skipped, and each is a
//! considered decision rather than an omission:
//!
//! * **Expiry** — the pin is the trust anchor. A certificate rotating would
//!   break a pairing the user explicitly confirmed, to no benefit.
//! * **Names** — the certificate carries a fixed placeholder name. Identity comes
//!   from the pinned bytes, and matching a name nobody assigned would be theatre.
//! * **Chains** — the certificate is self-signed by design. There is no chain.
//!
//! Signature verification over the handshake transcript is **not** skipped: that
//! is what proves the peer holds the private key rather than merely a copy of the
//! certificate, and it is delegated to `rustls`.

use crate::trust::TrustStore;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error as TlsError, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::sync::{Arc, RwLock};

/// A trust store shared with the running application.
///
/// Behind a lock because pairing can add a device while connections are being
/// made. Reads are far more common than writes, so an `RwLock` is the right
/// shape; the lock is never held across a network operation.
pub type SharedTrust = Arc<RwLock<TrustStore>>;

/// Maps a trust decision onto the error `rustls` expects.
///
/// Deliberately uniform. A verifier that distinguished "unknown device" from
/// "wrong certificate" in the TLS alert would tell an unauthenticated scanner
/// which identities this machine knows about. The specific reason is surfaced to
/// the *user* through the application, not to the peer over the wire.
fn reject() -> TlsError {
    TlsError::General("device is not paired with this computer".to_owned())
}

/// Verifies the peer's certificate against the trust store.
fn check(trust: &SharedTrust, certificate: &CertificateDer<'_>) -> Result<(), TlsError> {
    let store = trust.read().map_err(|_| {
        // A poisoned lock means a panic happened somewhere. Failing closed is the
        // only safe response: the alternative is accepting a connection while the
        // trust state is unknown.
        TlsError::General("trust store is unavailable".to_owned())
    })?;
    store.verify(certificate).map(|_| ()).map_err(|_| reject())
}

/// Signature verification, delegated to the crypto provider.
fn verify_signature(
    algorithms: &WebPkiSupportedAlgorithms,
    message: &[u8],
    certificate: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
    tls13: bool,
) -> Result<HandshakeSignatureValid, TlsError> {
    if tls13 {
        rustls::crypto::verify_tls13_signature(message, certificate, dss, algorithms)
    } else {
        rustls::crypto::verify_tls12_signature(message, certificate, dss, algorithms)
    }
}

/// Verifies a *server* certificate by pinning. Used by the connecting side.
#[derive(Debug)]
pub struct PinnedServerVerifier {
    trust: SharedTrust,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedServerVerifier {
    /// Builds a verifier over a shared trust store.
    #[must_use]
    pub fn new(trust: SharedTrust, provider: &CryptoProvider) -> Self {
        Self {
            trust,
            algorithms: provider.signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        check(&self.trust, end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(&self.algorithms, message, cert, dss, false)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(&self.algorithms, message, cert, dss, true)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Verifies a *client* certificate by pinning. Used by the accepting side.
///
/// Client authentication is mandatory: an unauthenticated peer must never reach
/// the point of sending input events.
#[derive(Debug)]
pub struct PinnedClientVerifier {
    trust: SharedTrust,
    algorithms: WebPkiSupportedAlgorithms,
    /// Empty by design — see [`ClientCertVerifier::root_hint_subjects`].
    hints: Vec<DistinguishedName>,
}

impl PinnedClientVerifier {
    /// Builds a verifier over a shared trust store.
    #[must_use]
    pub fn new(trust: SharedTrust, provider: &CryptoProvider) -> Self {
        Self {
            trust,
            algorithms: provider.signature_verification_algorithms,
            hints: Vec::new(),
        }
    }
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        // Without this, a peer could decline to present a certificate and the
        // handshake would still succeed, leaving the connection unauthenticated.
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        // Empty on purpose. The hint list tells a connecting client which issuers
        // this server accepts; populating it from the trust store would disclose
        // the set of devices this computer is paired with to anyone who opens a
        // TCP connection.
        &self.hints
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        check(&self.trust, end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(&self.algorithms, message, cert, dss, false)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(&self.algorithms, message, cert, dss, true)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use mb_types::DeviceName;

    fn provider() -> CryptoProvider {
        rustls::crypto::ring::default_provider()
    }

    fn shared(store: TrustStore) -> SharedTrust {
        Arc::new(RwLock::new(store))
    }

    fn name(text: &str) -> DeviceName {
        DeviceName::new(text).expect("valid")
    }

    #[test]
    fn a_pinned_certificate_is_accepted_in_both_directions() {
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("Peer"), peer.certificate(), 1);
        let trust = shared(store);

        let server = PinnedServerVerifier::new(Arc::clone(&trust), &provider());
        assert!(
            server
                .verify_server_cert(
                    peer.certificate(),
                    &[],
                    &ServerName::try_from("mousebridge.local").expect("name"),
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_700_000_000)),
                )
                .is_ok()
        );

        let client = PinnedClientVerifier::new(trust, &provider());
        assert!(
            client
                .verify_client_cert(
                    peer.certificate(),
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_700_000_000)),
                )
                .is_ok()
        );
    }

    #[test]
    fn an_unpinned_certificate_is_rejected_in_both_directions() {
        let trusted = Identity::generate().expect("generates");
        let attacker = Identity::generate().expect("generates");

        let mut store = TrustStore::new();
        store.trust(name("Peer"), trusted.certificate(), 1);
        let trust = shared(store);

        let server = PinnedServerVerifier::new(Arc::clone(&trust), &provider());
        assert!(
            server
                .verify_server_cert(
                    attacker.certificate(),
                    &[],
                    &ServerName::try_from("mousebridge.local").expect("name"),
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_700_000_000)),
                )
                .is_err()
        );

        let client = PinnedClientVerifier::new(trust, &provider());
        assert!(
            client
                .verify_client_cert(
                    attacker.certificate(),
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_700_000_000)),
                )
                .is_err()
        );
    }

    #[test]
    fn an_empty_trust_store_accepts_nobody() {
        let stranger = Identity::generate().expect("generates");
        let trust = shared(TrustStore::new());
        let client = PinnedClientVerifier::new(trust, &provider());
        assert!(
            client
                .verify_client_cert(
                    stranger.certificate(),
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::from_secs(0)),
                )
                .is_err()
        );
    }

    #[test]
    fn expiry_is_deliberately_not_consulted() {
        // The certificate is valid from the year 2000 to 4096, and the pin is the
        // trust anchor. Verifying at an absurd time must not change the outcome.
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("Peer"), peer.certificate(), 1);
        let client = PinnedClientVerifier::new(shared(store), &provider());

        for seconds in [0u64, 1_700_000_000, u32::MAX as u64] {
            assert!(
                client
                    .verify_client_cert(
                        peer.certificate(),
                        &[],
                        UnixTime::since_unix_epoch(std::time::Duration::from_secs(seconds)),
                    )
                    .is_ok()
            );
        }
    }

    #[test]
    fn client_authentication_is_mandatory() {
        // Without this a peer could decline to present a certificate and still
        // complete the handshake, arriving unauthenticated.
        let client = PinnedClientVerifier::new(shared(TrustStore::new()), &provider());
        assert!(client.offer_client_auth());
        assert!(client.client_auth_mandatory());
    }

    #[test]
    fn the_hint_list_does_not_disclose_paired_devices() {
        // Populating it would tell any machine that can open a TCP connection
        // exactly which devices this computer is paired with.
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("Secret Laptop"), peer.certificate(), 1);
        let client = PinnedClientVerifier::new(shared(store), &provider());
        assert!(client.root_hint_subjects().is_empty());
    }

    #[test]
    fn rejection_does_not_reveal_why() {
        // "Unknown device" and "wrong certificate" must look identical on the
        // wire, or a scanner learns which identities this machine knows.
        let trusted = Identity::generate().expect("generates");
        let stranger = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("Peer"), trusted.certificate(), 1);
        let trust = shared(store);

        let verifier = PinnedClientVerifier::new(trust, &provider());
        let now = UnixTime::since_unix_epoch(std::time::Duration::from_secs(1));
        let unknown = verifier
            .verify_client_cert(stranger.certificate(), &[], now)
            .expect_err("rejected");

        assert_eq!(unknown.to_string(), reject().to_string());
    }

    #[test]
    fn supported_schemes_are_not_empty() {
        // An empty list would make every handshake fail with an opaque error.
        let verifier = PinnedServerVerifier::new(shared(TrustStore::new()), &provider());
        assert!(!verifier.supported_verify_schemes().is_empty());
    }
}
