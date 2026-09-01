//! An in-memory [`Platform`] for tests and for exercising the UI without hardware.
//!
//! This is not a stub that returns empty values to satisfy the trait: it is a
//! configurable fake with real behaviour, so that topology, permission-gating and
//! dashboard logic can be tested against multi-monitor and denied-permission
//! scenarios that are impractical to reproduce on demand on real hardware.

use crate::{
    DisplayInfo, DisplayLayout, HostInfo, Permission, PermissionStatus, Platform, PlatformError,
};
use mb_types::{Arch, DeviceName, LogicalRect, OsKind, Scale, ScreenId};
use std::collections::HashMap;
use std::sync::Mutex;

/// A configurable in-memory platform.
#[derive(Debug)]
pub struct MockPlatform {
    host: HostInfo,
    displays: Mutex<DisplayLayout>,
    permissions: Mutex<HashMap<Permission, PermissionStatus>>,
    required: &'static [Permission],
    /// Number of times a permission prompt was requested, so tests can assert
    /// the UI does not re-prompt for an already-denied permission.
    prompts: Mutex<Vec<Permission>>,
    fail_displays: bool,
}

impl MockPlatform {
    /// A single 1920x1080 display at 1x, all permissions granted.
    #[must_use]
    pub fn single_display() -> Self {
        Self::with_displays(vec![DisplayInfo {
            id: ScreenId(1),
            bounds: rect(0.0, 0.0, 1920.0, 1080.0),
            scale: Scale::ONE,
            is_primary: true,
            name: Some("Mock Display".to_owned()),
        }])
    }

    /// A 2x Retina laptop panel beside a 1x external display.
    ///
    /// The common mixed-DPI arrangement, and the one that breaks naive
    /// pixel-based topology maths.
    #[must_use]
    pub fn mixed_dpi_dual_display() -> Self {
        Self::with_displays(vec![
            DisplayInfo {
                id: ScreenId(1),
                bounds: rect(0.0, 0.0, 1512.0, 982.0),
                scale: Scale::new(2.0).unwrap_or(Scale::ONE),
                is_primary: true,
                name: Some("Built-in Display".to_owned()),
            },
            DisplayInfo {
                id: ScreenId(2),
                bounds: rect(1512.0, 0.0, 2560.0, 1440.0),
                scale: Scale::ONE,
                is_primary: false,
                name: Some("External Display".to_owned()),
            },
        ])
    }

    /// Builds a mock with an explicit display list.
    #[must_use]
    pub fn with_displays(displays: Vec<DisplayInfo>) -> Self {
        let mut permissions = HashMap::new();
        permissions.insert(Permission::Accessibility, PermissionStatus::Granted);
        permissions.insert(Permission::InputMonitoring, PermissionStatus::Granted);
        Self {
            host: HostInfo {
                name: DeviceName::new("Mock Device")
                    .unwrap_or_else(|_| unreachable!("literal is a valid device name")),
                os: OsKind::current(),
                arch: Arch::current(),
                os_version: "0.0-mock".to_owned(),
            },
            displays: Mutex::new(DisplayLayout::new(displays)),
            permissions: Mutex::new(permissions),
            required: &[Permission::Accessibility, Permission::InputMonitoring],
            prompts: Mutex::new(Vec::new()),
            fail_displays: false,
        }
    }

    /// Overrides a permission's status.
    #[must_use]
    pub fn with_permission(self, permission: Permission, status: PermissionStatus) -> Self {
        if let Ok(mut map) = self.permissions.lock() {
            map.insert(permission, status);
        }
        self
    }

    /// Declares that no permissions are required, as on Windows.
    #[must_use]
    pub const fn with_no_required_permissions(mut self) -> Self {
        self.required = &[];
        self
    }

    /// Makes display enumeration fail, to exercise error paths.
    #[must_use]
    pub const fn failing_displays(mut self) -> Self {
        self.fail_displays = true;
        self
    }

    /// Replaces the display list, simulating a hotplug or resolution change.
    pub fn set_displays(&self, displays: Vec<DisplayInfo>) {
        if let Ok(mut guard) = self.displays.lock() {
            *guard = DisplayLayout::new(displays);
        }
    }

    /// Returns the permissions that were prompted for, in order.
    #[must_use]
    pub fn prompt_log(&self) -> Vec<Permission> {
        self.prompts.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
    LogicalRect::from_parts(x, y, w, h)
        .unwrap_or_else(|| unreachable!("mock fixtures use positive dimensions"))
}

impl Platform for MockPlatform {
    fn host(&self) -> Result<HostInfo, PlatformError> {
        Ok(self.host.clone())
    }

    fn displays(&self) -> Result<DisplayLayout, PlatformError> {
        if self.fail_displays {
            return Err(PlatformError::os_call("MockPlatform", "configured to fail"));
        }
        self.displays
            .lock()
            .map(|g| g.clone())
            .map_err(|_| PlatformError::os_call("MockPlatform", "display lock poisoned"))
    }

    fn permission_status(&self, permission: Permission) -> PermissionStatus {
        self.permissions
            .lock()
            .ok()
            .and_then(|m| m.get(&permission).copied())
            .unwrap_or(PermissionStatus::Unknown)
    }

    fn request_permission(&self, permission: Permission) -> Result<(), PlatformError> {
        if let Ok(mut log) = self.prompts.lock() {
            log.push(permission);
        }
        Ok(())
    }

    fn open_permission_settings(&self, _permission: Permission) -> Result<(), PlatformError> {
        Ok(())
    }

    fn required_permissions(&self) -> &'static [Permission] {
        self.required
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_permissions_reports_only_ungranted_ones() {
        let p = MockPlatform::single_display()
            .with_permission(Permission::InputMonitoring, PermissionStatus::Denied);
        assert_eq!(p.missing_permissions(), vec![Permission::InputMonitoring]);
    }

    #[test]
    fn unknown_status_counts_as_missing() {
        // The whole point of the Unknown variant: never optimistic.
        let p = MockPlatform::single_display()
            .with_permission(Permission::Accessibility, PermissionStatus::Unknown);
        assert_eq!(p.missing_permissions(), vec![Permission::Accessibility]);
    }

    #[test]
    fn a_platform_with_no_gates_is_never_blocked() {
        let p = MockPlatform::single_display().with_no_required_permissions();
        assert!(p.missing_permissions().is_empty());
    }

    #[test]
    fn mixed_dpi_fixture_is_coherent() {
        let p = MockPlatform::mixed_dpi_dual_display();
        let layout = p.displays().unwrap();
        assert_eq!(layout.len(), 2);
        // Adjacent, not overlapping: the seam is a valid crossing point.
        let bb = layout.bounding_box().unwrap();
        assert_eq!(bb.max_x(), 1512.0 + 2560.0);
        assert!(layout.displays()[0].scale.get() > layout.displays()[1].scale.get());
    }

    #[test]
    fn hotplug_is_observable() {
        let p = MockPlatform::single_display();
        assert_eq!(p.displays().unwrap().len(), 1);
        p.set_displays(vec![]);
        assert!(p.displays().unwrap().is_empty(), "display disconnect");
    }

    #[test]
    fn display_failure_is_surfaced_not_swallowed() {
        let p = MockPlatform::single_display().failing_displays();
        assert!(p.displays().is_err());
    }
}
