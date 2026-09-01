//! Device and screen identity.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Length in bytes of a [`DeviceId`].
pub const DEVICE_ID_LEN: usize = 32;

/// Maximum length of a [`DeviceName`], in Unicode scalar values.
pub const DEVICE_NAME_MAX_CHARS: usize = 64;

/// A stable, opaque identifier for one MouseBridge installation.
///
/// From milestone 10 onward this is the SHA-256 fingerprint of the device's
/// long-term Ed25519 public key, which makes the identifier self-authenticating:
/// a peer cannot claim an identity it does not hold the private key for. Until
/// the security crate lands, [`DeviceId::generate`] produces a random value with
/// the same shape, so no call site has to change when the derivation does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceId([u8; DEVICE_ID_LEN]);

impl DeviceId {
    /// Wraps raw fingerprint bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; DEVICE_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// Returns the raw fingerprint bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DEVICE_ID_LEN] {
        &self.0
    }

    /// Generates a random identifier from the operating system's CSPRNG.
    ///
    /// # Errors
    ///
    /// Returns [`IdError::Entropy`] if the OS random source is unavailable.
    pub fn generate() -> Result<Self, IdError> {
        let mut bytes = [0u8; DEVICE_ID_LEN];
        getrandom::fill(&mut bytes).map_err(|_| IdError::Entropy)?;
        Ok(Self(bytes))
    }

    /// Renders the full identifier as lowercase hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(DEVICE_ID_LEN * 2);
        for byte in &self.0 {
            use fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Returns the first 8 hex characters, for display in the UI and in logs.
    ///
    /// This is a *display* convenience only. Never use the short form to make a
    /// trust decision: 32 bits is far too little to resist a deliberate collision.
    #[must_use]
    pub fn short(&self) -> String {
        self.to_hex().chars().take(8).collect()
    }

    /// Parses a full lowercase or uppercase hex identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdError::Malformed`] if the input is not exactly
    /// `2 * DEVICE_ID_LEN` hex digits.
    pub fn parse_hex(text: &str) -> Result<Self, IdError> {
        if text.len() != DEVICE_ID_LEN * 2 {
            return Err(IdError::Malformed);
        }
        let mut bytes = [0u8; DEVICE_ID_LEN];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let hi = hex_val(text.as_bytes()[i * 2])?;
            let lo = hex_val(text.as_bytes()[i * 2 + 1])?;
            *byte = (hi << 4) | lo;
        }
        Ok(Self(bytes))
    }
}

fn hex_val(c: u8) -> Result<u8, IdError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(IdError::Malformed),
    }
}

/// Displays the short form. Full identifiers are long enough to wreck log lines,
/// so `Display` and `Debug` both abbreviate; use [`DeviceId::to_hex`] when the
/// complete value is actually needed.
impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

impl From<DeviceId> for String {
    fn from(id: DeviceId) -> Self {
        id.to_hex()
    }
}

impl TryFrom<String> for DeviceId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_hex(&value)
    }
}

/// Errors produced when constructing a [`DeviceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The operating system random source was unavailable.
    #[error("system entropy source unavailable")]
    Entropy,
    /// The text was not a valid 64-character hex string.
    #[error("malformed device id: expected 64 hex characters")]
    Malformed,
}

/// A human-readable device name, as shown in the UI and broadcast on discovery.
///
/// Validated on construction because this value is rendered in another user's
/// interface: control characters and unbounded length are a spoofing surface,
/// not merely a cosmetic problem.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceName(String);

impl DeviceName {
    /// Validates and wraps a display name.
    ///
    /// Leading and trailing whitespace is trimmed. The result must be non-empty,
    /// at most [`DEVICE_NAME_MAX_CHARS`] characters, and free of control characters.
    ///
    /// # Errors
    ///
    /// See [`DeviceNameError`].
    pub fn new(raw: impl Into<String>) -> Result<Self, DeviceNameError> {
        let trimmed = raw.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(DeviceNameError::Empty);
        }
        if trimmed.chars().count() > DEVICE_NAME_MAX_CHARS {
            return Err(DeviceNameError::TooLong);
        }
        if trimmed.chars().any(char::is_control) {
            return Err(DeviceNameError::ControlCharacter);
        }
        Ok(Self(trimmed))
    }

    /// Returns the validated name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceName({:?})", self.0)
    }
}

