//! The configuration schema.

use mb_types::{DeviceId, DeviceName};
use serde::{Deserialize, Serialize};

/// Schema version written by this build.
///
/// Bump this whenever a field is removed or changes meaning, and add a step in
/// [`crate::migrate`]. Adding an optional field with a default does not require a
/// bump.
pub const CURRENT_VERSION: u32 = 1;

/// Default UDP port for QUIC transport and discovery.
///
/// Chosen from the unassigned dynamic range and deliberately unrelated to the
/// ports used by other software KVMs, so a machine running both does not see
/// cross-talk during discovery.
pub const DEFAULT_PORT: u16 = 25470;

/// Root configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version of this document.
    #[serde(default = "default_version")]
    pub version: u32,
    /// This installation's identity.
    pub device: DeviceConfig,
    /// Transport and discovery settings.
    #[serde(default)]
    pub network: NetworkConfig,
    /// Edge-crossing behaviour.
    #[serde(default)]
    pub switching: SwitchingConfig,
    /// Diagnostics.
    #[serde(default)]
    pub logging: LoggingConfig,
}

const fn default_version() -> u32 {
    CURRENT_VERSION
}

impl Config {
    /// Builds a default configuration for a freshly installed device.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS entropy source is unavailable or `name` is not
    /// a valid [`DeviceName`].
    pub fn bootstrap(name: &str) -> Result<Self, BootstrapError> {
        Ok(Self {
            version: CURRENT_VERSION,
            device: DeviceConfig {
                id: DeviceId::generate().map_err(|_| BootstrapError::Entropy)?,
                name: DeviceName::new(name).map_err(BootstrapError::Name)?,
            },
            network: NetworkConfig::default(),
            switching: SwitchingConfig::default(),
            logging: LoggingConfig::default(),
        })
    }

    /// Checks every field for values that would produce degenerate behaviour.
    ///
    /// Called after loading, because the file is user-editable: a hand-typed
    /// `cooldown_ms = 0` must be rejected at the boundary rather than causing the
    /// cursor to oscillate between machines at event rate.
    ///
    /// # Errors
    ///
    /// Returns the first field that failed, with the acceptable range.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.network.validate()?;
        self.switching.validate()?;
        Ok(())
    }
}

/// Failure while constructing a first-run configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BootstrapError {
    /// The OS random source was unavailable.
    #[error("cannot generate device identity: system entropy source unavailable")]
    Entropy,
    /// The supplied device name was not usable.
    #[error("invalid device name: {0}")]
    Name(#[from] mb_types::DeviceNameError),
}

/// A configuration value outside its acceptable range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} is out of range: {reason}")]
pub struct ValidationError {
    /// Dotted path of the offending field, e.g. `switching.cooldown_ms`.
    pub field: &'static str,
    /// Human-readable explanation, including the acceptable range.
    pub reason: String,
}

impl ValidationError {
    fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}

/// Identity of this installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    /// Stable identifier. Regenerating this un-pairs the device from every peer.
    pub id: DeviceId,
    /// Name shown to other devices during discovery and pairing.
    pub name: DeviceName,
}

/// Transport and discovery settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// UDP port for QUIC and discovery.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Whether to announce and browse for peers on the local network.
    #[serde(default = "yes")]
    pub discovery_enabled: bool,
    /// Peers to contact directly, as `host` or `host:port`.
    ///
    /// The escape hatch for networks where multicast and broadcast are filtered,
    /// which describes most corporate Wi-Fi. Always available, never optional.
    #[serde(default)]
    pub manual_peers: Vec<String>,
}

const fn default_port() -> u16 {
    DEFAULT_PORT
}
const fn yes() -> bool {
    true
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            discovery_enabled: true,
            manual_peers: Vec::new(),
        }
    }
}

impl NetworkConfig {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.port < 1024 {
            return Err(ValidationError::new(
                "network.port",
                "must be 1024 or above; privileged ports would require running as root",
            ));
        }
        if self.manual_peers.len() > 64 {
            return Err(ValidationError::new(
                "network.manual_peers",
                "at most 64 entries",
            ));
        }
        Ok(())
    }
}

/// Edge-crossing behaviour.
///
/// These are the anti-jitter controls described in the topology design. The
/// defaults are starting points to be tuned against real hardware in milestone 7;
/// they are exposed in the file because the right value genuinely depends on
/// mouse DPI and the user's pointer speed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchingConfig {
    /// Master switch for handing input to other devices.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Logical points of continued outward motion required to commit a crossing.
    ///
    /// Guards against a single overshoot frame teleporting the cursor to another
    /// machine when the user was only reaching for a scrollbar.
    #[serde(default = "default_overshoot")]
    pub edge_overshoot_points: f64,
    /// Milliseconds after a crossing during which no further crossing may occur.
    ///
    /// Without this the cursor can oscillate across a seam at event rate.
    #[serde(default = "default_cooldown")]
    pub cooldown_ms: u64,
    /// Size of the excluded region at each screen corner, in logical points.
    ///
    /// The macOS menu bar, the Windows Start button and window close buttons all
    /// live in corners, and users throw the pointer at them at speed. Crossing
    /// there is almost always accidental.
    #[serde(default = "default_corner")]
    pub corner_deadzone_points: f64,
}

