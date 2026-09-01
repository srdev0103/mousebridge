//! Identity of the machine this process runs on.

use mb_types::{Arch, DeviceName, OsKind};

/// Name, OS family and architecture of the local machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    /// The user-facing computer name, as other devices will see it.
    pub name: DeviceName,
    /// Operating system family.
    pub os: OsKind,
    /// CPU architecture.
    pub arch: Arch,
    /// OS version string, for diagnostics and support.
    pub os_version: String,
}

impl HostInfo {
    /// Builds host information for the compiled target, with a supplied name.
    #[must_use]
    pub fn new(name: DeviceName, os_version: String) -> Self {
        Self {
            name,
            os: OsKind::current(),
            arch: Arch::current(),
            os_version,
        }
    }

    /// A short description for the UI, e.g. `macOS 15.7.9 (arm64)`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!("{} {} ({})", self.os, self.os_version, self.arch)
    }
}

/// Fallback name used when the OS cannot supply a computer name.
///
/// Deliberately generic rather than an error: an unnamed device should still be
/// discoverable and pairable, and the user can rename it in settings.
pub const FALLBACK_DEVICE_NAME: &str = "Unnamed Computer";

/// Validates an OS-supplied computer name, falling back if it is unusable.
///
/// Computer names come from user input and sync services, so they can be empty,
/// over-long or contain control characters. Rejecting the whole startup over a
/// bad hostname would be absurd; substituting a safe default is not.
#[must_use]
pub fn sanitize_host_name(raw: &str) -> DeviceName {
    DeviceName::new(raw).unwrap_or_else(|_| {
        let truncated: String = raw
            .chars()
            .filter(|c| !c.is_control())
            .take(mb_types::device::DEVICE_NAME_MAX_CHARS)
            .collect();
        DeviceName::new(truncated).unwrap_or_else(|_| {
            DeviceName::new(FALLBACK_DEVICE_NAME).unwrap_or_else(|_| {
                unreachable!("FALLBACK_DEVICE_NAME is a compile-time valid name")
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_names_pass_through() {
        assert_eq!(
            sanitize_host_name("Anh's MacBook Pro").as_str(),
            "Anh's MacBook Pro"
        );
    }

    #[test]
    fn hostile_names_are_repaired_not_rejected() {
        assert_eq!(sanitize_host_name("").as_str(), FALLBACK_DEVICE_NAME);
        assert_eq!(sanitize_host_name("   ").as_str(), FALLBACK_DEVICE_NAME);
        assert_eq!(sanitize_host_name("Mac\nBook").as_str(), "MacBook");
        let long = "a".repeat(500);
        assert_eq!(
            sanitize_host_name(&long).as_str().chars().count(),
            mb_types::device::DEVICE_NAME_MAX_CHARS
        );
    }
}
