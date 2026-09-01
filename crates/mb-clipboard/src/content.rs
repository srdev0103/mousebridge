//! What the clipboard holds.

use mb_types::Redacted;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// Largest clipboard payload that will be synchronised.
///
/// Sixteen mebibytes: comfortably above any text a person copies and above a
/// screenshot, while bounded enough that a peer cannot exhaust memory by copying
/// a very large image. Anything above this is left alone rather than truncated —
/// a silently shortened clipboard is worse than one that did not sync.
pub const MAX_CONTENT_LEN: usize = 16 * 1024 * 1024;

/// A content-identity hash.
///
/// Used to recognise "this is the thing I just wrote", which is how the
/// synchronisation loop is broken. Never used for a security decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Renders the first eight hex characters, for diagnostics.
    ///
    /// Safe to log: it identifies a clipboard entry without revealing anything
    /// about what it contains.
    #[must_use]
    pub fn short(&self) -> String {
        self.0.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
}

/// Why clipboard content was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClipboardError {
    /// The payload exceeded [`MAX_CONTENT_LEN`].
    #[error("clipboard content of {size} bytes exceeds the {MAX_CONTENT_LEN} byte limit")]
    TooLarge {
        /// Size offered.
        size: usize,
    },
    /// The payload claimed to be text but was not valid UTF-8.
    #[error("clipboard text was not valid UTF-8")]
    InvalidText,
    /// The payload was empty.
    ///
    /// An empty clipboard is a legitimate state, but it is not something worth
    /// sending: applying it would clear the other machine's clipboard for no
    /// reason the user asked for.
    #[error("clipboard content is empty")]
    Empty,
}

/// Image formats carried between machines.
///
/// PNG only. Both platforms can produce and consume it, it is lossless, and
/// picking one format avoids a negotiation that would otherwise have to happen
/// on every copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    /// PNG.
    Png,
}

/// Something on the clipboard.
///
/// Neither variant renders its contents. See the crate documentation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardContent {
    /// Unicode text.
    Text(Redacted<String>),
    /// An image.
    Image {
        /// Encoding.
        format: ImageFormat,
        /// Encoded bytes.
        data: Redacted<Vec<u8>>,
    },
}

impl ClipboardContent {
    /// Wraps text, validating its size.
    ///
    /// # Errors
    ///
    /// [`ClipboardError::Empty`] or [`ClipboardError::TooLarge`].
    pub fn text(value: impl Into<String>) -> Result<Self, ClipboardError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ClipboardError::Empty);
        }
        if value.len() > MAX_CONTENT_LEN {
            return Err(ClipboardError::TooLarge { size: value.len() });
        }
        Ok(Self::Text(Redacted::new(value)))
    }

    /// Wraps an image, validating its size.
    ///
    /// # Errors
    ///
    /// [`ClipboardError::Empty`] or [`ClipboardError::TooLarge`].
    pub fn image(format: ImageFormat, data: Vec<u8>) -> Result<Self, ClipboardError> {
        if data.is_empty() {
            return Err(ClipboardError::Empty);
        }
        if data.len() > MAX_CONTENT_LEN {
            return Err(ClipboardError::TooLarge { size: data.len() });
        }
        Ok(Self::Image {
            format,
            data: Redacted::new(data),
        })
    }

    /// Validates content that arrived from a peer.
    ///
    /// The size limit is enforced again here rather than trusted from the
    /// sending side, because the sending side is another machine.
    ///
    /// # Errors
    ///
    /// As the constructors above.
    pub fn validate(&self) -> Result<(), ClipboardError> {
        let size = self.len();
        if size == 0 {
            return Err(ClipboardError::Empty);
        }
        if size > MAX_CONTENT_LEN {
            return Err(ClipboardError::TooLarge { size });
        }
        Ok(())
    }

    /// Size in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.expose().len(),
            Self::Image { data, .. } => data.expose().len(),
        }
    }

    /// True when there is nothing on the clipboard.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A short name for the content type, safe to log.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Image { .. } => "image",
        }
    }

    /// The text, if this is text.
    ///
    /// Every call site is a deliberate decision to handle clipboard contents.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.expose()),
            Self::Image { .. } => None,
        }
    }

    /// The image bytes, if this is an image.
    #[must_use]
    pub fn as_image(&self) -> Option<(ImageFormat, &[u8])> {
        match self {
            Self::Image { format, data } => Some((*format, data.expose())),
            Self::Text(_) => None,
        }
    }

    /// Content identity, for recognising what was just written.
    ///
    /// The type is hashed alongside the bytes so that text and an image that
    /// happen to share a byte sequence are never confused for each other.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        let mut hasher = Sha256::new();
        match self {
            Self::Text(text) => {
                hasher.update([0u8]);
                hasher.update(text.expose().as_bytes());
            }
            Self::Image { format, data } => {
                hasher.update([1u8]);
                hasher.update(match format {
                    ImageFormat::Png => [0u8],
                });
                hasher.update(data.expose());
            }
        }
        let digest = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        ContentHash(bytes)
    }
}

