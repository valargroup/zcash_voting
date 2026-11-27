//! Canonical handles and proof ids (contracts-v1 §2.5).
//!
//! Lowercase is the only form on the wire and in state, so clients lowercase
//! what a user types and what a provider returns ([`normalize_handle`]) before
//! using it.

use crate::IdentityProvider;

/// Whether `handle` is the canonical handle of an account at `provider`: for
/// X, 1 to 15 bytes of `[a-z0-9_]`; for GitHub, 1 to 39 bytes of `[a-z0-9-]`
/// that neither start nor end with `-`.
pub fn is_canonical_handle(provider: IdentityProvider, handle: &str) -> bool {
    let (max_bytes, separator) = match provider {
        IdentityProvider::X => (15, b'_'),
        IdentityProvider::GitHub => (39, b'-'),
    };
    let bytes = handle.as_bytes();
    (1..=max_bytes).contains(&bytes.len())
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == separator)
        && (provider == IdentityProvider::X || !(handle.starts_with('-') || handle.ends_with('-')))
}

/// Whether `proof_id` is the canonical id of a proof at `provider`: for X, the
/// post id as 1 to 20 decimal digits not starting with `0`; for GitHub, the
/// gist id as 1 to 40 bytes of `[0-9a-f]`.
pub fn is_canonical_proof_id(provider: IdentityProvider, proof_id: &str) -> bool {
    let bytes = proof_id.as_bytes();
    match provider {
        IdentityProvider::X => {
            (1..=20).contains(&bytes.len())
                && bytes.iter().all(u8::is_ascii_digit)
                && bytes[0] != b'0'
        }
        IdentityProvider::GitHub => {
            (1..=40).contains(&bytes.len())
                && bytes
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        }
    }
}

/// `handle` trimmed, ASCII-lowercased and without one leading `@`. The result
/// is canonical only if [`is_canonical_handle`] says so.
pub fn normalize_handle(handle: &str) -> String {
    let lowercase = handle.trim().to_ascii_lowercase();
    match lowercase.strip_prefix('@') {
        Some(unprefixed) => unprefixed.to_owned(),
        None => lowercase,
    }
}
