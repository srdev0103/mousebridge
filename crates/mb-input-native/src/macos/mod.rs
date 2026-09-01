//! macOS input backend.
//!
//! `keymap` and `convert` are pure logic and build on every host, so their tests
//! run everywhere. Only the modules that call CoreGraphics are gated.

pub mod convert;
pub mod keymap;

#[cfg(target_os = "macos")]
pub mod inject;
#[cfg(target_os = "macos")]
pub mod tap;

#[cfg(target_os = "macos")]
pub use inject::{INJECTION_MARKER, MacInjector};
#[cfg(target_os = "macos")]
pub use tap::{CaptureDiagnostics, MacCapture};
