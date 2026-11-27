//! The attestation check before REGISTER is signed: the answer binds the
//! delegate's key and provider to a canonical handle and proof id, within
//! current windows, under the chain's verifier set.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use zcash_vote_delegate::{AttestationWindow, IdentityProvider};

use super::fixtures::{
    alice_attestation, alice_key, attestation, bob_key, chain, vector_case, vector_window,
    verifiers, ATTEST_NOW, SECOND_VERIFIER_SECRET, VERIFIER_SECRET,
};
use crate::{
    proxy::delegate::{
        check_attestation, delegate_key_info, AttestationExpectation, AttestationMismatch,
        ProxyVerifierSet, MAX_ATTESTATIONS, MAX_ATTESTATION_CLOCK_SKEW_SECONDS,
        MAX_ATTESTATION_WINDOW_SECONDS,
    },
    wire::{DelegateAttestation, ProxyDelegateProvider},
};

/// What alice expects of her X registration, checked against `verifiers`.
fn expectation(verifiers: Option<&ProxyVerifierSet>) -> AttestationExpectation<'_> {
    AttestationExpectation {
        provider: IdentityProvider::X,
        verifiers,
    }
}

/// Checks `attestation` of alice's key at [`ATTEST_NOW`].
fn check(
    attestation: &DelegateAttestation,
    verifiers: Option<&ProxyVerifierSet>,
) -> Result<(), AttestationMismatch> {
    check_attestation(
        &chain(),
        attestation,
        &alice_key(),
        &expectation(verifiers),
        ATTEST_NOW,
    )
}

/// The fixture attestation carries the `attest/x_entry` vector's signature,
/// and passes under the chain's verifier set.
#[test]
fn the_vector_attestation_binds_what_the_delegate_posted() {
    let case = vector_case("attest", "x_entry");
    let attestation = alice_attestation();
    assert_eq!(
        hex::encode(BASE64.decode(&attestation.attestations[0].sig).unwrap()),
        case["expected"]["sig"].as_str().unwrap()
    );
    assert_eq!(check(&attestation, Some(&verifiers(1))), Ok(()));
    assert_eq!(check(&attestation, None), Ok(()));
}

/// The attestation must be for the key being registered and the provider the
/// marker was posted on, with a handle and proof id canonical for it.
#[test]
fn key_provider_handle_and_proof_id_are_bound() {
    let good = alice_attestation();
    let other_key = DelegateAttestation {
        key: delegate_key_info(&bob_key()),
        ..good.clone()
    };
    assert!(matches!(
        check(&other_key, None),
        Err(AttestationMismatch::WrongKey { .. })
    ));
    let mut wrong_fingerprint = good.clone();
    wrong_fingerprint.key.fingerprint = bob_key().fingerprint().to_string();
    assert!(matches!(
        check(&wrong_fingerprint, None),
        Err(AttestationMismatch::WrongKey { .. })
    ));
    let mut bad_key = good.clone();
    bad_key.key.dk = good.key.dk.to_uppercase();
    assert_eq!(
        check(&bad_key, None),
        Err(AttestationMismatch::Malformed { field: "key.dk" })
    );
    let github = DelegateAttestation {
        provider: ProxyDelegateProvider::GitHub,
        ..good.clone()
    };
    assert_eq!(
        check(&github, None),
        Err(AttestationMismatch::WrongProvider {
            expected: IdentityProvider::X,
            attested: IdentityProvider::GitHub,
        })
    );
    for handle in ["Alice", "alice-dev", ""] {
        let attestation = DelegateAttestation {
            handle: handle.to_owned(),
            ..good.clone()
        };
        assert_eq!(
            check(&attestation, None),
            Err(AttestationMismatch::Malformed { field: "handle" }),
            "{handle:?}"
        );
    }
    for proof_id in ["0179", "17a", ""] {
        let attestation = DelegateAttestation {
            proof_id: proof_id.to_owned(),
            ..good.clone()
        };
        assert_eq!(
            check(&attestation, None),
            Err(AttestationMismatch::Malformed { field: "proof_id" }),
            "{proof_id:?}"
        );
    }
}

