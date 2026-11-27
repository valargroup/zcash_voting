//! Strict text encodings shared by the registry and delegate-mode parsers.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

/// Exactly `N` bytes from canonical base64. See [`base64_bytes`].
pub(in crate::proxy) fn base64_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    base64_bytes(text)?.try_into().ok()
}

/// Bytes from padded standard base64 with zero padding bits, as Go's
/// `encoding/json` writes (contract §0). The engine rejects any other
/// encoding, so equal bytes have one accepted text.
pub(in crate::proxy) fn base64_bytes(text: &str) -> Option<Vec<u8>> {
    BASE64.decode(text).ok()
}

/// A `u64` from canonical decimal: ASCII digits, no sign and no leading zero.
pub(super) fn canonical_decimal(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}
