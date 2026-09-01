//! The snapshot the user interface renders.
//!
//! Everything here is `Serialize` and crosses the Tauri IPC boundary. It is a
//! deliberately flat, presentation-shaped projection of internal state rather
//! than the state itself: the frontend should never have to reimplement a rule
//! that Rust already enforces, and internal types should be free to change
//! without breaking the UI contract.

use mb_config::Config;
use mb_platform::{Permission, Platform};
use serde::Serialize;
use std::path::Path;

/// Why the previous session's settings are not in effect.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StartupNotice {
    /// A configuration was created for a first run.
    FirstRun,
    /// Settings were carried forward from an older schema version.
    Migrated {
        /// Version the file was written at.
        from_version: u32,
    },
    /// The configuration file was unreadable and has been reset.
    ///
    /// The UI must show this prominently. The user's paired devices and screen
    /// layout are gone, and saying nothing would look like an unexplained
    /// amnesia rather than a recoverable fault.
    Recovered {
        /// Where the damaged file was kept.
        backup_path: String,
        /// Why it could not be used.
        reason: String,
    },
}

impl StartupNotice {
    /// Derives a notice from a configuration load, if one is worth showing.
    #[must_use]
    pub fn from_outcome(outcome: &mb_config::LoadOutcome) -> Option<Self> {
        match outcome {
            mb_config::LoadOutcome::Loaded(_) => None,
            mb_config::LoadOutcome::Created(_) => Some(Self::FirstRun),
            mb_config::LoadOutcome::Migrated { from_version, .. } => Some(Self::Migrated {
                from_version: *from_version,
            }),
            mb_config::LoadOutcome::Recovered { backup, reason, .. } => Some(Self::Recovered {
                backup_path: backup.display().to_string(),
                reason: reason.clone(),
            }),
        }
    }
}

/// Identity of this device, as shown in the dashboard.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceStatus {
    /// Short form of the device identifier, for display only.
    pub id_short: String,
    /// User-facing device name.
    pub name: String,
    /// Operating system family, e.g. `macOS`.
    pub os: String,
    /// OS version, e.g. `15.7.9`.
    pub os_version: String,
    /// CPU architecture, e.g. `arm64`.
    pub arch: String,
}

/// One privacy permission and what the UI should offer for it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PermissionEntry {
    /// Stable machine-readable key, e.g. `accessibility`.
    pub id: String,
    /// Short label.
    pub title: String,
    /// Plain-language explanation of why it is needed.
    pub rationale: String,
    /// Whether the capability behind it will work.
    pub granted: bool,
    /// Whether a prompt could still change the outcome.
    ///
    /// When false and not granted, the UI must offer a link to System Settings
    /// instead of a prompt button, because macOS will not re-ask.
    pub can_prompt: bool,
}

/// One attached display.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DisplayStatus {
    /// Platform-assigned identifier.
    pub id: u32,
    /// Human-readable name where the OS provides one.
    pub name: Option<String>,
    /// Left edge in the device's native cursor space.
    pub x: f64,
    /// Top edge in the device's native cursor space.
    pub y: f64,
    /// Width in the device's native cursor space.
    pub width: f64,
    /// Height in the device's native cursor space.
    pub height: f64,
    /// Backing scale factor.
    pub scale: f64,
    /// Whether this is the primary display.
    pub is_primary: bool,
}

/// Everything the dashboard needs for one render.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CoreStatus {
    /// This device.
    pub device: DeviceStatus,
    /// Required permissions and their state. Empty on platforms with no gates.
    pub permissions: Vec<PermissionEntry>,
    /// Attached displays. Empty if the query failed; see `display_error`.
    pub displays: Vec<DisplayStatus>,
    /// Why the display list is empty, if a query failed.
    pub display_error: Option<String>,
    /// True only when every required permission is granted.
    ///
    /// Computed in Rust so the frontend cannot disagree with the input layer
    /// about whether capture will work.
    pub sharing_ready: bool,
    /// Absolute path of the configuration file, for the settings pane.
    pub config_path: String,
    /// Anything the user should be told about how this session started.
    pub notice: Option<StartupNotice>,
}

impl CoreStatus {
    /// Builds a snapshot from live state.
    #[must_use]
    pub fn build(
        config: &Config,
        platform: &dyn Platform,
        config_path: &Path,
        notice: Option<StartupNotice>,
    ) -> Self {
        let host = platform.host().ok();

        let device = DeviceStatus {
            id_short: config.device.id.short(),
            // The configured name wins over the OS name: the user may have
            // renamed the device in settings, and that choice must stick.
            name: config.device.name.as_str().to_owned(),
            os: host.as_ref().map_or_else(
                || mb_types::OsKind::current().to_string(),
                |h| h.os.to_string(),
            ),
            os_version: host
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |h| h.os_version.clone()),
            arch: host.as_ref().map_or_else(
                || mb_types::Arch::current().to_string(),
                |h| h.arch.to_string(),
            ),
        };

        let permissions = platform
            .required_permissions()
            .iter()
            .map(|p| {
                let status = platform.permission_status(*p);
                PermissionEntry {
                    id: permission_key(*p).to_owned(),
                    title: p.title().to_owned(),
                    rationale: p.rationale().to_owned(),
                    granted: status.is_usable(),
                    can_prompt: status.can_prompt(),
                }
            })
            .collect();

