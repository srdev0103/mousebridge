//! Device identity, certificate pinning and the trust store.
//!
//! # Threat model
//!
//! The adversary is anyone else on the local network, including an active
//! man-in-the-middle who can intercept and rewrite traffic. Defending against an
//! attacker who already has code execution on a paired machine is explicitly a
//! **non-goal**: at that point they have the user's keyboard anyway.
//!
//! # No custom cryptography
//!
//! Every primitive comes from `rustls` (TLS 1.3), `rcgen` (certificate
//! generation) and `sha2`. This crate contains no cryptographic construction of
//! its own — only key management, identity derivation and trust decisions.
//!
//! # How identity works
//!
//! Each installation generates an Ed25519 key pair and a long-lived self-signed
//! certificate. The device's identity is the SHA-256 of that certificate's DER
//! encoding, which makes it self-authenticating: a peer cannot claim an identity
//! without holding the private key that signs for it.
//!
//! Public certificate authorities are irrelevant here and are explicitly not
//! trusted. Both ends authenticate by *pinning*: a connection is accepted only
//! if the peer presents exactly the certificate recorded for that device.
//!
//! Milestone 5 establishes trust on first use. Milestone 10 adds the numeric
//! comparison that makes first use safe against an active attacker.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod error;
pub mod identity;
pub mod pinning;
pub mod trust;

pub use error::SecurityError;
pub use identity::{Identity, fingerprint};
pub use pinning::{PinnedClientVerifier, PinnedServerVerifier};
pub use trust::{TrustEntry, TrustStore};
