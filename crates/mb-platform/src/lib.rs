//! OS environment abstraction: displays, permissions and host identity.
//!
//! This crate answers questions *about* the machine. Capturing and injecting
//! input is a separate concern with a very different lifetime, and lives in
//! `mb-input`.
//!
//! Everything platform-specific sits behind [`Platform`]. Generic code depends on
//! the trait, never on `#[cfg(target_os = ...)]`, so the topology engine and the
//! UI can be exercised on any host through [`mock::MockPlatform`].
//!
//! # Platform validation status
//!
//! | Area | macOS | Windows |
//! |---|---|---|
//! | Display enumeration | verified on macOS 15 | **compile-checked only** |
//! | Host name | verified on macOS 15 | **compile-checked only** |
//! | Permissions | verified on macOS 15 | not applicable |
//!
//! Windows code in this crate is type-checked from the development Mac via
//! `cargo check --target x86_64-pc-windows-msvc`, which catches signature and
//! type errors but proves nothing about runtime behaviour. Every item marked
//! above is listed in `docs/platform-validation.md` and stays unverified until a
//! human has run it on the target OS.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod display;
pub mod error;
pub mod host;
pub mod mock;
pub mod permission;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::Arc;

pub use display::{DisplayInfo, DisplayLayout};
pub use error::PlatformError;
pub use host::HostInfo;
pub use permission::{Permission, PermissionStatus};

/// Queries about the machine this process is running on.
///
/// Implementations must be cheap to call and must never block for longer than a
/// UI frame: the dashboard polls permission status while a settings pane is open,
/// because neither macOS nor Windows notifies an application when a privacy
/// grant changes.
pub trait Platform: Send + Sync + 'static {
    /// Returns the machine's name, OS family and architecture.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] if the OS refused the query.
    fn host(&self) -> Result<HostInfo, PlatformError>;

    /// Enumerates the currently attached displays.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] if the display list could not be read.
    fn displays(&self) -> Result<DisplayLayout, PlatformError>;

    /// Returns the current status of a privacy permission.
    fn permission_status(&self, permission: Permission) -> PermissionStatus;

    /// Asks the OS to prompt for a permission, if it supports prompting.
    ///
    /// Returns `Ok(())` once the request has been *made*. It does not mean the
    /// permission was granted: on macOS the user must act in System Settings and
    /// the application usually has to be relaunched, so the caller must keep
    /// polling [`Platform::permission_status`] rather than assuming success.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] if the request could not be issued.
    fn request_permission(&self, permission: Permission) -> Result<(), PlatformError>;

    /// Opens the OS settings pane for a permission.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError`] if the settings application could not be launched.
    fn open_permission_settings(&self, permission: Permission) -> Result<(), PlatformError>;

    /// Permissions this platform requires before input can be captured.
    ///
    /// Empty on platforms with no equivalent gate.
    fn required_permissions(&self) -> &'static [Permission];

    /// Returns the required permissions that are not currently granted.
    ///
    /// The UI uses this to decide whether to show the setup workflow. Sharing
    /// must never be enabled while this is non-empty: capture would fail with no
    /// visible cause, which is exactly the silent failure mode to avoid.
    fn missing_permissions(&self) -> Vec<Permission> {
        self.required_permissions()
            .iter()
            .copied()
            .filter(|p| !self.permission_status(*p).is_usable())
            .collect()
    }
}

/// Returns the platform implementation for the host this binary runs on.
///
/// # Errors
///
/// Returns [`PlatformError::Unsupported`] on an OS with no native backend, so
/// that an unsupported build fails loudly at startup rather than silently
/// capturing nothing.
pub fn current() -> Result<Arc<dyn Platform>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(macos::MacPlatform::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Arc::new(windows::WindowsPlatform::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(PlatformError::Unsupported {
            what: "platform backend",
        })
    }
}