        let (displays, display_error) = match platform.displays() {
            Ok(layout) => (
                layout
                    .displays()
                    .iter()
                    .map(|d| DisplayStatus {
                        id: d.id.0,
                        name: d.name.clone(),
                        x: d.bounds.min_x(),
                        y: d.bounds.min_y(),
                        width: d.bounds.size.width,
                        height: d.bounds.size.height,
                        scale: d.scale.get(),
                        is_primary: d.is_primary,
                    })
                    .collect(),
                None,
            ),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };

        Self {
            device,
            permissions,
            displays,
            display_error,
            sharing_ready: platform.missing_permissions().is_empty(),
            config_path: config_path.display().to_string(),
            notice,
        }
    }
}

/// Stable key for a permission, used by the frontend to pick copy and icons.
///
/// Deliberately separate from [`Permission::title`], which is display text and
/// may be localised or reworded.
#[must_use]
pub const fn permission_key(permission: Permission) -> &'static str {
    match permission {
        Permission::Accessibility => "accessibility",
        Permission::InputMonitoring => "input-monitoring",
    }
}

/// Resolves a key from the frontend back to a [`Permission`].
///
/// The inverse of [`permission_key`], kept beside it so the two cannot drift.
/// The frontend sends these strings, so an unrecognised value is untrusted input
/// and yields `None` rather than a panic or a default.
#[must_use]
pub fn permission_from_key(key: &str) -> Option<Permission> {
    Permission::ALL
        .iter()
        .copied()
        .find(|p| permission_key(*p) == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_platform::PermissionStatus;
    use mb_platform::mock::MockPlatform;
    use std::path::PathBuf;

    fn config() -> Config {
        Config::bootstrap("Studio Mac").unwrap()
    }

    #[test]
    fn configured_name_wins_over_the_os_name() {
        // The mock host is called "Mock Device"; the config says "Studio Mac".
        // A user's rename must not be overwritten by the OS on every render.
        let status = CoreStatus::build(
            &config(),
            &MockPlatform::single_display(),
            &PathBuf::from("/tmp/config.toml"),
            None,
        );
        assert_eq!(status.device.name, "Studio Mac");
    }

    #[test]
    fn sharing_is_ready_only_when_every_gate_is_open() {
        let ok = CoreStatus::build(
            &config(),
            &MockPlatform::single_display(),
            &PathBuf::from("/c"),
            None,
        );
        assert!(ok.sharing_ready);

        let blocked = CoreStatus::build(
            &config(),
            &MockPlatform::single_display()
                .with_permission(Permission::Accessibility, PermissionStatus::Unknown),
            &PathBuf::from("/c"),
            None,
        );
        assert!(!blocked.sharing_ready, "Unknown must not count as ready");
    }

    #[test]
    fn a_platform_without_gates_reports_no_permissions_and_is_ready() {
        let status = CoreStatus::build(
            &config(),
            &MockPlatform::single_display().with_no_required_permissions(),
            &PathBuf::from("/c"),
            None,
        );
        assert!(status.permissions.is_empty());
        assert!(status.sharing_ready);
    }

    #[test]
    fn display_failure_degrades_one_section_only() {
        let status = CoreStatus::build(
            &config(),
            &MockPlatform::single_display().failing_displays(),
            &PathBuf::from("/c"),
            None,
        );
        assert!(status.displays.is_empty());
        assert!(status.display_error.is_some());
        assert!(!status.device.id_short.is_empty());
    }

    #[test]
    fn mixed_dpi_displays_are_reported_faithfully() {
        let status = CoreStatus::build(
            &config(),
            &MockPlatform::mixed_dpi_dual_display(),
            &PathBuf::from("/c"),
            None,
        );
        assert_eq!(status.displays.len(), 2);
        let scales: Vec<f64> = status.displays.iter().map(|d| d.scale).collect();
        assert!(scales.contains(&2.0) && scales.contains(&1.0));
    }

    #[test]
    fn permission_keys_are_stable_and_distinct() {
        // The frontend switches on these; renaming one silently breaks its copy.
        assert_eq!(permission_key(Permission::Accessibility), "accessibility");
        assert_eq!(
            permission_key(Permission::InputMonitoring),
            "input-monitoring"
        );
    }

    #[test]
    fn every_permission_key_round_trips() {
        for p in Permission::ALL {
            assert_eq!(permission_from_key(permission_key(p)), Some(p));
        }
    }

    #[test]
    fn an_unknown_key_from_the_frontend_is_rejected() {
        assert_eq!(permission_from_key("camera"), None);
        assert_eq!(permission_from_key(""), None);
    }

    #[test]
    fn a_recovery_notice_carries_the_backup_path() {
        let notice = StartupNotice::Recovered {
            backup_path: "/tmp/config.toml.corrupt-1".to_owned(),
            reason: "invalid toml".to_owned(),
        };
        let status = CoreStatus::build(
            &config(),
            &MockPlatform::single_display(),
            &PathBuf::from("/c"),
            Some(notice),
        );
        let json = serde_json::to_string(&status.notice).expect("serialises");
        assert!(
            json.contains("corrupt-1"),
            "user must be told where it went"
        );
        assert!(json.contains("recovered"));
    }
}
