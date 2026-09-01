//! Privacy permissions required to capture and inject input.

use std::fmt;

/// A privacy permission gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// macOS Accessibility.
    ///
    /// Required to create an *active* event tap — one that can suppress events —
    /// and to post synthesised events. Without it the cursor cannot be stopped
    /// locally while control is on another machine.
    Accessibility,
    /// macOS Input Monitoring.
    ///
    /// Required since macOS 10.15 to observe keyboard events at all. This is a
    /// separate grant from Accessibility, and the pair is the single most common
    /// source of "it connects but nothing types": one is granted, the other is
    /// not, and nothing in the OS explains the difference.
    InputMonitoring,
}

impl Permission {
    /// Every permission, in a stable order.
    ///
    /// Exhaustive by construction: adding a variant without adding it here fails
    /// the round-trip test in `mb-core`.
    pub const ALL: [Self; 2] = [Self::Accessibility, Self::InputMonitoring];

    /// Short label for the UI.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Accessibility => "Accessibility",
            Self::InputMonitoring => "Input Monitoring",
        }
    }

    /// Plain-language explanation of why the permission is needed.
    ///
    /// Shown verbatim in the setup workflow. Users are right to be suspicious of
    /// an application asking to watch their keyboard, and a vague justification
    /// makes that worse.
    #[must_use]
    pub const fn rationale(self) -> &'static str {
        match self {
            Self::Accessibility => {
                "Lets MouseBridge move your pointer and stop it at the edge of the \
                 screen when you switch to another computer."
            }
            Self::InputMonitoring => {
                "Lets MouseBridge read your keystrokes so it can send them to the \
                 computer you are currently controlling."
            }
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// Current state of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionStatus {
    /// Granted; the associated capability will work.
    Granted,
    /// Explicitly denied by the user. Only the user can reverse this, in System
    /// Settings — the application cannot re-prompt.
    Denied,
    /// Never requested. A prompt is still possible.
    NotDetermined,
    /// This OS has no such gate, so nothing is required.
    NotRequired,
    /// The OS returned a state we do not recognise.
    ///
    /// Treated as not usable. Assuming success here would produce exactly the
    /// silent failure this design forbids.
    Unknown,
}

impl PermissionStatus {
    /// Returns true if the capability behind this permission will work.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Granted | Self::NotRequired)
    }

    /// Returns true if prompting the user could still change the outcome.
    ///
    /// `false` for [`PermissionStatus::Denied`]: macOS will not re-prompt once
    /// denied, so the UI must direct the user to System Settings instead of
    /// showing a button that appears to do nothing.
    #[must_use]
    pub const fn can_prompt(self) -> bool {
        matches!(self, Self::NotDetermined | Self::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_granted_and_not_required_are_usable() {
        assert!(PermissionStatus::Granted.is_usable());
        assert!(PermissionStatus::NotRequired.is_usable());
        assert!(!PermissionStatus::Denied.is_usable());
        assert!(!PermissionStatus::NotDetermined.is_usable());
        // An unrecognised OS response must never be optimistically treated as a
        // grant; that is how capture fails with no visible cause.
        assert!(!PermissionStatus::Unknown.is_usable());
    }

    #[test]
    fn denied_cannot_be_reprompted() {
        assert!(!PermissionStatus::Denied.can_prompt());
        assert!(PermissionStatus::NotDetermined.can_prompt());
    }

    #[test]
    fn every_permission_has_user_facing_copy() {
        for p in [Permission::Accessibility, Permission::InputMonitoring] {
            assert!(!p.title().is_empty());
            assert!(p.rationale().len() > 40, "rationale must actually explain");
        }
    }
}
