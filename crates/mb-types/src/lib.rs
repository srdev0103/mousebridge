//! Shared primitive types for MouseBridge.
//!
//! This crate is the root of the dependency graph. It deliberately contains no
//! I/O, no async runtime, no platform-specific code and no logging, so that every
//! other crate can depend on it without pulling in a runtime.
//!
//! # Coordinate conventions
//!
//! All geometry in MouseBridge is expressed in **logical points**, never physical
//! pixels. One logical point is 1/96 inch on Windows at 100% scaling and one
//! point on macOS at 1x. Each platform layer converts at its boundary, so the
//! topology engine never sees a DPI-dependent value. See [`geom`].

#![forbid(unsafe_code)]
// Panicking helpers are the clearest way to express a test fixture that is a
// known-good constant. They stay denied in library code.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod device;
pub mod geom;
pub mod redact;

pub use device::{Arch, DeviceId, DeviceName, DeviceNameError, GlobalScreenId, OsKind, ScreenId};
pub use geom::{Edge, LogicalPoint, LogicalRect, LogicalSize, Scale};
pub use redact::Redacted;
