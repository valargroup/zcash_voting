//! The registration marker a delegate posts on X or in a GitHub gist
//! (contract §11.5).
//!
//! ```text
//! zcash-vote-delegate v1 zvdk1…
//! ```
//!
//! A post matches when its text after [`normalize_marker_whitespace`] equals
//! this form exactly, case included.

use std::fmt;

use crate::{DelegateKey, DelegateKeyDecodeError};

/// The marker's first word.
pub const MARKER_PREFIX: &str = "zcash-vote-delegate";
/// The marker version this crate writes and accepts.
pub const MARKER_VERSION: &str = "v1";

/// A post's text is not a valid marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerError {
    /// The text does not start with `zcash-vote-delegate`.
    NotAMarker,
    /// The second word is not `v1`.
    UnsupportedVersion,
    /// The version is not followed by exactly one word, the key.
    Malformed,
    /// The key is not the canonical `zvdk1` string of a valid delegate key.
    InvalidKey(DelegateKeyDecodeError),
}

impl fmt::Display for MarkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAMarker => write!(f, "text does not start with {MARKER_PREFIX}"),
            Self::UnsupportedVersion => write!(f, "marker version is not {MARKER_VERSION}"),
            Self::Malformed => f.write_str("marker is not the prefix, the version and one key"),
            Self::InvalidKey(error) => write!(f, "marker key: {error}"),
        }
    }
}

impl std::error::Error for MarkerError {}

/// A parsed registration marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegateMarker {
    /// The key being registered.
    pub dk: DelegateKey,
}

impl DelegateMarker {
    /// The marker's canonical text, the exact string a delegate posts.
    pub fn to_text(&self) -> String {
        format!("{MARKER_PREFIX} {MARKER_VERSION} {}", self.dk.encode())
    }

    /// Parses a post's whole text. Succeeds exactly when the normalized text
    /// equals [`Self::to_text`] of the result.
    pub fn parse(post_text: &str) -> Result<Self, MarkerError> {
        let normalized = normalize_marker_whitespace(post_text);
        let words: Vec<&str> = normalized.split(' ').collect();
        let key = match words.as_slice() {
            [prefix, ..] if *prefix != MARKER_PREFIX => return Err(MarkerError::NotAMarker),
            [_, version, ..] if *version != MARKER_VERSION => {
                return Err(MarkerError::UnsupportedVersion)
            }
            [_, _, key] => *key,
            _ => return Err(MarkerError::Malformed),
        };
        let dk = DelegateKey::decode(key).map_err(MarkerError::InvalidKey)?;
        Ok(Self { dk })
    }
}

/// Trims whitespace and collapses each inner run to one space. Whitespace is
/// Unicode `White_Space`, as Rust's `char::is_whitespace` and Go's
/// `unicode.IsSpace` both use.
pub fn normalize_marker_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
