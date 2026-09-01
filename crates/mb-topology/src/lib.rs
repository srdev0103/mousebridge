//! Where the screens are, and when the cursor crosses between them.
//!
//! # One plane
//!
//! Every screen of every device is placed as a rectangle in a single shared 2D
//! space. That one decision makes multi-monitor, mismatched resolutions, Retina
//! panels and Windows fractional scaling all the same problem: a cursor position
//! is a point, a screen is a rectangle, and crossing is a geometric question.
//!
//! # What this crate does not know
//!
//! Nothing about devices, connections, or input. It answers exactly one
//! question — given where the cursor was and how far it moved, where is it now
//! and did it change screens — and leaves acting on the answer to the caller.
//! That is what makes the anti-jitter rules, which are the fiddly part,
//! exhaustively testable.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod arrange;
pub mod cursor;
pub mod layout;
pub mod mapping;

pub use arrange::{SNAP_DISTANCE, SnapResult, normalise, overlaps_any, snap};
pub use cursor::{CursorTracker, Movement, SwitchingRules};
pub use layout::{Layout, LayoutError, PlacedScreen};
pub use mapping::{DeviceScreens, ScreenMapping};
