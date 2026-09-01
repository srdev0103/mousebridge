//! Windows input backend.
//!
//! `keymap` and `convert` are pure logic and build on every host, so their tests
//! run everywhere. Only the modules that call the Win32 API are gated.

pub mod convert;
pub mod keymap;

#[cfg(target_os = "windows")]
pub mod hook;
#[cfg(target_os = "windows")]
pub mod inject;

#[cfg(target_os = "windows")]
pub use hook::{CaptureDiagnostics, INJECTION_MARKER, WindowsCapture};
#[cfg(target_os = "windows")]
pub use inject::WindowsInjector;
