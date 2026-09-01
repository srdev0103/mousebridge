//! TLS configuration for QUIC.
//!
//! Both directions authenticate with pinned certificates from
//! [`mb_security::TrustStore`]. There is no anonymous mode and no
//! server-authenticated-only mode: a peer that cannot prove which device it is
//! must never reach the point of sending input events.

use crate::error::NetError;
use mb_security::pinning::SharedTrust;
use mb_security::{Identity, PinnedClientVerifier, PinnedServerVerifier};
use std::sync::Arc;

/// Application-Layer Protocol Negotiation identifier.
///
/// QUIC requires ALPN. Versioning it means a future incompatible transport can
/// coexist on the same port: a peer offering only `mousebridge/2` is rejected at
/// the TLS layer rather than failing confusingly later.
pub const ALPN: &[u8] = b"mousebridge/1";

/// Builds a `rustls` server configuration that accepts any peer, for pairing.
///
/// The connection this permits carries **only** nonce exchange and confirmation.
/// See [`mb_security::AcceptAnyForPairing`] for why that is safe and what it
/// still verifies.
///
/// # Errors
///
/// [`NetError::TlsConfig`] if the identity is unusable.
pub fn pairing_server_config(identity: &Identity) -> Result<rustls::ServerConfig, NetError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(mb_security::AcceptAnyForPairing::new(&provider));

    let mut config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::TlsConfig {
            side: "pairing server",
            detail: e.to_string(),
        })?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![identity.certificate().clone()],
            identity.private_key()?,
        )
        .map_err(|e| NetError::TlsConfig {
            side: "pairing server",
            detail: e.to_string(),
        })?;

    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// Builds a `rustls` client configuration that accepts any peer, for pairing.
///
/// # Errors
///
/// [`NetError::TlsConfig`] if the identity is unusable.
pub fn pairing_client_config(identity: &Identity) -> Result<rustls::ClientConfig, NetError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(mb_security::AcceptAnyForPairing::new(&provider));

    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::TlsConfig {
            side: "pairing client",
            detail: e.to_string(),
        })?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(
            vec![identity.certificate().clone()],
            identity.private_key()?,
        )
        .map_err(|e| NetError::TlsConfig {
            side: "pairing client",
            detail: e.to_string(),
        })?;

    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// Builds the `rustls` server configuration.
///
/// # Errors
///
/// [`NetError::TlsConfig`] if the identity's key and certificate are not a
/// usable pair.
pub fn server_config(
    identity: &Identity,
    trust: SharedTrust,
) -> Result<rustls::ServerConfig, NetError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedClientVerifier::new(trust, &provider));

    let mut config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        // QUIC is TLS 1.3 only. Stating it explicitly means a misconfiguration
        // fails here rather than at handshake time.
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::TlsConfig {
            side: "server",
            detail: e.to_string(),
        })?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![identity.certificate().clone()],
            identity.private_key()?,
        )
        .map_err(|e| NetError::TlsConfig {
            side: "server",
            detail: e.to_string(),
        })?;

    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// Builds the `rustls` client configuration.
///
/// # Errors
///
/// [`NetError::TlsConfig`] if the identity's key and certificate are not a
/// usable pair.
pub fn client_config(
    identity: &Identity,
    trust: SharedTrust,
) -> Result<rustls::ClientConfig, NetError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedServerVerifier::new(trust, &provider));

    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::TlsConfig {
            side: "client",
            detail: e.to_string(),
        })?
        // `dangerous` names the fact that the public CA system is being bypassed.
        // That is the intent: see the module documentation on `mb_security::pinning`
        // for why a CA cannot answer the question being asked here.
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(
            vec![identity.certificate().clone()],
            identity.private_key()?,
        )
        .map_err(|e| NetError::TlsConfig {
            side: "client",
            detail: e.to_string(),
        })?;

    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}
