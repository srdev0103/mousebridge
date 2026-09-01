//! This installation's cryptographic identity.

use crate::error::SecurityError;
use mb_types::{DeviceId, Redacted};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest as _, Sha256};

/// Subject alternative name placed in the self-signed certificate.
///
/// Never used for verification — trust comes from the pin, not the name — but
/// TLS requires a server name, and a fixed, obviously-internal value is clearer
/// in a packet capture than a hostname that implies it means something.
pub const CERTIFICATE_NAME: &str = "mousebridge.local";

/// Returns the identity fingerprint of a certificate.
///
/// The SHA-256 of the DER encoding. Hashing the whole certificate rather than
/// just its public key means no X.509 parser is needed to verify a peer — the
/// bytes arriving from the handshake are hashed directly — at the cost of the
/// identity changing if the certificate is ever reissued for the same key. Since
/// the certificate is generated once and stored alongside the key, that does not
/// happen in practice.
#[must_use]
pub fn fingerprint(certificate: &CertificateDer<'_>) -> DeviceId {
    let digest = Sha256::digest(certificate.as_ref());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    DeviceId::from_bytes(bytes)
}

/// A device's key pair and certificate.
#[derive(Debug)]
pub struct Identity {
    /// PKCS#8 DER of the private key. Never logged; see [`Redacted`].
    private_key: Redacted<Vec<u8>>,
    certificate: CertificateDer<'static>,
    device_id: DeviceId,
}

impl Identity {
    /// Generates a new identity.
    ///
    /// # Errors
    ///
    /// [`SecurityError::Generation`] if the key pair or certificate could not be
    /// produced.
    pub fn generate() -> Result<Self, SecurityError> {
        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).map_err(|e| {
            SecurityError::Generation {
                what: "Ed25519 key pair",
                detail: e.to_string(),
            }
        })?;
        Self::from_key_pair(key_pair)
    }

    /// Rebuilds an identity from a stored private key.
    ///
    /// # Errors
    ///
    /// [`SecurityError::CorruptIdentity`] if the key does not parse, or
    /// [`SecurityError::Generation`] if the certificate could not be reissued.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, SecurityError> {
        let key_pair =
            rcgen::KeyPair::try_from(der).map_err(|e| SecurityError::CorruptIdentity {
                detail: e.to_string(),
            })?;
        Self::from_key_pair(key_pair)
    }

    fn from_key_pair(key_pair: rcgen::KeyPair) -> Result<Self, SecurityError> {
        let private_key = key_pair.serialize_der();

        let mut params =
            rcgen::CertificateParams::new(vec![CERTIFICATE_NAME.to_owned()]).map_err(|e| {
                SecurityError::Generation {
                    what: "certificate parameters",
                    detail: e.to_string(),
                }
            })?;

        // A pinned self-signed certificate gains nothing from expiry: the pin is
        // the trust anchor, and a renewal would only break the pairing the user
        // already confirmed. The window is deliberately wide, and the verifier
        // does not consult it.
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(4096, 1, 1);
        params.distinguished_name = {
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, "MouseBridge Device");
            dn
        };

        let certificate = params
            .self_signed(&key_pair)
            .map_err(|e| SecurityError::Generation {
                what: "self-signed certificate",
                detail: e.to_string(),
            })?;

        let certificate = certificate.der().clone();
        let device_id = fingerprint(&certificate);

        Ok(Self {
            private_key: Redacted::new(private_key),
            certificate,
            device_id,
        })
    }

    /// This device's identity.
    #[must_use]
    pub const fn device_id(&self) -> DeviceId {
        self.device_id
    }

    /// The certificate presented to peers.
    #[must_use]
    pub const fn certificate(&self) -> &CertificateDer<'static> {
        &self.certificate
    }

    /// The private key, for building a TLS configuration.
    ///
    /// # Errors
    ///
    /// [`SecurityError::CorruptIdentity`] if the stored key is not valid PKCS#8.
    pub fn private_key(&self) -> Result<PrivateKeyDer<'static>, SecurityError> {
        Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            self.private_key.expose().clone(),
        )))
    }

    /// The private key in PKCS#8 DER, for storage in the OS keystore.
    ///
    /// Returned wrapped so it cannot reach a log through a stray `Debug`.
    #[must_use]
    pub const fn private_key_pkcs8(&self) -> &Redacted<Vec<u8>> {
        &self.private_key
    }
}

