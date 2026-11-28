//! Registering the device's key and retiring its entry: the ops sign the
//! vectors' digests in canonical `MsgDelegateOp` JSON, a prepared register
//! op is resent unchanged until a proven entry settles it, and retire is one
//! op by the entry's key.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use zcash_vote_delegate::{verify_zip215, DelegateStatus, IdentityProvider};

use super::fixtures::{
    alice_attestation, alice_entry, alice_key, attestation, bob_key, broadcast_result, chain, db,
    entry, proven, proven_entry, signing_key, vector_bytes, vector_case, vector_window, verifiers,
    ALICE_PROOF_ID, ALICE_SECRET, ATTEST_NOW, BOB_SECRET, DELEGATE_INDEX, VERIFIER_SECRET,
};
use crate::{
    proxy::delegate::{
        build_retire_op, classify_register_answer, classify_retire_answer, device_delegate_key,
        forget_delegate_key, parse_delegate_op_answer, pending_register_op, prepare_register_op,
        registration_result, retire_result, AttestationExpectation, DelegateChain,
        DelegateModeError, DelegateOpKind, ProxyVerifierSet, RegisterAnswer, RetireAnswer,
        SignedDelegateOp,
    },
    storage::VotingDb,
    wire::{DelegateRegisterResult, DelegateRetireResult},
};

/// What alice expects of her X registration, checked against the chain's
/// verifier set.
fn expectation(verifiers: &ProxyVerifierSet) -> AttestationExpectation<'_> {
    AttestationExpectation {
        provider: IdentityProvider::X,
        verifiers: Some(verifiers),
    }
}

/// Prepares alice's register op on `chain` from her vector attestation.
fn prepare(db: &VotingDb, chain: &DelegateChain) -> Result<SignedDelegateOp, DelegateModeError> {
    prepare_register_op(
        db,
        chain,
        &alice_attestation(),
        &expectation(&verifiers(1)),
        &signing_key(ALICE_SECRET),
        ATTEST_NOW,
    )
}

/// The node's answer with `code` in codespace `vote`, classified.
fn register_answer(code: u32) -> RegisterAnswer {
    let (status, codespace) = if code == 0 { (200, "") } else { (422, "vote") };
    classify_register_answer(
        &parse_delegate_op_answer(status, &broadcast_result(code, codespace, "log")).unwrap(),
    )
}

/// The register op carries the `register/x_entry` vector's key and its
/// signature over the vector's REGISTER digest, with the attester's handle,
/// proof id and attestation, as `MsgDelegateOp` JSON in proto field order.
#[test]
fn the_register_op_signs_the_vector_digest_in_canonical_json() {
    let case = vector_case("register", "x_entry");
    let expected = &case["expected"];
    let op = prepare(&db(), &chain()).unwrap();
    assert_eq!(op.kind(), DelegateOpKind::Register);
    assert_eq!(op.delegate_index(), 0);
    assert_eq!(op.dk(), alice_key());

    let attested = alice_attestation();
    let signature = &attested.attestations[0];
    let dk: [u8; 32] = vector_bytes(&expected["dk"]);
    let dk_sig: [u8; 64] = vector_bytes(&expected["dk_sig"]);
    assert_eq!(
        String::from_utf8(op.body().to_vec()).unwrap(),
        format!(
            r#"{{"register":{{"dk":"{}","provider":1,"handle":"alice","proof_id":"{ALICE_PROOF_ID}","attestations":[{{"verifier_id":1,"not_before":{},"expires_at":{},"sig":"{}"}}],"dk_sig":"{}"}}}}"#,
            BASE64.encode(dk),
            signature.not_before,
            signature.expires_at,
            signature.sig,
            BASE64.encode(dk_sig),
        )
    );
    let digest: [u8; 32] = vector_bytes(&expected["digest"]);
    assert_eq!(verify_zip215(&dk, &digest, &dk_sig), Ok(()));
}

/// Nothing is signed or recorded unless the attestation passes its check
/// and is for the signing key.
#[test]
fn register_signs_only_a_checked_attestation_of_its_key() {
    let db = db();
    let chain = chain();
    let bob_signs = prepare_register_op(
        &db,
        &chain,
        &alice_attestation(),
        &expectation(&verifiers(1)),
        &signing_key(BOB_SECRET),
        ATTEST_NOW,
    );
    assert!(matches!(bob_signs, Err(DelegateModeError::Attestation(_))));
    assert_eq!(device_delegate_key(&db, &chain).unwrap(), None);
    assert_eq!(pending_register_op(&db, &chain).unwrap(), None);
}

