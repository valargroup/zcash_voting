//! The attestation check before REGISTER is signed: the answer binds the
//! delegate's key and provider to a canonical handle and proof id, within
//! current windows, under the chain's verifier set.

use zcash_vote_delegate::{AttestationWindow, IdentityProvider};

use super::fixtures::{
    alice_attestation, alice_key, attestation, bob_key, chain, vector_window, verifiers,
    ATTEST_NOW, SECOND_VERIFIER_SECRET, VERIFIER_SECRET,
};
use crate::{
    proxy::delegate::{
        check_attestation, delegate_key_info, AttestationExpectation, AttestationMismatch,
        ProxyVerifierSet, MAX_ATTESTATION_CLOCK_SKEW_SECONDS, MAX_ATTESTATION_WINDOW_SECONDS,
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
    let upper_handle = DelegateAttestation {
        handle: "Alice".to_owned(),
        ..good.clone()
    };
    assert_eq!(
        check(&upper_handle, None),
        Err(AttestationMismatch::Malformed { field: "handle" })
    );
    let padded_proof_id = DelegateAttestation {
        proof_id: "0179".to_owned(),
        ..good
    };
    assert_eq!(
        check(&padded_proof_id, None),
        Err(AttestationMismatch::Malformed { field: "proof_id" })
    );
}

/// A register op carries at least one attestation, each with a window of
/// at most 72 hours that is current on the device's clock.
#[test]
fn attestation_windows_must_be_current() {
    let window = vector_window();
    let with_window = |window| attestation(&alice_key(), &[(1, VERIFIER_SECRET)], window);
    let check_at = |attestation: &DelegateAttestation, now| {
        check_attestation(&chain(), attestation, &alice_key(), &expectation(None), now)
    };
    let good = with_window(window);
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

    let too_long = AttestationWindow {
        expires_at: window.not_before + MAX_ATTESTATION_WINDOW_SECONDS + 1,
        ..window
    };
    assert!(matches!(
        check_at(&with_window(too_long), ATTEST_NOW),
        Err(AttestationMismatch::InvalidWindow { verifier_id: 1, .. })
    ));

    let mut none = good.clone();
    none.attestations.clear();
    assert_eq!(
        check_at(&none, ATTEST_NOW),
        Err(AttestationMismatch::AttestationCount { count: 0 })
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

    // A signature covers the attested fields.
    let mut other_handle = one;
    other_handle.handle = "mallory".to_owned();
    assert_eq!(
        check(&other_handle, Some(&verifiers(1))),
        Err(AttestationMismatch::BadSignature { verifier_id: 1 })
    );
}
