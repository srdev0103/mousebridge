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
//! # Validation status
//!
//! Behaviour that touches the OS cannot be verified by `cargo test`. Everything
//! that needs a person at a real machine is listed in
//! `docs/platform-validation.md`, and nothing there may be described as working
//! until someone has run it.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(target_os = "macos")]
pub mod macos;

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
    #[cfg(not(target_os = "macos"))]
    {
        Err(mb_input::InputError::Unsupported {
            what: "input capture",
        })
    }
}
