//! macOS input backend.

pub mod convert;
pub mod inject;
pub mod keymap;
pub mod tap;

pub use inject::{INJECTION_MARKER, MacInjector};
pub use tap::{CaptureDiagnostics, MacCapture};