impl From<DeviceName> for String {
    fn from(name: DeviceName) -> Self {
        name.0
    }
}

impl TryFrom<String> for DeviceName {
    type Error = DeviceNameError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Reasons a [`DeviceName`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeviceNameError {
    /// The name was empty or entirely whitespace.
    #[error("device name is empty")]
    Empty,
    /// The name exceeded [`DEVICE_NAME_MAX_CHARS`].
    #[error("device name exceeds {DEVICE_NAME_MAX_CHARS} characters")]
    TooLong,
    /// The name contained a control character.
    #[error("device name contains a control character")]
    ControlCharacter,
}

/// Operating system family of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsKind {
    /// Apple macOS.
    MacOs,
    /// Microsoft Windows.
    Windows,
    /// A platform this build does not support natively.
    Unknown,
}

impl OsKind {
    /// Returns the family this binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Unknown
        }
    }
}

impl fmt::Display for OsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Unknown => "Unknown",
        })
    }
}

/// CPU architecture of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
    /// An architecture this build does not recognise.
    Unknown,
}

impl Arch {
    /// Returns the architecture this binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::Unknown
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "arm64",
            Self::Unknown => "unknown",
        })
    }
}

/// Identifies one display within a single device.
///
/// The numeric value is assigned by the platform layer and is stable only for as
/// long as the display stays attached; it is not persisted across reboots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScreenId(pub u32);

impl fmt::Display for ScreenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "screen#{}", self.0)
    }
}

/// Identifies one display anywhere in the topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GlobalScreenId {
    /// The device the display is attached to.
    pub device: DeviceId,
    /// The display's index within that device.
    pub screen: ScreenId,
}

impl GlobalScreenId {
    /// Builds a global screen identifier.
    #[must_use]
    pub const fn new(device: DeviceId, screen: ScreenId) -> Self {
        Self { device, screen }
    }
}

impl fmt::Display for GlobalScreenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.device, self.screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_hex_round_trips() {
        let id = DeviceId::from_bytes([0xAB; DEVICE_ID_LEN]);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(DeviceId::parse_hex(&hex), Ok(id));
    }

    #[test]
    fn device_id_parse_rejects_bad_input() {
        assert_eq!(DeviceId::parse_hex(""), Err(IdError::Malformed));
        assert_eq!(DeviceId::parse_hex("zz"), Err(IdError::Malformed));
        // Right length, wrong alphabet.
        assert_eq!(
            DeviceId::parse_hex(&"g".repeat(64)),
            Err(IdError::Malformed)
        );
        // Right alphabet, wrong length.
        assert_eq!(
            DeviceId::parse_hex(&"a".repeat(63)),
            Err(IdError::Malformed)
        );
    }

    #[test]
    fn device_id_display_is_abbreviated() {
        // Full 64-char ids wreck log lines and tempt callers into comparing them
        // by eye, so Display must never emit the whole thing.
        let id = DeviceId::from_bytes([0x0F; DEVICE_ID_LEN]);
        assert_eq!(id.to_string(), "0f0f0f0f");
        assert_eq!(format!("{id:?}"), "DeviceId(0f0f0f0f)");
    }

    #[test]
    fn generated_ids_differ() {
        let a = DeviceId::generate().expect("entropy");
        let b = DeviceId::generate().expect("entropy");
        assert_ne!(a, b);
    }

    #[test]
    fn device_name_trims_and_validates() {
        assert_eq!(
            DeviceName::new("  MacBook Pro  ").expect("valid").as_str(),
            "MacBook Pro"
        );
        assert_eq!(DeviceName::new("   "), Err(DeviceNameError::Empty));
        assert_eq!(
            DeviceName::new("a".repeat(DEVICE_NAME_MAX_CHARS + 1)),
            Err(DeviceNameError::TooLong)
        );
        assert_eq!(
            DeviceName::new("evil\u{0}name"),
            Err(DeviceNameError::ControlCharacter)
        );
        // A newline would let a hostile peer forge extra lines in our UI or logs.
        assert_eq!(
            DeviceName::new("line\nbreak"),
            Err(DeviceNameError::ControlCharacter)
        );
    }

    #[test]
    fn device_name_allows_unicode() {
        assert!(DeviceName::new("Björn's MacBook 💻").is_ok());
    }
}
