//! Workspace automation.
//!
//! Tasks that are part of the build contract but do not belong to any crate.
//! Exposed as a library as well as a binary so the checks run under
//! `cargo test --workspace` rather than only when someone remembers to invoke
//! them.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod unsafe_audit;
