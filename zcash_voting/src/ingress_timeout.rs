//! Versioned, attempt-bound non-dispatch evidence from trusted REST ingress.

use rand::RngCore;
use serde::Deserialize;

pub(crate) const REQUEST_HEADER: &str = "x-vote-ingress-attempt-v1";

/// Fresh per-POST token, unrelated to transaction identity or diagnostic IDs.
/// If entropy is unavailable the request retains conservative error handling.
pub(crate) fn attempt_token() -> Option<String> {
    let mut token = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut token).ok()?;
    Some(hex::encode(token))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IngressFailure {
    error: TimeoutReceipt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeoutReceipt {
    version: u32,
    code: String,
    dispatch: String,
    attempt: String,
}

/// Accepts only a complete v1 receipt for this attempt. Callers must first
/// validate their response size limit and JSON content type. This trusts configured
/// HTTPS ingress to honor the non-broadcast contract; the token prevents stale
/// response reuse, not forgery by that trusted ingress.
pub(crate) fn is_not_dispatched(status: u16, payload: &[u8], attempt: Option<&str>) -> bool {
    let Some(attempt) = attempt else { return false };
    if status != 408 {
        return false;
    }
    let Ok(receipt) = serde_json::from_slice::<IngressFailure>(payload) else {
        return false;
    };
    receipt.error.version == 1
        && receipt.error.code == "request_body_timeout"
        && receipt.error.dispatch == "not_started"
        && receipt.error.attempt == attempt
}
