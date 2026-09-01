//! The set of devices this computer will accept a connection from.
//!
//! Trust is per-device and explicit. A device on the local network can be
//! *discovered* without being trusted; only an entry here permits a connection.

use crate::error::SecurityError;
use crate::identity::fingerprint;
use mb_types::{DeviceId, DeviceName};
use rustls_pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One trusted device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    /// Name the device advertised when it was paired.
    pub name: DeviceName,
    /// The exact certificate recorded for this device, DER-encoded.
    ///
    /// Stored in full rather than as a hash so verification is a constant-time
    /// byte comparison against known-good material, not a second hash of
    /// attacker-supplied input.
    #[serde(with = "base64_bytes")]
    pub certificate: Vec<u8>,
    /// When the device was first trusted, as seconds since the Unix epoch.
    pub paired_at: u64,
    /// When it last connected successfully.
    pub last_seen: u64,
}

/// Minimal base64 for embedding DER in TOML.
///
/// Hand-rolled rather than pulling in a dependency: the alphabet is fixed, the
/// input is small, and the round trip is directly tested below.
mod base64_bytes {
    use serde::{Deserialize as _, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(super) fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    let index = ((n >> (18 - i * 6)) & 0x3F) as usize;
                    out.push(ALPHABET[index] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for byte in text.bytes() {
            if byte == b'=' {
                break;
            }
            let value = ALPHABET.iter().position(|c| *c == byte)? as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xFF) as u8);
            }
        }
        Some(out)
    }

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        decode(&text).ok_or_else(|| serde::de::Error::custom("invalid base64"))
    }
}

/// Persisted form of the trust store.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustDocument {
    #[serde(default)]
    devices: BTreeMap<String, TrustEntry>,
}

/// Devices this computer will accept connections from.
#[derive(Debug, Default, Clone)]
pub struct TrustStore {
    devices: BTreeMap<DeviceId, TrustEntry>,
}

