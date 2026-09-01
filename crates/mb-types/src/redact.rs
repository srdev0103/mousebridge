//! Type-level protection against logging secrets.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Deref, DerefMut};

/// Wraps a value whose contents must never reach a log or an error message.
///
/// The logging rule "do not log clipboard contents, keys or file payloads" is not
/// enforceable by review alone: a single `debug!("{:?}", msg)` on a struct that
/// happens to contain a secret defeats it, and nobody notices until the log is
/// shipped to a bug report. `Redacted` moves the guarantee into the type system —
/// both [`fmt::Debug`] and [`fmt::Display`] print a placeholder, so the only way
/// to log the contents is to call [`Redacted::expose`], which is greppable.
///
/// ```
/// use mb_types::Redacted;
/// let secret = Redacted::new("hunter2");
/// assert_eq!(format!("{secret:?}"), "<redacted 7 bytes>");
/// assert_eq!(secret.expose(), &"hunter2");
/// ```
///
/// Serialization is deliberately transparent: these values still have to travel
/// over the wire and into config files. Only the *human-readable* renderings are
/// suppressed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Returns the wrapped value.
    ///
    /// Every call site is a deliberate decision to handle a secret. Grep for
    /// `.expose()` when auditing what can escape.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Consumes the wrapper and returns the value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: SizeHint> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} bytes>", self.0.size_hint())
    }
}

impl<T: SizeHint> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> Deref for Redacted<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Redacted<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> From<T> for Redacted<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

/// Reports an approximate size, so redacted values can still be diagnosed.
///
/// Knowing that a clipboard payload was 4 MB rather than 0 bytes is often the
/// whole diagnosis; knowing what it said is never necessary.
pub trait SizeHint {
    /// Returns the value's size in bytes, for logging.
    fn size_hint(&self) -> usize;
}

impl SizeHint for String {
    fn size_hint(&self) -> usize {
        self.len()
    }
}

impl SizeHint for &str {
    fn size_hint(&self) -> usize {
        self.len()
    }
}

impl SizeHint for Vec<u8> {
    fn size_hint(&self) -> usize {
        self.len()
    }
}

impl<const N: usize> SizeHint for [u8; N] {
    fn size_hint(&self) -> usize {
        N
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_leak() {
        let secret = Redacted::new(String::from("correct horse battery staple"));
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("horse"), "Debug leaked: {debug}");
        assert!(!display.contains("horse"), "Display leaked: {display}");
        assert_eq!(debug, "<redacted 28 bytes>");
        assert_eq!(display, "<redacted>");
    }

    #[test]
    fn nested_in_a_struct_still_never_leaks() {
        // The realistic failure: someone derives Debug on a message struct and
        // logs the whole thing. The wrapper has to hold the line from inside.
        #[derive(Debug)]
        #[allow(dead_code, reason = "fields are read through the derived Debug impl")]
        struct ClipboardMessage {
            len: usize,
            payload: Redacted<String>,
        }
        let msg = ClipboardMessage {
            len: 6,
            payload: Redacted::new("secret".to_owned()),
        };
        let rendered = format!("{msg:?}");
        assert!(!rendered.contains("secret"), "leaked: {rendered}");
        assert!(rendered.contains("len: 6"), "diagnostics still work");
    }

    #[test]
    fn size_hint_survives_redaction() {
        let key = Redacted::new([0u8; 32]);
        assert_eq!(format!("{key:?}"), "<redacted 32 bytes>");
    }

    #[test]
    fn expose_returns_the_value() {
        let secret = Redacted::new(vec![1u8, 2, 3]);
        assert_eq!(secret.expose(), &vec![1u8, 2, 3]);
        assert_eq!(secret.len(), 3, "Deref reaches the inner value");
    }
}