/// Loads the identity from `path`, creating one if it is not there.
///
/// # Where the key lives
///
/// A file beside the configuration, readable only by the owning user. The
/// original design said the OS keystore — Keychain on macOS, Credential Manager
/// on Windows — and that remains the right destination. It is not there yet, and
/// this comment exists so the gap is visible rather than forgotten: a file is
/// readable by anything running as the same user, whereas a keystore entry is
/// not, and on macOS moving it there also means the key survives a reinstall.
///
/// # Errors
///
/// [`SecurityError::Io`] if the file cannot be read or written, or
/// [`SecurityError::CorruptIdentity`] if it exists but is unusable.
pub fn load_or_create(path: &std::path::Path) -> Result<Identity, SecurityError> {
    match std::fs::read(path) {
        Ok(bytes) => Identity::from_pkcs8(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let identity = Identity::generate()?;
            save(&identity, path)?;
            Ok(identity)
        }
        Err(e) => Err(SecurityError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        }),
    }
}

/// Writes the private key, restricted to the owning user.
///
/// # Errors
///
/// [`SecurityError::Io`] if the file could not be written.
pub fn save(identity: &Identity, path: &std::path::Path) -> Result<(), SecurityError> {
    let io = |e: std::io::Error| SecurityError::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }

    // Written to a temporary file and renamed, so a crash cannot leave a
    // half-written key. Recovering from that would mean generating a new
    // identity, which un-pairs the device from every peer it knows.
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, identity.private_key_pkcs8().expose()).map_err(io)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    std::fs::rename(&temporary, path).map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_a_usable_identity() {
        let identity = Identity::generate().expect("generates");
        assert!(!identity.certificate().as_ref().is_empty());
        assert!(identity.private_key().is_ok());
        assert_eq!(identity.device_id(), fingerprint(identity.certificate()));
    }

    #[test]
    fn two_identities_differ() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        assert_ne!(a.device_id(), b.device_id());
        assert_ne!(a.certificate(), b.certificate());
    }

    #[test]
    fn an_identity_reloaded_from_its_key_keeps_the_same_device_id() {
        // The property the whole trust model rests on: restarting the
        // application must not change who this computer is, or every peer would
        // see a stranger and refuse to connect.
        let original = Identity::generate().expect("generates");
        let stored = original.private_key_pkcs8().expose().clone();

        let reloaded = Identity::from_pkcs8(&stored).expect("reloads");
        assert_eq!(
            reloaded.device_id(),
            original.device_id(),
            "identity changed across a restart"
        );
        assert_eq!(reloaded.certificate(), original.certificate());
    }

    #[test]
    fn a_corrupt_private_key_is_rejected() {
        assert!(matches!(
            Identity::from_pkcs8(b"not a key"),
            Err(SecurityError::CorruptIdentity { .. })
        ));
        assert!(Identity::from_pkcs8(&[]).is_err());
    }

    #[test]
    fn the_private_key_never_appears_in_debug_output() {
        let identity = Identity::generate().expect("generates");
        let rendered = format!("{identity:?}");
        assert!(rendered.contains("redacted"), "{rendered}");

        // And the actual bytes are nowhere in it.
        let key_prefix = &identity.private_key_pkcs8().expose()[..8];
        let hex: String = key_prefix.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!rendered.contains(&hex), "key material leaked into Debug");
    }

    #[test]
    fn an_identity_persists_across_restarts() {
        // The property every pairing depends on: restarting must not change who
        // this computer is, or every peer sees a stranger and refuses it.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("identity.key");

        let first = load_or_create(&path).expect("creates");
        let second = load_or_create(&path).expect("loads");
        assert_eq!(first.device_id(), second.device_id());
        assert_eq!(first.certificate(), second.certificate());
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_not_readable_by_other_accounts() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("identity.key");
        load_or_create(&path).expect("creates");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "the private key is readable by other users"
        );
    }

    #[test]
    fn a_corrupt_key_file_is_reported_not_replaced() {
        // Silently generating a new identity would un-pair the device from every
        // peer, and the reconnection would look like an ordinary first pairing.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("identity.key");
        std::fs::write(&path, b"not a key").expect("writes");

        assert!(matches!(
            load_or_create(&path),
            Err(SecurityError::CorruptIdentity { .. })
        ));
    }

    #[test]
    fn fingerprints_distinguish_certificates() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        assert_ne!(fingerprint(a.certificate()), fingerprint(b.certificate()));

        // And are stable for the same input.
        assert_eq!(fingerprint(a.certificate()), fingerprint(a.certificate()));
    }

    #[test]
    fn a_single_flipped_bit_changes_the_fingerprint() {
        // Pinning is only meaningful if the fingerprint actually covers the
        // whole certificate.
        let identity = Identity::generate().expect("generates");
        let mut tampered = identity.certificate().as_ref().to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;

        let tampered = CertificateDer::from(tampered);
        assert_ne!(fingerprint(&tampered), identity.device_id());
    }
}
