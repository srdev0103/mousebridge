//! Platform-independent input events, and the traits platform backends implement.
//!
//! # Structural guarantees
//!
//! This crate has **no logger and no async runtime**, and that is enforced by the
//! dependency graph rather than by convention:
//!
//! * No `tracing` dependency means no code path here can log a keystroke.
//!   Rendering which keys a user pressed is keylogging whatever the log level, so
//!   the ability is removed rather than discouraged.
//! * No async runtime means the input path cannot await, block on a reactor, or
//!   inherit an executor's scheduling latency.
//!
//! [`state::InputStateTracker::apply`] performs no allocation, which is what lets
//! it run directly on the OS capture thread.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod capture;
pub mod error;
pub mod event;
pub mod inject;
pub mod keycode;
pub mod modifiers;
pub mod state;
pub mod virtual_backend;

pub use capture::{Disposition, InputCapture, InputSink};
pub use error::InputError;
pub use event::{HeldInput, InputEvent, ScrollDelta};
pub use inject::{InputInject, Validated, inject_all, release_tracked};
pub use keycode::{KeyCode, PAGE_CONSUMER, PAGE_KEYBOARD, keys};
pub use modifiers::{Modifiers, MouseButton, MouseButtons};
pub use state::{ApplyOutcome, InputStateTracker, MAX_HELD_KEYS};
pub use virtual_backend::{
    CaptureStats, Destination, Loopback, RoutingSink, VirtualCapture, VirtualInjector,
};
