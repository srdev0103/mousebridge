//! macOS platform backend.
//!
//! Uses CoreGraphics for display enumeration, ApplicationServices for the
//! Accessibility gate, IOKit for the Input Monitoring gate, and
//! SystemConfiguration for the user-facing computer name.

mod ffi;

use crate::host::sanitize_host_name;
use crate::{
    DisplayInfo, DisplayLayout, HostInfo, Permission, PermissionStatus, Platform, PlatformError,
};
use core_graphics::display::CGDisplay;
use mb_types::{Arch, LogicalRect, OsKind, Scale, ScreenId};
use std::process::Command;
use tracing::warn;

/// Permissions macOS requires before input can be captured or injected.
///
/// Both are needed, and they are independent. Accessibility alone lets us post
/// events but not observe keystrokes; Input Monitoring alone lets us observe but
/// not suppress. Requesting only one produces a half-working application whose
/// failure mode is invisible to the user.
const REQUIRED: &[Permission] = &[Permission::Accessibility, Permission::InputMonitoring];

/// macOS implementation of [`Platform`].
#[derive(Debug, Default)]
pub struct MacPlatform {
    _private: (),
}

impl MacPlatform {
    /// Builds the backend.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Platform for MacPlatform {
    fn host(&self) -> Result<HostInfo, PlatformError> {
        let raw = ffi::computer_name().unwrap_or_else(|| {
            warn!("SCDynamicStoreCopyComputerName returned nothing; using fallback name");
            String::new()
        });
        Ok(HostInfo {
            name: sanitize_host_name(&raw),
            os: OsKind::MacOs,
            arch: Arch::current(),
            os_version: product_version(),
        })
    }

    fn displays(&self) -> Result<DisplayLayout, PlatformError> {
        let ids = CGDisplay::active_displays().map_err(|code| {
            PlatformError::os_call("CGGetActiveDisplayList", format!("CGError {code}"))
        })?;

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();

            // CGDisplayBounds is already in logical points in the global display
            // space, which is exactly the unit the topology engine works in. The
            // scale factor is still needed so the input layer can convert back to
            // physical pixels when it injects a position.
            let Some(rect) = LogicalRect::from_parts(
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ) else {
                // A zero-sized display is not a real configuration; skipping it
                // is safer than letting a division by zero reach edge detection.
                warn!(display_id = id, "skipping display with degenerate bounds");
                continue;
            };

            let scale = display
                .display_mode()
                .and_then(|mode| {
                    let logical = mode.width();
                    (logical > 0).then(|| {
                        #[allow(
                            clippy::cast_precision_loss,
                            reason = "display dimensions are far below 2^53"
                        )]
                        let factor = mode.pixel_width() as f64 / logical as f64;
                        factor
                    })
                })
                .and_then(Scale::new)
                .unwrap_or(Scale::ONE);

            out.push(DisplayInfo {
                id: ScreenId(id),
                bounds: rect,
                scale,
                is_primary: display.is_main(),
                // CoreGraphics exposes no display name; obtaining one requires
                // walking the IOKit registry. Deferred until the topology editor
                // needs it, rather than inventing a placeholder here.
                name: None,
            });
        }

        Ok(DisplayLayout::new(out))
    }

    fn permission_status(&self, permission: Permission) -> PermissionStatus {
        match permission {
            Permission::Accessibility => {
                // AXIsProcessTrusted answers granted/not-granted only; macOS
                // exposes no way to distinguish "denied" from "never asked" for
                // this gate, so an ungranted state is reported as NotDetermined
                // and the UI offers both a prompt and a settings link.
                if ffi::is_process_trusted() {
                    PermissionStatus::Granted
                } else {
                    PermissionStatus::NotDetermined
                }
            }
            Permission::InputMonitoring => ffi::input_monitoring_status(),
        }
    }

    fn request_permission(&self, permission: Permission) -> Result<(), PlatformError> {
        match permission {
            Permission::Accessibility => {
                ffi::prompt_for_accessibility();
                Ok(())
            }
            Permission::InputMonitoring => {
                ffi::request_input_monitoring();
                Ok(())
            }
        }
    }

    fn open_permission_settings(&self, permission: Permission) -> Result<(), PlatformError> {
        // Anchors into the Privacy & Security pane. These identifiers have been
        // stable across macOS 13-15; if a future release changes them the URL
        // opens the pane without scrolling to the row, which degrades gracefully.
        let anchor = match permission {
            Permission::Accessibility => "Privacy_Accessibility",
            Permission::InputMonitoring => "Privacy_ListenEvent",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        Command::new("/usr/bin/open")
            .arg(&url)
            .spawn()
            .map(|_| ())
            .map_err(|e| PlatformError::os_call("open(1)", e.to_string()))
    }

    fn required_permissions(&self) -> &'static [Permission] {
        REQUIRED
    }
}

/// Reads the macOS product version from the system property list.
///
/// Diagnostics only, so a scan of the plist is preferable to spawning `sw_vers`
/// on every launch. Returns `"unknown"` rather than failing: the version string
/// appears in support output and is never used for a behavioural decision.
fn product_version() -> String {
    const PLIST: &str = "/System/Library/CoreServices/SystemVersion.plist";
    let Ok(text) = std::fs::read_to_string(PLIST) else {
        return "unknown".to_owned();
    };
    let Some(after_key) = text.split("<key>ProductVersion</key>").nth(1) else {
        return "unknown".to_owned();
    };
    let Some(start) = after_key.find("<string>") else {
        return "unknown".to_owned();
    };
    let rest = &after_key[start + "<string>".len()..];
    rest.find("</string>")
        .map_or_else(|| "unknown".to_owned(), |end| rest[..end].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_version_is_readable_on_this_host() {
        let v = product_version();
        assert_ne!(v, "unknown", "could not read SystemVersion.plist");
        assert!(
            v.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "unexpected version format: {v}"
        );
    }

    #[test]
    fn host_info_is_populated() {
        let host = MacPlatform::new().host().expect("host query");
        assert_eq!(host.os, OsKind::MacOs);
        assert!(!host.name.as_str().is_empty());
    }

    #[test]
    fn displays_enumerate_on_this_host() {
        let layout = MacPlatform::new().displays().expect("display enumeration");
        // A headless CI runner legitimately has no displays; assert coherence
        // rather than a specific count.
        for d in layout.displays() {
            assert!(d.bounds.size.width > 0.0);
            assert!(d.bounds.size.height > 0.0);
            assert!(d.scale.get() >= 1.0, "scale below 1x is not a real display");
        }
        if !layout.is_empty() {
            assert!(layout.primary().is_some());
            assert!(layout.bounding_box().is_some());
        }
    }

    #[test]
    fn permission_queries_do_not_panic_or_prompt() {
        // Status queries must be side-effect free: the dashboard polls them.
        let p = MacPlatform::new();
        for perm in REQUIRED {
            let status = p.permission_status(*perm);
            assert!(
                !matches!(status, PermissionStatus::NotRequired),
                "macOS gates {perm} and must never report NotRequired"
            );
        }
    }
}