const fn default_overshoot() -> f64 {
    12.0
}
const fn default_cooldown() -> u64 {
    200
}
const fn default_corner() -> f64 {
    8.0
}

impl Default for SwitchingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            edge_overshoot_points: default_overshoot(),
            cooldown_ms: default_cooldown(),
            corner_deadzone_points: default_corner(),
        }
    }
}

impl SwitchingConfig {
    fn validate(&self) -> Result<(), ValidationError> {
        if !self.edge_overshoot_points.is_finite()
            || !(0.0..=500.0).contains(&self.edge_overshoot_points)
        {
            return Err(ValidationError::new(
                "switching.edge_overshoot_points",
                "must be a finite value between 0 and 500",
            ));
        }
        if self.cooldown_ms > 10_000 {
            return Err(ValidationError::new(
                "switching.cooldown_ms",
                "must be 10000 or below",
            ));
        }
        if !self.corner_deadzone_points.is_finite()
            || !(0.0..=500.0).contains(&self.corner_deadzone_points)
        {
            return Err(ValidationError::new(
                "switching.corner_deadzone_points",
                "must be a finite value between 0 and 500",
            ));
        }
        Ok(())
    }
}

/// Diagnostics settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    /// Verbosity.
    #[serde(default)]
    pub level: LogLevel,
    /// Whether to also write a rotating log file next to the configuration.
    #[serde(default = "yes")]
    pub file_enabled: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            file_enabled: true,
        }
    }
}

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Failures that stop a feature working.
    Error,
    /// Recoverable problems worth surfacing.
    Warn,
    /// Lifecycle events. The default.
    #[default]
    Info,
    /// Detailed diagnostics.
    Debug,
    /// Per-event tracing. Never enable on the input path in a release build.
    Trace,
}

impl LogLevel {
    /// Returns the `tracing` filter directive for this level.
    #[must_use]
    pub const fn as_filter_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config::bootstrap("Test Device").unwrap()
    }

    #[test]
    fn bootstrap_produces_a_valid_config() {
        let cfg = sample();
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.network.port, DEFAULT_PORT);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn bootstrap_rejects_a_bad_name() {
        assert!(matches!(
            Config::bootstrap("  "),
            Err(BootstrapError::Name(_))
        ));
    }

    #[test]
    fn toml_round_trips() {
        let cfg = sample();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn omitted_sections_fall_back_to_defaults() {
        // A user editing this file by hand should be able to delete a whole
        // section without breaking startup.
        let id = DeviceId::generate().unwrap().to_hex();
        let text = format!("version = 1\n[device]\nid = \"{id}\"\nname = \"Mac\"\n");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.network, NetworkConfig::default());
        assert_eq!(cfg.switching, SwitchingConfig::default());
        assert_eq!(cfg.logging.level, LogLevel::Info);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // A silently ignored typo is a support ticket that never gets diagnosed.
        let id = DeviceId::generate().unwrap().to_hex();
        let text =
            format!("version = 1\n[device]\nid = \"{id}\"\nname = \"Mac\"\n[netwrok]\nport = 1\n");
        assert!(toml::from_str::<Config>(&text).is_err());
    }

    #[test]
    fn validate_rejects_dangerous_hand_edits() {
        let mut cfg = sample();
        cfg.switching.edge_overshoot_points = f64::NAN;
        let err = cfg.validate().unwrap_err();
        assert_eq!(err.field, "switching.edge_overshoot_points");

        let mut cfg = sample();
        cfg.switching.cooldown_ms = 60_000;
        assert_eq!(cfg.validate().unwrap_err().field, "switching.cooldown_ms");

        let mut cfg = sample();
        cfg.network.port = 80;
        assert_eq!(cfg.validate().unwrap_err().field, "network.port");
    }

    #[test]
    fn zero_cooldown_is_allowed_but_zero_overshoot_too() {
        // Both are legitimate for a user who wants instant switching; the
        // validator guards against nonsense, not against aggressive tuning.
        let mut cfg = sample();
        cfg.switching.cooldown_ms = 0;
        cfg.switching.edge_overshoot_points = 0.0;
        assert!(cfg.validate().is_ok());
    }
}
