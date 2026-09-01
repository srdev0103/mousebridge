//! Protocol versioning and negotiation.

use crate::error::ProtocolError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The protocol version this build speaks.
///
/// Bump on any change to a wire layout, a field's meaning, or the set of
/// messages a peer must understand. Adding an optional field that older peers can
/// ignore does not require a bump; removing or reinterpreting one always does.
pub const PROTOCOL_VERSION: Version = Version(1);

/// Oldest version this build can still talk to.
pub const MIN_SUPPORTED: Version = Version(1);

/// A protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Version(pub u16);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// The inclusive range of versions a peer accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    /// Oldest accepted.
    pub min: Version,
    /// Newest accepted, and the one preferred.
    pub max: Version,
}

impl VersionRange {
    /// The range this build supports.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            min: MIN_SUPPORTED,
            max: PROTOCOL_VERSION,
        }
    }

    /// Builds a range, rejecting an inverted one.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::Invalid`] if `min` exceeds `max`. A peer
    /// advertising an inverted range is malfunctioning or probing; accepting it
    /// would make the negotiation below produce nonsense.
    pub const fn new(min: Version, max: Version) -> Result<Self, ProtocolError> {
        if min.0 > max.0 {
            return Err(ProtocolError::Invalid {
                field: "version range",
                reason: "minimum is greater than maximum",
            });
        }
        Ok(Self { min, max })
    }

    /// Returns the highest version both peers accept.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::VersionMismatch`] when the ranges do not
    /// overlap. This is deliberately fatal and deliberately specific: the UI has
    /// to be able to say "update MouseBridge on the other computer" rather than
    /// showing a connection that fails for no visible reason.
    pub const fn negotiate(self, peer: Self) -> Result<Version, ProtocolError> {
        let lower_max = if self.max.0 < peer.max.0 {
            self.max
        } else {
            peer.max
        };
        let higher_min = if self.min.0 > peer.min.0 {
            self.min
        } else {
            peer.min
        };
        if higher_min.0 > lower_max.0 {
            return Err(ProtocolError::VersionMismatch {
                ours: self,
                theirs: peer,
            });
        }
        Ok(lower_max)
    }

    /// True if this range accepts `version`.
    #[must_use]
    pub const fn accepts(self, version: Version) -> bool {
        version.0 >= self.min.0 && version.0 <= self.max.0
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{}-{}", self.min, self.max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: u16, max: u16) -> VersionRange {
        VersionRange::new(Version(min), Version(max)).expect("valid range")
    }

    #[test]
    fn identical_ranges_agree_on_the_shared_maximum() {
        assert_eq!(range(1, 3).negotiate(range(1, 3)), Ok(Version(3)));
    }

    #[test]
    fn negotiation_picks_the_highest_version_both_accept() {
        // A newer build talking to an older one must step down, not refuse.
        assert_eq!(range(1, 5).negotiate(range(1, 2)), Ok(Version(2)));
        assert_eq!(range(1, 2).negotiate(range(1, 5)), Ok(Version(2)));
    }

    #[test]
    fn negotiation_is_symmetric() {
        // Both ends compute the version independently. If they disagreed, each
        // would encode for a different layout and the session would corrupt
        // silently rather than fail.
        for (a, b) in [((1, 5), (2, 7)), ((3, 3), (1, 9)), ((1, 1), (1, 1))] {
            let ra = range(a.0, a.1);
            let rb = range(b.0, b.1);
            assert_eq!(
                ra.negotiate(rb),
                rb.negotiate(ra),
                "{ra} and {rb} disagreed"
            );
        }
    }

    #[test]
    fn disjoint_ranges_fail_with_both_sides_named() {
        // The error has to be actionable: it is shown to a user who needs to know
        // which machine to update.
        let ours = range(5, 9);
        let theirs = range(1, 3);
        let err = ours.negotiate(theirs).expect_err("no overlap");
        assert_eq!(err, ProtocolError::VersionMismatch { ours, theirs });
        let text = err.to_string();
        assert!(text.contains("v5-v9") && text.contains("v1-v3"), "{text}");
    }

    #[test]
    fn adjacent_but_disjoint_ranges_are_rejected() {
        // Off-by-one at the boundary: 1-2 and 3-4 share nothing.
        assert!(range(1, 2).negotiate(range(3, 4)).is_err());
        // But 1-3 and 3-4 share exactly version 3.
        assert_eq!(range(1, 3).negotiate(range(3, 4)), Ok(Version(3)));
    }

    #[test]
    fn an_inverted_range_is_refused_at_construction() {
        assert!(VersionRange::new(Version(5), Version(2)).is_err());
    }

    #[test]
    fn the_current_range_negotiates_with_itself() {
        let current = VersionRange::current();
        assert_eq!(current.negotiate(current), Ok(PROTOCOL_VERSION));
        assert!(current.accepts(PROTOCOL_VERSION));
    }

    #[test]
    fn accepts_covers_the_inclusive_bounds() {
        let r = range(2, 4);
        assert!(!r.accepts(Version(1)));
        assert!(r.accepts(Version(2)));
        assert!(r.accepts(Version(4)));
        assert!(!r.accepts(Version(5)));
    }
}
