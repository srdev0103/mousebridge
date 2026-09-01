//! Clipboard content and the rules for keeping two machines in step.
//!
//! # The loop
//!
//! Clipboard synchronisation has one characteristic failure, and it is severe:
//!
//! ```text
//!   A copies  →  A's clipboard changes  →  send to B
//!             →  B's clipboard changes  →  B's watcher fires  →  send to A
//!             →  A's clipboard changes  →  A's watcher fires   →  send to B
//!             →  ...
//! ```
//!
//! Nothing about this is self-limiting. Each machine sees a genuine local change
//! and does the obviously correct thing with it, and the two saturate the link
//! passing the same string back and forth forever.
//!
//! [`sync::ClipboardSync`] breaks it by remembering what it last wrote from a
//! peer, and declining to send back a change that matches. See that module for
//! why content identity, rather than a suppression flag or a timer, is the right
//! thing to remember.
//!
//! # Privacy
//!
//! Clipboard contents are among the most sensitive data on a machine — they hold
//! passwords more often than anything else does. Nothing here logs, and
//! [`content::ClipboardContent`] renders as a type and a size, never a value.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod content;
pub mod sync;

pub use content::{ClipboardContent, ClipboardError, ContentHash, ImageFormat, MAX_CONTENT_LEN};
pub use sync::{ClipboardSync, SyncDecision, SyncStats};
