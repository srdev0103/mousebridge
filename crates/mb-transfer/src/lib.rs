//! Receiving files from another machine.
//!
//! # The filename comes from the peer
//!
//! That single fact shapes this crate. A paired peer is trusted to send input,
//! but a compromised or malicious one supplying a *path* is the classic route to
//! writing outside the folder the user chose — `../../.ssh/authorized_keys` is
//! the canonical example, and it is not hypothetical.
//!
//! [`path`] therefore treats every incoming name as hostile: it is reduced to a
//! single leaf component, checked against the ways a name can alias something
//! else on either platform, and the assembled destination is verified to be
//! inside the target folder before anything is opened.
//!
//! # Nothing is overwritten
//!
//! A peer sending `notes.txt` must not be able to destroy the user's
//! `notes.txt`. Colliding names are given a suffix instead.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod path;
pub mod transfer;

pub use path::{PathError, safe_file_name, unique_destination};
pub use transfer::{Transfer, TransferError, TransferEvent, TransferState};
