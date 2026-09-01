//! Errors from input capture and injection.

/// A failure in the input layer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// The OS refused because a required permission is missing.
    ///
    /// Distinct from every other error because it is the only one the *user* can
    /// fix, and the UI must route it to the permission workflow rather than
    /// showing a generic failure.
    #[error("{operation} requires the {permission} permission")]
    PermissionDenied {
        /// What was attempted.
        operation: &'static str,
        /// The permission needed, as a display name.
        permission: &'static str,
    },

    /// The OS revoked an active capture.
    ///
    /// macOS disables an event tap whose callback ran too slowly, and Windows
    /// drops a low-level hook that exceeded `LowLevelHooksTimeout`. Both are
    /// recoverable by re-arming, and both are silent unless explicitly detected —
    /// which is why this is a distinct variant rather than a generic failure.
    #[error("input capture was disabled by the operating system: {reason}")]
    CaptureDisabled {
        /// Why the OS disabled it.
        reason: &'static str,
    },

    /// Another process is blocking input capture.
    ///
    /// On macOS a password field enables Secure Input, which suppresses every
    /// keyboard tap system-wide. Nothing can be done about it except tell the
    /// user what is happening and which process is responsible.
    #[error("another application is blocking keyboard capture{}", match .blocking_pid {
        Some(pid) => format!(" (process {pid})"),
        None => String::new(),
    })]
    Blocked {
        /// The responsible process, when the OS identifies it.
        blocking_pid: Option<u32>,
    },

    /// The operation needs capture to be running, and it is not.
    #[error("input capture is not running")]
    NotRunning,

    /// The event contained a value that must never reach an OS input API.
    ///
    /// Chiefly non-finite coordinates. Events arrive from the network, and a NaN
    /// passed to a platform injection call is undefined behaviour in the
    /// receiving application, not a cosmetic glitch.
    #[error("refusing to inject a malformed event: {reason}")]
    Malformed {
        /// What was wrong.
        reason: &'static str,
    },

    /// An OS call failed.
    #[error("{api} failed: {detail}")]
    OsCall {
        /// The API that failed.
        api: &'static str,
        /// Detail from the OS.
        detail: String,
    },

    /// Not implemented on this platform.
    #[error("{what} is not supported on this platform")]
    Unsupported {
        /// What was attempted.
        what: &'static str,
    },
}

impl InputError {
    /// True if re-arming capture could plausibly recover.
    ///
    /// Drives the supervisor's retry decision: a disabled tap should be re-armed
    /// immediately, while a missing permission should not be retried in a loop.
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::CaptureDisabled { .. } | Self::NotRunning)
    }

    /// True if only the user can resolve this, via the OS.
    #[must_use]
    pub const fn needs_user_action(&self) -> bool {
        matches!(self, Self::PermissionDenied { .. } | Self::Blocked { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_and_user_actionable_are_disjoint() {
        let errors = [
            InputError::PermissionDenied {
                operation: "capture",
                permission: "Accessibility",
            },
            InputError::CaptureDisabled { reason: "timeout" },
            InputError::Blocked {
                blocking_pid: Some(42),
            },
            InputError::NotRunning,
            InputError::Malformed { reason: "NaN" },
        ];
        for e in errors {
            assert!(
                !(e.is_recoverable() && e.needs_user_action()),
                "{e} is both auto-recoverable and user-actionable"
            );
        }
    }

    #[test]
    fn a_retry_loop_cannot_spin_on_a_permission_error() {
        let e = InputError::PermissionDenied {
            operation: "capture",
            permission: "Input Monitoring",
        };
        assert!(!e.is_recoverable(), "would retry forever without a grant");
        assert!(e.needs_user_action());
    }

    #[test]
    fn messages_name_the_blocking_process_when_known() {
        let known = InputError::Blocked {
            blocking_pid: Some(913),
        };
        assert!(known.to_string().contains("913"));
        let unknown = InputError::Blocked { blocking_pid: None };
        assert!(!unknown.to_string().contains("None"));
    }
}