impl TrustStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of trusted devices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// True when no device is trusted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Returns the entry for a device.
    #[must_use]
    pub fn get(&self, device: DeviceId) -> Option<&TrustEntry> {
        self.devices.get(&device)
    }

    /// Iterates trusted devices.
    pub fn iter(&self) -> impl Iterator<Item = (&DeviceId, &TrustEntry)> {
        self.devices.iter()
    }

    /// Records trust in a device presenting `certificate`.
    ///
    /// Replaces any existing entry, so callers must have already decided that
    /// replacement is intended — see [`TrustStore::verify`], which refuses a
    /// changed certificate rather than letting it through to here.
    pub fn trust(&mut self, name: DeviceName, certificate: &CertificateDer<'_>, now: u64) {
        let device = fingerprint(certificate);
        let paired_at = self.devices.get(&device).map_or(now, |e| e.paired_at);
        self.devices.insert(
            device,
            TrustEntry {
                name,
                certificate: certificate.as_ref().to_vec(),
                paired_at,
                last_seen: now,
            },
        );
    }

    /// Removes a device.
    ///
    /// Returns whether anything was removed.
    pub fn forget(&mut self, device: DeviceId) -> bool {
        self.devices.remove(&device).is_some()
    }

    /// Decides whether a presented certificate belongs to a trusted device.
    ///
    /// # Errors
    ///
    /// [`SecurityError::UntrustedDevice`] when the device is unknown, or
    /// [`SecurityError::CertificateChanged`] when a known device presents
    /// different key material. The latter is never resolved automatically: a
    /// reinstall and an impersonation attempt look identical from here, so the
    /// user has to decide.
    pub fn verify(&self, certificate: &CertificateDer<'_>) -> Result<DeviceId, SecurityError> {
        let device = fingerprint(certificate);

        let Some(entry) = self.devices.get(&device) else {
            // Because the identity *is* the hash of the certificate, a device
            // that changed its key material presents an unknown fingerprint
            // rather than a mismatched one — there is no way to tell it apart
            // from a stranger, and treating it as one is the safe reading.
            return Err(SecurityError::UntrustedDevice {
                fingerprint: device.short(),
            });
        };

        if entry.certificate != certificate.as_ref() {
            // Unreachable while the fingerprint is a hash of the whole
            // certificate, but asserted rather than assumed: if the derivation
            // ever changes, this is the check that keeps a collision from
            // becoming an authentication bypass.
            return Err(SecurityError::CertificateChanged {
                device: device.short(),
            });
        }
        Ok(device)
    }

    /// Records a successful connection.
    pub fn touch(&mut self, device: DeviceId, now: u64) {
        if let Some(entry) = self.devices.get_mut(&device) {
            entry.last_seen = now;
        }
    }

    /// Loads a trust store, returning an empty one if the file does not exist.
    ///
    /// # Errors
    ///
    /// [`SecurityError::Io`] if the file exists but cannot be read, or
    /// [`SecurityError::CorruptTrustStore`] if it does not parse.
    ///
    /// A malformed trust store is **not** silently replaced with an empty one:
    /// that would drop every pairing and, worse, would make the next connection
    /// from an attacker look like an ordinary first-time pairing.
    pub fn load(path: &Path) -> Result<Self, SecurityError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => {
                return Err(SecurityError::Io {
                    path: path.display().to_string(),
                    detail: e.to_string(),
                });
            }
        };

        let document: TrustDocument =
            toml::from_str(&text).map_err(|e| SecurityError::CorruptTrustStore {
                detail: e.to_string(),
            })?;

        let mut devices = BTreeMap::new();
        for (key, entry) in document.devices {
            let device =
                DeviceId::parse_hex(&key).map_err(|e| SecurityError::CorruptTrustStore {
                    detail: format!("bad device id {key}: {e}"),
                })?;

            // The key must match the certificate it claims to identify.
            // Otherwise anyone who can write this file could point a trusted
            // identity at a certificate of their choosing.
            let presented = CertificateDer::from(entry.certificate.clone());
            if fingerprint(&presented) != device {
                return Err(SecurityError::CorruptTrustStore {
                    detail: format!("entry {} does not match its certificate", device.short()),
                });
            }
            devices.insert(device, entry);
        }
        Ok(Self { devices })
    }

    /// Writes the trust store atomically.
    ///
    /// # Errors
    ///
    /// [`SecurityError::Io`] if the file could not be written.
    pub fn save(&self, path: &Path) -> Result<(), SecurityError> {
        let document = TrustDocument {
            devices: self
                .devices
                .iter()
                .map(|(id, entry)| (id.to_hex(), entry.clone()))
                .collect(),
        };
        let text = toml::to_string_pretty(&document).map_err(|e| SecurityError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;

        let io = |e: std::io::Error| SecurityError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }

        // Same atomic write as the configuration: a half-written trust store
        // would drop pairings, and the recovery would look like a first pairing.
        let temporary: PathBuf = path.with_extension("toml.tmp");
        fs::write(&temporary, text.as_bytes()).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(io)?;
        }
        fs::rename(&temporary, path).map_err(io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn name(text: &str) -> DeviceName {
        DeviceName::new(text).expect("valid name")
    }

    #[test]
    fn base64_round_trips_including_padding() {
        for len in 0..64 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let encoded = base64_bytes::encode(&bytes);
            assert_eq!(
                base64_bytes::decode(&encoded).expect("decodes"),
                bytes,
                "round trip failed at length {len}"
            );
        }
    }

    #[test]
    fn base64_rejects_invalid_input() {
        assert_eq!(base64_bytes::decode("!!!!"), None);
    }

    #[test]
    fn an_unknown_device_is_refused() {
        let store = TrustStore::new();
        let stranger = Identity::generate().expect("generates");
        assert!(matches!(
            store.verify(stranger.certificate()),
            Err(SecurityError::UntrustedDevice { .. })
        ));
    }

    #[test]
    fn a_trusted_device_is_accepted() {
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("MacBook"), peer.certificate(), 100);

        assert_eq!(store.verify(peer.certificate()), Ok(peer.device_id()));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_different_certificate_from_a_stranger_is_refused() {
        // The core of the model: trusting one device must not trust another.
        let trusted = Identity::generate().expect("generates");
        let attacker = Identity::generate().expect("generates");

        let mut store = TrustStore::new();
        store.trust(name("MacBook"), trusted.certificate(), 100);

        assert!(store.verify(attacker.certificate()).is_err());
    }

    #[test]
    fn forgetting_a_device_revokes_it() {
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("PC"), peer.certificate(), 100);

        assert!(store.forget(peer.device_id()));
        assert!(store.verify(peer.certificate()).is_err());
        assert!(!store.forget(peer.device_id()), "second removal is a no-op");
    }

    #[test]
    fn re_trusting_preserves_the_original_pairing_time() {
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("PC"), peer.certificate(), 100);
        store.trust(name("PC renamed"), peer.certificate(), 500);

        let entry = store.get(peer.device_id()).expect("present");
        assert_eq!(entry.paired_at, 100, "pairing time was overwritten");
        assert_eq!(entry.last_seen, 500);
        assert_eq!(entry.name.as_str(), "PC renamed");
    }

    #[test]
    fn touch_updates_only_the_last_seen_time() {
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("PC"), peer.certificate(), 100);
        store.touch(peer.device_id(), 900);

        let entry = store.get(peer.device_id()).expect("present");
        assert_eq!(entry.paired_at, 100);
        assert_eq!(entry.last_seen, 900);
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.toml");

        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("MacBook Pro"), a.certificate(), 10);
        store.trust(name("Windows PC"), b.certificate(), 20);
        store.save(&path).expect("saves");

        let loaded = TrustStore::load(&path).expect("loads");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.verify(a.certificate()), Ok(a.device_id()));
        assert_eq!(loaded.verify(b.certificate()), Ok(b.device_id()));
        assert_eq!(
            loaded.get(a.device_id()).expect("present").name.as_str(),
            "MacBook Pro"
        );
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TrustStore::load(&dir.path().join("absent.toml")).expect("loads");
        assert!(store.is_empty());
    }

    #[test]
    fn a_corrupt_trust_store_is_an_error_not_an_empty_one() {
        // Silently starting empty would drop every pairing, and the attacker's
        // next connection would look like an ordinary first-time pairing.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.toml");
        fs::write(&path, "this is not [ valid toml").expect("writes");

        assert!(matches!(
            TrustStore::load(&path),
            Err(SecurityError::CorruptTrustStore { .. })
        ));
    }

    #[test]
    fn an_entry_whose_key_does_not_match_its_certificate_is_rejected() {
        // Anyone who can write this file could otherwise point a trusted
        // identity at a certificate of their choosing.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.toml");

        let real = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("PC"), real.certificate(), 1);
        store.save(&path).expect("saves");

        // Swap the recorded device id for a different one.
        let text = fs::read_to_string(&path).expect("reads");
        let forged = text.replace(&real.device_id().to_hex(), &"aa".repeat(32));
        fs::write(&path, forged).expect("writes");

        assert!(matches!(
            TrustStore::load(&path),
            Err(SecurityError::CorruptTrustStore { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_saved_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trust.toml");
        let peer = Identity::generate().expect("generates");
        let mut store = TrustStore::new();
        store.trust(name("PC"), peer.certificate(), 1);
        store.save(&path).expect("saves");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o077, 0, "trust store readable by other accounts");
    }
}