/// Preparing records the key as the device's and stores the op, which every
/// retry resends byte for byte. A fresh attestation replaces it. A proven
/// entry holding the key settles the registration and clears the op.
#[test]
fn a_prepared_register_op_is_resent_unchanged_until_the_entry_settles_it() {
    let db = db();
    let chain = chain();
    assert_eq!(pending_register_op(&db, &chain).unwrap(), None);
    let op = prepare(&db, &chain).unwrap();
    assert_eq!(
        device_delegate_key(&db, &chain).unwrap().map(|key| key.dk),
        Some(alice_key())
    );
    assert_eq!(pending_register_op(&db, &chain).unwrap(), Some(op.clone()));
    assert_eq!(prepare(&db, &chain).unwrap(), op, "ops are deterministic");

    let mut later = vector_window();
    later.expires_at += 60;
    let fresh = prepare_register_op(
        &db,
        &chain,
        &attestation(&alice_key(), &[(1, VERIFIER_SECRET)], later),
        &expectation(&verifiers(1)),
        &signing_key(ALICE_SECRET),
        ATTEST_NOW,
    )
    .unwrap();
    assert_ne!(fresh, op);
    assert_eq!(pending_register_op(&db, &chain).unwrap(), Some(fresh));

    assert_eq!(
        registration_result(&db, &chain, &alice_entry()).unwrap(),
        DelegateRegisterResult::Registered {
            delegate_index: DELEGATE_INDEX
        }
    );
    assert_eq!(pending_register_op(&db, &chain).unwrap(), None);
    assert!(matches!(
        prepare(&db, &chain),
        Err(DelegateModeError::KeyRegistered {
            delegate_index: DELEGATE_INDEX
        })
    ));
}

/// Code 0 and code 61 both mean the key may be registered, which only the
/// proven entry settles. Every other code's classification is in
/// `chain_codes.rs`.
#[test]
fn register_answers_settle_from_the_proven_entry() {
    assert!(matches!(
        register_answer(0),
        RegisterAnswer::Submitted { tx_hash } if tx_hash == "AB".repeat(32)
    ));
    assert_eq!(register_answer(61), RegisterAnswer::AlreadyRegistered);
}

/// The retire op carries the `retire/entry_17` vector's signature over
/// RETIRE by the entry's key, and only that key can build it.
#[test]
fn the_retire_op_signs_the_vector_digest_with_the_entry_s_key() {
    let case = vector_case("retire", "entry_17");
    let expected = &case["expected"];
    let op = build_retire_op(&chain(), &alice_entry(), &signing_key(ALICE_SECRET)).unwrap();
    assert_eq!(op.kind(), DelegateOpKind::Retire);
    assert_eq!(op.delegate_index(), DELEGATE_INDEX);
    assert_eq!(op.dk(), alice_key());
    let dk_sig: [u8; 64] = vector_bytes(&expected["dk_sig"]);
    assert_eq!(
        String::from_utf8(op.body().to_vec()).unwrap(),
        format!(
            r#"{{"retire":{{"delegate_index":17,"dk_sig":"{}"}}}}"#,
            BASE64.encode(dk_sig)
        )
    );
    assert_eq!(
        verify_zip215(
            &vector_bytes(&expected["dk"]),
            &vector_bytes::<32>(&expected["digest"]),
            &dk_sig
        ),
        Ok(())
    );

    assert!(matches!(
        build_retire_op(&chain(), &alice_entry(), &signing_key(BOB_SECRET)),
        Err(DelegateModeError::NotDeviceEntry {
            delegate_index: DELEGATE_INDEX
        })
    ));
}

/// Code 55, already retired, is a success. An accepted op settles once the
/// proven entry shows it retired. Every other code's classification is in
/// `chain_codes.rs`.
#[test]
fn retire_answers_settle_retire() {
    let retire_answer = |code: u32| {
        let (status, codespace) = if code == 0 { (200, "") } else { (422, "vote") };
        classify_retire_answer(
            &parse_delegate_op_answer(status, &broadcast_result(code, codespace, "log")).unwrap(),
        )
    };
    assert!(matches!(retire_answer(0), RetireAnswer::Submitted { .. }));
    assert_eq!(
        retire_answer(55),
        RetireAnswer::Settled(DelegateRetireResult::Retired)
    );

    assert_eq!(retire_result(&alice_entry()), None);
    assert_eq!(
        retire_result(&proven_entry(alice_key(), DelegateStatus::Suspended)),
        None
    );
    assert_eq!(
        retire_result(&proven_entry(alice_key(), DelegateStatus::Retired)),
        Some(DelegateRetireResult::Retired)
    );
}

/// To change its key a delegate retires, forgets the key and registers a
/// new one, which gets a new entry.
#[test]
fn a_key_changes_by_retiring_and_registering_again() {
    let db = db();
    let chain = chain();
    prepare(&db, &chain).unwrap();
    registration_result(&db, &chain, &alice_entry()).unwrap();
    build_retire_op(&chain, &alice_entry(), &signing_key(ALICE_SECRET)).unwrap();

    forget_delegate_key(&db, &chain).unwrap();
    let new_key = signing_key(BOB_SECRET);
    let op = prepare_register_op(
        &db,
        &chain,
        &attestation(&bob_key(), &[(1, VERIFIER_SECRET)], vector_window()),
        &expectation(&verifiers(1)),
        &new_key,
        ATTEST_NOW,
    )
    .unwrap();
    assert_eq!(op.dk(), bob_key());
    let new_entry = proven(&entry(
        DELEGATE_INDEX + 1,
        bob_key(),
        DelegateStatus::Active,
    ));
    assert_eq!(
        registration_result(&db, &chain, &new_entry).unwrap(),
        DelegateRegisterResult::Registered {
            delegate_index: DELEGATE_INDEX + 1
        }
    );
}
