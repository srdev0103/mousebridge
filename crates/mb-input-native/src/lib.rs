//! Operating-system input backends.
//!
//! Each backend implements [`mb_input::InputCapture`] and
//! [`mb_input::InputInject`]. Nothing outside this crate is
//! `#[cfg(target_os = ...)]`, and nothing here is reachable from generic code
//! except through those traits.
//!
//! `unsafe` is confined to the modules that call the OS directly, and each block
//! carries a `SAFETY` comment. Pure logic — key mapping, event field conversion,
//! scroll normalisation — is separated into its own modules precisely so it can
//! be unit tested without a display server, a permission grant, or a human.
//!
//! # Every platform's pure logic compiles everywhere
//!
//! Only the modules that call the OS are `#[cfg]`-gated. The key tables and field
//! conversions for *both* platforms build and test on *either* host. That is not
//! tidiness: the two key tables are written by hand from different sources, and
//! the test that checks they agree about a given key can only exist if both are
//! compiled at once. Gating them by host would mean the Windows tables were never
//! exercised on a macOS machine, and vice versa — each table would round-trip
//! perfectly on its own while disagreeing with the other, and a keystroke
//! crossing between platforms would land on the wrong key.
//!
//! # Validation status
//!
//! Behaviour that touches the OS cannot be verified by `cargo test`. Everything
//! that needs a person at a real machine is listed in
//! `docs/platform-validation.md`, and nothing there may be described as working
//! until someone has run it.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod macos;
pub mod windows;

/// Returns the capture backend for this platform.
///
/// # Errors
///
/// Returns [`mb_input::InputError::Unsupported`] on a platform with no backend,
/// so an unsupported build fails loudly instead of silently capturing nothing.
pub fn capture() -> Result<Box<dyn mb_input::InputCapture>, mb_input::InputError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacCapture::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsCapture::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(mb_input::InputError::Unsupported {
            what: "input capture",
        })
    }
}

/// Returns the injection backend for this platform, wrapped in validation.
///
/// The [`mb_input::Validated`] wrapper is applied here rather than left to the
/// caller: injected events arrive from the network, and a non-finite coordinate
/// reaching a platform injection call is undefined behaviour in whatever
/// application has focus. Handing out a bare injector would make that a matter of
/// remembering.
///
/// # Errors
///
/// Returns [`mb_input::InputError::Unsupported`] on a platform with no backend,
/// or a platform error if the backend could not be initialised.
pub fn inject() -> Result<Box<dyn mb_input::InputInject>, mb_input::InputError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(mb_input::Validated::new(
            macos::MacInjector::new()?
        )))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(mb_input::Validated::new(
            windows::WindowsInjector::new(),
        )))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(mb_input::InputError::Unsupported {
            what: "input injection",
        })
    }
}