/// A register op carries 1 to 16 attestations, each with a window of at most
/// 72 hours that is current on the device's clock.
#[test]
fn attestation_windows_must_be_current() {
    let window = vector_window();
    let with_window = |window| attestation(&alice_key(), &[(1, VERIFIER_SECRET)], window);
    let check_at = |attestation: &DelegateAttestation, now| {
        check_attestation(&chain(), attestation, &alice_key(), &expectation(None), now)
    };
    let good = with_window(window);
    assert_eq!(check_at(&good, window.not_before), Ok(()));
    assert_eq!(check_at(&good, window.expires_at - 1), Ok(()));
    assert_eq!(
        check_at(&good, window.expires_at),
        Err(AttestationMismatch::Expired {
            verifier_id: 1,
            expires_at: window.expires_at,
        })
    );
    let skew = MAX_ATTESTATION_CLOCK_SKEW_SECONDS;
    assert_eq!(check_at(&good, window.not_before - skew), Ok(()));
    assert_eq!(
        check_at(&good, window.not_before - skew - 1),
        Err(AttestationMismatch::NotYetValid {
            verifier_id: 1,
            not_before: window.not_before,
        })
    );

    let longest = AttestationWindow {
        expires_at: window.not_before + MAX_ATTESTATION_WINDOW_SECONDS,
        ..window
    };
    assert_eq!(check_at(&with_window(longest), ATTEST_NOW), Ok(()));
    for invalid in [
        AttestationWindow {
            expires_at: longest.expires_at + 1,
            ..window
        },
        AttestationWindow {
            expires_at: window.not_before,
            ..window
        },
    ] {
        assert!(matches!(
            check_at(&with_window(invalid), ATTEST_NOW),
            Err(AttestationMismatch::InvalidWindow { verifier_id: 1, .. })
        ));
    }

    let mut none = good.clone();
    none.attestations.clear();
    assert_eq!(
        check_at(&none, ATTEST_NOW),
        Err(AttestationMismatch::AttestationCount { count: 0 })
    );
    let mut too_many = good.clone();
    too_many.attestations = vec![good.attestations[0].clone(); MAX_ATTESTATIONS + 1];
    assert_eq!(
        check_at(&too_many, ATTEST_NOW),
        Err(AttestationMismatch::AttestationCount {
            count: MAX_ATTESTATIONS + 1
        })
    );
    let mut bad_sig = good;
    bad_sig.attestations[0].sig = "AAAA".to_owned();
    assert_eq!(
        check_at(&bad_sig, ATTEST_NOW),
        Err(AttestationMismatch::Malformed { field: "sig" })
    );
}

/// With the chain's verifier set, every signature must verify over ATTEST
/// for exactly the attested fields, and distinct verifiers must reach the
/// threshold.
#[test]
fn signatures_verify_under_the_chain_s_verifier_set() {
    let both = attestation(
        &alice_key(),
        &[(1, VERIFIER_SECRET), (2, SECOND_VERIFIER_SECRET)],
        vector_window(),
    );
    assert_eq!(check(&both, Some(&verifiers(2))), Ok(()));

    let one = alice_attestation();
    assert_eq!(
        check(&one, Some(&verifiers(2))),
        Err(AttestationMismatch::BelowThreshold {
            distinct: 1,
            threshold: 2,
        })
    );
    let mut repeated = one.clone();
    repeated.attestations.push(one.attestations[0].clone());
    assert_eq!(
        check(&repeated, Some(&verifiers(2))),
        Err(AttestationMismatch::BelowThreshold {
            distinct: 1,
            threshold: 2,
        })
    );

    let closed = ProxyVerifierSet {
        verifiers: Vec::new(),
        threshold: 0,
    };
    assert_eq!(
        check(&one, Some(&closed)),
        Err(AttestationMismatch::RegistrationClosed)
    );
    let unknown = attestation(&alice_key(), &[(9, VERIFIER_SECRET)], vector_window());
    assert_eq!(
        check(&unknown, Some(&verifiers(1))),
        Err(AttestationMismatch::UnknownVerifier { verifier_id: 9 })
    );
    let wrong_signer = attestation(
        &alice_key(),
        &[(1, SECOND_VERIFIER_SECRET)],
        vector_window(),
    );
    assert_eq!(
        check(&wrong_signer, Some(&verifiers(1))),
        Err(AttestationMismatch::BadSignature { verifier_id: 1 })
    );

    // A signature covers the handle, the proof id and the window.
    let mut other_handle = one.clone();
    other_handle.handle = "mallory".to_owned();
    let mut other_proof = one.clone();
    other_proof.proof_id = "1790000000000000001".to_owned();
    let mut other_window = one;
    other_window.attestations[0].expires_at += 1;
    for tampered in [other_handle, other_proof, other_window] {
        assert_eq!(
            check(&tampered, Some(&verifiers(1))),
            Err(AttestationMismatch::BadSignature { verifier_id: 1 })
        );
    }
}