/// Renders the type and size only. Never the contents.
impl fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClipboardContent::{}({} bytes)", self.kind(), self.len())
    }
}

impl fmt::Display for ClipboardContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({} bytes)", self.kind(), self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        let content = ClipboardContent::text("hello").expect("valid");
        assert_eq!(content.as_text(), Some("hello"));
        assert_eq!(content.len(), 5);
        assert_eq!(content.kind(), "text");
    }

    #[test]
    fn unicode_survives() {
        // The clipboard is full of things that are not ASCII.
        for value in ["日本語のテキスト", "emoji 🎉 and combining é", "Ünïcödé"] {
            let content = ClipboardContent::text(value).expect("valid");
            assert_eq!(content.as_text(), Some(value));
        }
    }

    #[test]
    fn images_round_trip() {
        let content =
            ClipboardContent::image(ImageFormat::Png, vec![0x89, 0x50, 0x4E, 0x47]).expect("valid");
        assert_eq!(content.as_image().map(|(f, _)| f), Some(ImageFormat::Png));
        assert_eq!(content.as_image().map(|(_, d)| d.len()), Some(4));
        assert!(content.as_text().is_none());
    }

    #[test]
    fn empty_content_is_refused() {
        // Applying an empty clipboard would clear the other machine's for no
        // reason the user asked for.
        assert_eq!(ClipboardContent::text(""), Err(ClipboardError::Empty));
        assert_eq!(
            ClipboardContent::image(ImageFormat::Png, vec![]),
            Err(ClipboardError::Empty)
        );
    }

    #[test]
    fn oversized_content_is_refused_rather_than_truncated() {
        // A silently shortened clipboard is worse than one that did not sync:
        // the user pastes something subtly wrong and may not notice.
        let huge = "a".repeat(MAX_CONTENT_LEN + 1);
        assert!(matches!(
            ClipboardContent::text(huge),
            Err(ClipboardError::TooLarge { .. })
        ));
    }

    #[test]
    fn content_at_the_limit_is_accepted() {
        let exact = "a".repeat(MAX_CONTENT_LEN);
        assert!(ClipboardContent::text(exact).is_ok());
    }

    #[test]
    fn remote_content_is_re_validated() {
        // The size limit is checked again on receipt, because the sending side
        // is another machine and its claim is not evidence.
        let content = ClipboardContent::Text(Redacted::new(String::new()));
        assert_eq!(content.validate(), Err(ClipboardError::Empty));

        let content = ClipboardContent::Text(Redacted::new("a".repeat(MAX_CONTENT_LEN + 1)));
        assert!(matches!(
            content.validate(),
            Err(ClipboardError::TooLarge { .. })
        ));
    }

    #[test]
    fn contents_never_appear_in_debug_or_display() {
        // Clipboards hold passwords more often than they hold anything else.
        let content = ClipboardContent::text("correct horse battery staple").expect("valid");
        let debug = format!("{content:?}");
        let display = format!("{content}");

        assert!(!debug.contains("horse"), "Debug leaked: {debug}");
        assert!(!display.contains("horse"), "Display leaked: {display}");
        assert_eq!(debug, "ClipboardContent::text(28 bytes)");
        assert_eq!(display, "text (28 bytes)");
    }

    #[test]
    fn contents_do_not_leak_through_a_containing_struct() {
        #[derive(Debug)]
        #[allow(dead_code, reason = "read through the derived Debug impl")]
        struct Message {
            from: &'static str,
            content: ClipboardContent,
        }
        let message = Message {
            from: "peer",
            content: ClipboardContent::text("hunter2").expect("valid"),
        };
        let rendered = format!("{message:?}");
        assert!(!rendered.contains("hunter2"), "leaked: {rendered}");
        assert!(rendered.contains("peer"), "diagnostics still work");
    }

    #[test]
    fn identical_content_hashes_identically() {
        let a = ClipboardContent::text("same").expect("valid");
        let b = ClipboardContent::text("same").expect("valid");
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn different_content_hashes_differently() {
        let a = ClipboardContent::text("one").expect("valid");
        let b = ClipboardContent::text("two").expect("valid");
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn text_and_an_image_with_the_same_bytes_do_not_collide() {
        // Otherwise copying an image whose bytes happen to spell some text would
        // be mistaken for that text, and the loop-breaking logic would suppress
        // a genuine change.
        let text = ClipboardContent::text("PNG").expect("valid");
        let image = ClipboardContent::image(ImageFormat::Png, b"PNG".to_vec()).expect("valid");
        assert_ne!(text.hash(), image.hash());
    }

    #[test]
    fn a_hash_summary_reveals_nothing() {
        let content = ClipboardContent::text("secret").expect("valid");
        let short = content.hash().short();
        assert_eq!(short.len(), 8);
        assert!(!short.contains("secret"));
    }
}
