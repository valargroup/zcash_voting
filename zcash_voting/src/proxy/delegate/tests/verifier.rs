//! The attester API client: request bodies, attestations and the five errors
//! the attester answers (contract §12.4).

use serde_json::json;
use zcash_vote_delegate::IdentityProvider;

use super::fixtures::{alice_attestation, alice_key};
use crate::{
    proxy::delegate::{attest_request, parse_attest_response, VERIFIER_ATTEST_PATH},
    wire::{DelegateVerifierError, DelegateVerifierRefusal},
};

/// The contract's error table: each code, its HTTP status and its error.
const ERROR_TABLE: [(&str, u16, DelegateVerifierError); 5] = [
    ("bad_request", 400, DelegateVerifierError::BadRequest),
    ("post_not_found", 404, DelegateVerifierError::PostNotFound),
    (
        "marker_mismatch",
        409,
        DelegateVerifierError::MarkerMismatch,
    ),
    ("rate_limited", 429, DelegateVerifierError::RateLimited),
    (
        "provider_unavailable",
        503,
        DelegateVerifierError::ProviderUnavailable,
    ),
];

/// An error answer body.
fn error_body(code: &str, retry_after: Option<u64>) -> Vec<u8> {
    let mut body = json!({ "error": code, "message": format!("{code} explained") });
    if let Some(retry_after) = retry_after {
        body["retry_after"] = json!(retry_after);
    }
    body.to_string().into_bytes()
}

/// The 200 answer for alice's registration, as the attester writes it.
fn attest_answer() -> serde_json::Value {
    let attestation = alice_attestation();
    let signature = &attestation.attestations[0];
    json!({
        "provider": 1,
        "handle": attestation.handle,
        "proof_id": attestation.proof_id,
        "attestations": [{
            "verifier_id": signature.verifier_id,
            "not_before": signature.not_before,
            "expires_at": signature.expires_at,
            "sig": signature.sig,
        }],
    })
}

/// An attest request posts the provider code, the proof link and the
/// `zvdk1` key.
#[test]
fn requests_carry_provider_proof_and_key() {
    let dk = alice_key();
    let attest = attest_request(IdentityProvider::X, "https://x.com/alice/status/20", &dk);
    assert_eq!(VERIFIER_ATTEST_PATH, "/v1/attest");
    assert_eq!(
        String::from_utf8(attest.body).unwrap(),
        format!(
            r#"{{"provider":1,"proof_url":"https://x.com/alice/status/20","dk":"{}"}}"#,
            dk.encode()
        )
    );
}

/// A 200 answer becomes the attestation for the requested key, and fields
/// the contract does not define are ignored.
#[test]
fn attest_answers_become_attestations() {
    let mut answer = attest_answer();
    answer["added_later"] = json!({ "anything": [1, 2] });
    assert_eq!(
        parse_attest_response(&alice_key(), 200, answer.to_string().as_bytes()),
        Ok(alice_attestation())
    );
}

/// A 200 answer whose known fields break the contract's encodings, or whose
/// handle or proof id is not canonical for its provider, is an unknown
/// answer, not an attestation.
#[test]
fn malformed_attest_answers_are_unknown() {
    let mutations: [(&str, serde_json::Value); 5] = [
        ("provider", json!(3)),
        ("handle", json!("Alice")),
        ("proof_id", json!("0179")),
        ("attestations", json!([{ "verifier_id": 1 }])),
        (
            "attestations",
            json!([{ "verifier_id": 1, "not_before": 1, "expires_at": 2, "sig": "AAAA" }]),
        ),
    ];
    for (field, value) in mutations {
        let mut answer = attest_answer();
        answer[field] = value;
        let refusal =
            parse_attest_response(&alice_key(), 200, answer.to_string().as_bytes()).unwrap_err();
        assert_eq!(refusal.error, DelegateVerifierError::Unknown, "{field}");
    }
    let refusal = parse_attest_response(&alice_key(), 200, b"<html>").unwrap_err();
    assert_eq!(refusal.error, DelegateVerifierError::Unknown);
}

/// Each of the attester's five codes is read under the status the table
/// pairs it with, with its `retry_after` and message.
#[test]
fn every_attester_error_code_is_read() {
    for (code, status, error) in ERROR_TABLE {
        assert_eq!(
            parse_attest_response(&alice_key(), status, &error_body(code, Some(1_790_000_600))),
            Err(DelegateVerifierRefusal {
                error,
                retry_after: Some(1_790_000_600),
                detail: Some(format!("{code} explained")),
            }),
            "{code}"
        );
    }
}

/// A code under another status, a code the table lacks and an answer that is
/// not the API's JSON are all unknown.
#[test]
fn any_other_error_answer_is_unknown() {
    for (case, status, body) in [
        (
            "a code under the wrong status",
            500,
            error_body("post_not_found", None),
        ),
        (
            "a code the table lacks",
            422,
            error_body("account_too_young", None),
        ),
        ("no JSON body", 502, b"bad gateway".to_vec()),
    ] {
        let refusal = parse_attest_response(&alice_key(), status, &body).unwrap_err();
        assert_eq!(refusal.error, DelegateVerifierError::Unknown, "{case}");
        assert!(refusal.detail.is_some(), "{case}");
    }
}
