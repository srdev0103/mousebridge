//! Application orchestration for MouseBridge.
//!
//! At milestone 1 this crate owns configuration lifecycle, logging setup and the
//! status snapshot the user interface renders. Networking, input routing and the
//! session state machine arrive in later milestones and will be wired in here.
//!
//! The frontend never reaches past this crate. Every Tauri command is a thin
//! wrapper over a method on [`Core`], which keeps the decision-making in Rust and
//! testable without a webview.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod logging;
pub mod routing;
pub mod status;

use mb_config::{Config, ConfigError, ConfigStore};
use mb_platform::{Platform, PlatformError};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

pub use routing::{Destination, ReceiveState, Router, RouterMonitor, RouterStats};
pub use status::{
    CoreStatus, DisplayStatus, PermissionEntry, StartupNotice, permission_from_key, permission_key,
};

/// Failures that prevent the core from starting.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Configuration could not be loaded or created.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// No platform backend exists for this OS.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// Internal state was poisoned by a panic on another thread.
    ///
    /// Surfaced rather than swallowed: a poisoned lock means a panic happened
    /// somewhere, and silently continuing would hide it.
    #[error("internal state lock was poisoned by a panic")]
    Poisoned,
}

/// The running application.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    platform: Arc<dyn Platform>,
    store: ConfigStore,
    config: RwLock<Config>,
    notice: Option<StartupNotice>,
}

impl Core {
    /// Starts the core: loads configuration and binds the platform backend.
    ///
    /// The device name defaults to the machine's name on first run. If the
    /// platform cannot supply one, a safe fallback is used rather than failing:
    /// an unnamed device should still be usable.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] if no platform backend exists, or if configuration
    /// is unreadable in a way that cannot be recovered from — specifically, a
    /// file written by a newer build, which must not be silently overwritten.
    pub fn start(store: ConfigStore) -> Result<Self, CoreError> {
        let platform = mb_platform::current()?;

        let default_name = platform.host().map_or_else(
            |e| {
                warn!(error = %e, "could not read the computer name; using a fallback");
                mb_platform::host::FALLBACK_DEVICE_NAME.to_owned()
            },
            |h| h.name.as_str().to_owned(),
        );

        let outcome = store.load_or_bootstrap(&default_name)?;
        let notice = StartupNotice::from_outcome(&outcome);
        let config = outcome.into_config();

        info!(
            device = %config.device.name,
            id = %config.device.id,
            "core started"
        );

        Ok(Self {
            inner: Arc::new(Inner {
                platform,
                store,
                config: RwLock::new(config),
                notice,
            }),
        })
    }

    /// Starts the core with an explicit platform backend, for tests.
    ///
    /// # Errors
    ///
    /// As [`Core::start`].
    pub fn start_with(store: ConfigStore, platform: Arc<dyn Platform>) -> Result<Self, CoreError> {
        let default_name = platform
            .host()
            .map_or_else(|_| "Test Device".to_owned(), |h| h.name.as_str().to_owned());
        let outcome = store.load_or_bootstrap(&default_name)?;
        let notice = StartupNotice::from_outcome(&outcome);
        let config = outcome.into_config();
        Ok(Self {
            inner: Arc::new(Inner {
                platform,
                store,
                config: RwLock::new(config),
                notice,
            }),
        })
    }

