//! The attester API client (contract §12.4): `POST /v1/attest` bodies and
//! their answers.
//!
//! An error code counts only under the HTTP status the table pairs it with,
//! else it is [`DelegateVerifierError::Unknown`]. Unknown fields are ignored
//! and known ones checked strictly. Binding is checked separately by
//! [`check_attestation`](super::check_attestation).

use serde::{Deserialize, Serialize};
use zcash_vote_delegate::{
    is_canonical_handle, is_canonical_proof_id, DelegateKey, IdentityProvider,
};

use super::{delegate_op::bounded_server_text, device_key::delegate_key_info};
use crate::{
    proxy::registry::{encoding, provider_view},
    wire::{
        DelegateAttestation, DelegateAttestationSignature, DelegateVerifierError,
        DelegateVerifierRefusal,
    },
};

/// The attest route, under a verifier API base URL.
pub const VERIFIER_ATTEST_PATH: &str = "/v1/attest";

/// One attest request. The host posts `body` as `application/json` to a
/// `verifier_api_urls` entry followed by
/// [`VERIFIER_ATTEST_PATH`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifierRequest {
    /// The JSON body.
    pub body: Vec<u8>,
}

/// Asks the attester to attest `dk` from the marker at `proof_url`, an X
/// post or GitHub gist link.
pub fn attest_request(
    provider: IdentityProvider,
    proof_url: &str,
    dk: &DelegateKey,
) -> VerifierRequest {
    let body = serde_json::to_vec(&ProofRequestJson {
        provider: provider.code(),
        proof_url,
        dk: dk.encode(),
    })
    .expect("a verifier request always serializes");
    VerifierRequest { body }
}

/// Reads the attester's answer to an attest request for `dk` into the
/// [`DelegateAttestation`] the host passes to register.
///
/// # Errors
///
/// The [`DelegateVerifierRefusal`] the answer carries, or
/// [`DelegateVerifierError::Unknown`] for an unreadable answer.
pub fn parse_attest_response(
    dk: &DelegateKey,
    status: u16,
    body: &[u8],
) -> Result<DelegateAttestation, DelegateVerifierRefusal> {
    if status != 200 {
        return Err(refusal(status, body));
    }
    let answer: AttestAnswer = serde_json::from_slice(body)
        .map_err(|error| unknown(format!("the attest answer does not parse: {error}")))?;
    let malformed = |field: &str| unknown(format!("the attest answer's {field} is malformed"));
    let provider =
        IdentityProvider::from_code(answer.provider).ok_or_else(|| malformed("provider"))?;
    if !is_canonical_handle(provider, &answer.handle) {
        return Err(malformed("handle"));
    }
    if !is_canonical_proof_id(provider, &answer.proof_id) {
        return Err(malformed("proof_id"));
    }
    let attestations = answer
        .attestations
        .into_iter()
        .map(|attestation| {
            encoding::base64_array::<64>(&attestation.sig).ok_or_else(|| malformed("sig"))?;
            Ok(DelegateAttestationSignature {
                verifier_id: attestation.verifier_id,
                not_before: attestation.not_before,
                expires_at: attestation.expires_at,
                sig: attestation.sig,
            })
        })
        .collect::<Result<Vec<_>, DelegateVerifierRefusal>>()?;
    Ok(DelegateAttestation {
        key: delegate_key_info(dk),
        provider: provider_view(provider),
        handle: answer.handle,
        proof_id: answer.proof_id,
        attestations,
    })
}

/// The refusal a non-200 answer carries.
fn refusal(status: u16, body: &[u8]) -> DelegateVerifierRefusal {
    let Ok(answer) = serde_json::from_slice::<ErrorAnswer>(body) else {
        return unknown(format!("HTTP {status} without an attester error body"));
    };
    let detail = (!answer.message.trim().is_empty()).then(|| bounded_server_text(&answer.message));
    match error_code(&answer.error) {
        Some((error, paired_status)) if paired_status == status => DelegateVerifierRefusal {
            error,
            retry_after: answer.retry_after,
            detail,
        },
        _ => DelegateVerifierRefusal {
            error: DelegateVerifierError::Unknown,
            retry_after: answer.retry_after,
            detail: Some(bounded_server_text(&format!(
                "HTTP {status} with error {:?}: {}",
                answer.error, answer.message
            ))),
        },
    }
}

/// The table's error for a wire code, with the HTTP status it pairs with.
fn error_code(code: &str) -> Option<(DelegateVerifierError, u16)> {
    Some(match code {
        "bad_request" => (DelegateVerifierError::BadRequest, 400),
        "post_not_found" => (DelegateVerifierError::PostNotFound, 404),
        "marker_mismatch" => (DelegateVerifierError::MarkerMismatch, 409),
        "rate_limited" => (DelegateVerifierError::RateLimited, 429),
        "provider_unavailable" => (DelegateVerifierError::ProviderUnavailable, 503),
        _ => return None,
    })
}

/// A [`DelegateVerifierError::Unknown`] refusal explained by `detail`.
fn unknown(detail: String) -> DelegateVerifierRefusal {
    DelegateVerifierRefusal {
        error: DelegateVerifierError::Unknown,
        retry_after: None,
        detail: Some(bounded_server_text(&detail)),
    }
}

/// The attest request body.
#[derive(Serialize)]
struct ProofRequestJson<'a> {
    provider: u32,
    proof_url: &'a str,
    dk: String,
}

/// The attest route's 200 answer.
#[derive(Deserialize)]
struct AttestAnswer {
    provider: u32,
    handle: String,
    proof_id: String,
    attestations: Vec<AttestationAnswer>,
}

/// One attestation in the attest answer.
#[derive(Deserialize)]
struct AttestationAnswer {
    verifier_id: u32,
    not_before: u64,
    expires_at: u64,
    sig: String,
}

/// Every error answer.
#[derive(Deserialize)]
struct ErrorAnswer {
    error: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    retry_after: Option<u64>,
}
