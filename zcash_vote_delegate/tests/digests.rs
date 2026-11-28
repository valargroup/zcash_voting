//! What each §4 digest binds, and the length prefix that refuses an
//! overlong chain id. The vectors pin every digest's value.

use zcash_vote_delegate::{
    ballot_digest, proxy_batch_digest, proxy_batch_sighash, register_digest, retire_digest,
    DelegateBallotFields, DelegateRegisterFields, DelegateRetireFields, DigestError,
    DigestPreimage, IdentityProvider, ProxyDelegationActionFields, ProxyDelegationBatchFields,
};

const CHAIN_ID: &str = "zvote-1";

/// A batch with a registration and two DCs whose fields are all distinct.
fn batch_with_registration() -> ProxyDelegationBatchFields {
    let dc = |seed: u8| ProxyDelegationActionFields {
        r_vpk: [seed; 32],
        van_nullifier: [seed + 1; 32],
        vote_authority_note_new: [seed + 2; 32],
        proxy_delegation_commitment: [seed + 3; 32],
        dc_slot_nullifier: [seed + 4; 32],
    };
    ProxyDelegationBatchFields {
        vote_round_id: [1; 32],
        vote_comm_tree_anchor_height: 0,
        expiry_height: 2_000_500,
        registration_van_cmx: Some([2; 32]),
        dcs: vec![dc(10), dc(20)],
    }
}

/// The registration of `alice` on X.
fn registration() -> DelegateRegisterFields<'static> {
    DelegateRegisterFields {
        dk: [4; 32],
        provider: IdentityProvider::X,
        handle: "alice",
        proof_id: "1790000000000000000",
    }
}

/// A chain id over 255 bytes cannot be length-prefixed, so every digest fails.
#[test]
fn every_digest_rejects_an_overlong_chain_id() {
    let chain_id = "z".repeat(256);
    let overflow = Err(DigestError::LengthPrefixOverflow { actual: 256 });
    assert_eq!(
        DigestPreimage::new("D", &chain_id).map(|preimage| preimage.finish()),
        overflow
    );
    assert_eq!(
        proxy_batch_sighash(&chain_id, &batch_with_registration()),
        overflow
    );
    assert_eq!(register_digest(&chain_id, &registration()), overflow);
    let retirement = DelegateRetireFields {
        delegate_index: 17,
        dk: [4; 32],
    };
    assert_eq!(retire_digest(&chain_id, &retirement), overflow);
    assert!(register_digest(&"z".repeat(255), &registration()).is_ok());
}

/// The batch identity ignores the expiry, so every re-signed attempt shares it,
/// while D1 itself changes with each new expiry.
#[test]
fn batch_digest_is_d1_without_the_expiry() {
    let batch = batch_with_registration();
    let resigned = ProxyDelegationBatchFields {
        expiry_height: batch.expiry_height + 500,
        ..batch.clone()
    };
    let unexpiring = ProxyDelegationBatchFields {
        expiry_height: 0,
        ..batch.clone()
    };
    assert_ne!(
        proxy_batch_sighash(CHAIN_ID, &batch),
        proxy_batch_sighash(CHAIN_ID, &resigned)
    );
    assert_eq!(
        proxy_batch_digest(CHAIN_ID, &batch),
        proxy_batch_digest(CHAIN_ID, &resigned)
    );
    assert_eq!(
        proxy_batch_digest(CHAIN_ID, &batch),
        proxy_batch_sighash(CHAIN_ID, &unexpiring)
    );
}

/// D1 binds whether a registration is present, even against an all-zero
/// `van_cmx`, and binds the order of the DCs.
#[test]
fn batch_sighash_binds_the_registration_flag_and_dc_order() {
    let without_registration = ProxyDelegationBatchFields {
        registration_van_cmx: None,
        ..batch_with_registration()
    };
    let zero_registration = ProxyDelegationBatchFields {
        registration_van_cmx: Some([0; 32]),
        ..batch_with_registration()
    };
    assert_ne!(
        proxy_batch_sighash(CHAIN_ID, &without_registration),
        proxy_batch_sighash(CHAIN_ID, &zero_registration)
    );
    let mut reordered = batch_with_registration();
    reordered.dcs.reverse();
    assert_ne!(
        proxy_batch_sighash(CHAIN_ID, &batch_with_registration()),
        proxy_batch_sighash(CHAIN_ID, &reordered)
    );
}

/// The length prefixes keep a handle and a proof id apart: moving a byte
/// from one to the other changes the digest.
#[test]
fn register_digest_separates_handle_from_proof_id() {
    let moved = DelegateRegisterFields {
        handle: "alice1",
        proof_id: "790000000000000000",
        ..registration()
    };
    assert_ne!(
        register_digest(CHAIN_ID, &registration()).unwrap(),
        register_digest(CHAIN_ID, &moved).unwrap()
    );
}

/// BALLOT binds the option count, so a ballot for a shorter round is never a
/// prefix-compatible preimage of a longer one.
#[test]
fn ballot_digest_binds_the_option_count() {
    let ballot = |options: Vec<u32>| DelegateBallotFields {
        vote_round_id: [8; 32],
        delegate_index: 17,
        options,
    };
    let digests = [
        ballot_digest(CHAIN_ID, &ballot(vec![])).unwrap(),
        ballot_digest(CHAIN_ID, &ballot(vec![0])).unwrap(),
        ballot_digest(CHAIN_ID, &ballot(vec![0, 0])).unwrap(),
        ballot_digest(CHAIN_ID, &ballot(vec![1, 0])).unwrap(),
    ];
    for (i, left) in digests.iter().enumerate() {
        for right in &digests[i + 1..] {
            assert_ne!(left, right);
        }
    }
}