    /// Returns the platform backend.
    #[must_use]
    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.inner.platform
    }

    /// Returns a copy of the current configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Poisoned`] if a panic left the lock poisoned.
    pub fn config(&self) -> Result<Config, CoreError> {
        self.inner
            .config
            .read()
            .map(|c| c.clone())
            .map_err(|_| CoreError::Poisoned)
    }

    /// Applies a change to the configuration and persists it.
    ///
    /// Validation runs *before* the change is committed to memory, so a rejected
    /// edit leaves the running configuration untouched. Persisting after
    /// validation means the file on disk can never hold a value the application
    /// would refuse to load.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Config`] if the result fails validation or cannot be
    /// written, leaving the in-memory configuration unchanged.
    pub fn update_config<F>(&self, edit: F) -> Result<Config, CoreError>
    where
        F: FnOnce(&mut Config),
    {
        let mut guard = self.inner.config.write().map_err(|_| CoreError::Poisoned)?;

        let mut candidate = guard.clone();
        edit(&mut candidate);
        candidate.validate().map_err(ConfigError::from)?;
        self.inner.store.save(&candidate)?;

        *guard = candidate.clone();
        info!("configuration updated");
        Ok(candidate)
    }

    /// Builds the snapshot the dashboard renders.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Poisoned`] if configuration state is unreadable.
    /// Display and permission failures are reported *inside* the snapshot rather
    /// than as errors, so a transient display query failure does not blank the
    /// entire dashboard.
    pub fn status(&self) -> Result<CoreStatus, CoreError> {
        let config = self.config()?;
        Ok(CoreStatus::build(
            &config,
            self.inner.platform.as_ref(),
            self.inner.store.path(),
            self.inner.notice.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_platform::mock::MockPlatform;
    use mb_platform::{Permission, PermissionStatus};

    fn core_with(platform: Arc<dyn Platform>) -> (Core, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::with_path(dir.path().join("config.toml"));
        (Core::start_with(store, platform).unwrap(), dir)
    }

    #[test]
    fn starts_and_persists_identity_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::with_path(dir.path().join("config.toml"));
        let platform: Arc<dyn Platform> = Arc::new(MockPlatform::single_display());

        let first = Core::start_with(store.clone(), Arc::clone(&platform)).unwrap();
        let id = first.config().unwrap().device.id;

        let second = Core::start_with(store, platform).unwrap();
        assert_eq!(second.config().unwrap().device.id, id);
    }

    #[test]
    fn rejected_edits_leave_running_config_untouched() {
        let (core, _dir) = core_with(Arc::new(MockPlatform::single_display()));
        let before = core.config().unwrap();

        let err = core.update_config(|c| c.switching.cooldown_ms = 999_999);
        assert!(err.is_err());
        assert_eq!(
            core.config().unwrap(),
            before,
            "in-memory config was mutated"
        );
    }

    #[test]
    fn accepted_edits_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::with_path(dir.path().join("config.toml"));
        let platform: Arc<dyn Platform> = Arc::new(MockPlatform::single_display());

        let core = Core::start_with(store.clone(), Arc::clone(&platform)).unwrap();
        core.update_config(|c| c.switching.edge_overshoot_points = 25.0)
            .unwrap();

        let restarted = Core::start_with(store, platform).unwrap();
        assert_eq!(
            restarted.config().unwrap().switching.edge_overshoot_points,
            25.0
        );
    }

    #[test]
    fn status_reports_blocked_sharing_when_a_permission_is_missing() {
        let platform = MockPlatform::single_display()
            .with_permission(Permission::InputMonitoring, PermissionStatus::Denied);
        let (core, _dir) = core_with(Arc::new(platform));

        let status = core.status().unwrap();
        assert!(!status.sharing_ready, "must not claim readiness");
        let entry = status
            .permissions
            .iter()
            .find(|p| p.id == "input-monitoring")
            .unwrap();
        assert!(!entry.granted);
        assert!(
            !entry.can_prompt,
            "denied permissions cannot be re-prompted"
        );
    }

    #[test]
    fn status_survives_a_display_query_failure() {
        // A transient display failure must degrade one section, not the dashboard.
        let (core, _dir) = core_with(Arc::new(MockPlatform::single_display().failing_displays()));
        let status = core.status().unwrap();
        assert!(status.displays.is_empty());
        assert!(status.display_error.is_some());
        assert!(
            !status.device.name.is_empty(),
            "rest of the snapshot is intact"
        );
    }
}
