//! The 0x09 proxy batch generation: what its digest binds, the request and
//! layout derivation rebuilds from durable state, and what derivation refuses.

use base64::Engine as _;

use super::{
    tests::{delegation_inputs, identity},
    *,
};
use crate::{
    chain_submission::tests::scripted_chain::sign_attempt,
    proxy::{
        batch::ProvedProxyDc,
        tests::submission_fixtures::{proved_batch, FixtureAnchor, NETWORK, ROUND_ID, WALLET_ID},
        DcShareLayout,
    },
    storage::VotingDb,
};

/// A synthetic two-DC batch with fixed bytes.
fn synthetic_batch(anchor_height: u32) -> ProvedProxyBatch {
    let dc = |slot: u8| ProvedProxyDc {
        dc_slot: slot,
        delegate_index: 10 + u32::from(slot),
        weight_ballots: 3 + u64::from(slot),
        van_nullifier: [0x30 + slot; 32],
        r_vpk: [0x40 + slot; 32],
        vote_authority_note_new: [0x50 + slot; 32],
        delegation_commitment: [0x60 + slot; 32],
        dc_slot_nullifier: [0x70 + slot; 32],
        alpha_v: [0x90 + slot; 32],
        proof: vec![0x80 + slot; 48],
    };
    ProvedProxyBatch {
        bundle_index: 7,
        anchor_height,
        layout: DcShareLayout::Standard,
        dcs: vec![dc(0), dc(1)],
    }
}

fn proxy_identity(batch_digest: [u8; 32]) -> ChainSubmissionIdentity {
    identity(ChainSubmissionTarget::ProxyDelegationBatch { batch_digest })
}

fn generation_of(
    batch: &ProvedProxyBatch,
    registration: Option<&DelegationGenerationInputs>,
) -> ChainSubmissionGenerationDigest {
    proxy_batch_generation(&proxy_identity([0x5d; 32]), batch, registration).digest()
}

#[test]
fn proxy_batch_generation_matches_its_frozen_vectors() {
    assert_eq!(
        (
            generation_of(&synthetic_batch(0), Some(&delegation_inputs())).to_hex(),
            generation_of(&synthetic_batch(41), None).to_hex(),
        ),
        (
            "094fdf9a0c5ac3e6ecef1a4aa834f4e80f3d8fc659b4f1b8286bb4f92ca06462".to_string(),
            "cdaf7c749b98a6382ed2799507141e4d0edfe7e13ab3c8c09ce9ea7cdbdf17db".to_string(),
        )
    );
}

#[test]
fn proxy_batch_generation_binds_every_dc_field_and_the_registration() {
    let original = generation_of(&synthetic_batch(0), Some(&delegation_inputs()));
    let mut changed_proof = synthetic_batch(0);
    changed_proof.dcs[1].proof[0] ^= 1;
    let mut reordered = synthetic_batch(0);
    reordered.dcs.swap(0, 1);
    let mut changed_weight = synthetic_batch(0);
    changed_weight.dcs[0].weight_ballots += 1;
    let mut changed_registration = delegation_inputs();
    changed_registration.gov_comm[0] ^= 1;

    for changed in [
        generation_of(&changed_proof, Some(&delegation_inputs())),
        generation_of(&reordered, Some(&delegation_inputs())),
        generation_of(&changed_weight, Some(&delegation_inputs())),
        generation_of(&synthetic_batch(0), Some(&changed_registration)),
        generation_of(&synthetic_batch(0), None),
    ] {
        assert_ne!(changed.as_bytes(), original.as_bytes());
    }
}

fn fixture_identity(batch_digest: [u8; 32]) -> ChainSubmissionIdentity {
    ChainSubmissionIdentity::new(
        WALLET_ID,
        NETWORK,
        hex::decode(ROUND_ID).unwrap().try_into().unwrap(),
        0,
        ChainSubmissionTarget::ProxyDelegationBatch { batch_digest },
    )
    .unwrap()
}

fn derive(db: &VotingDb, batch_digest: [u8; 32]) -> Result<DerivedChainSubmission, VotingError> {
    derive_proxy_batch(&db.conn(), &fixture_identity(batch_digest))
}

#[test]
fn derivation_rebuilds_the_signed_batch_its_layout_and_its_facts() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let digest = sign_attempt(&fixture, 1_000, false);

    let derived = derive(&fixture.db, digest).unwrap();

    let ChainSubmissionRequest::ProxyDelegationBatch(wire) = derived.request() else {
        panic!("a proxy batch derives a proxy batch request")
    };
    assert_eq!(wire.expiry_height, 1_500);
    assert_eq!(wire.vote_comm_tree_anchor_height, 0);
    assert_eq!(wire.dcs.len(), fixture.batch.dcs.len());
    let registration = wire.registration.as_ref().expect("the batch carries ZKP1");
    assert_eq!(
        registration.spend_auth_sig,
        base64::prelude::BASE64_STANDARD.encode(fixture.registration_signature)
    );
    let registration_van: [u8; 32] = fixture
        .db
        .conn()
        .query_row("SELECT gov_comm FROM bundles", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .unwrap()
        .try_into()
        .unwrap();
    let final_van = fixture.batch.dcs.last().unwrap().vote_authority_note_new;
    let mut leaves = vec![final_van];
    leaves.extend(fixture.batch.dcs.iter().map(|dc| dc.delegation_commitment));
    assert_eq!(derived.expected_layout().leaves(), leaves);
    assert_eq!(
        derived.expected_layout().split_off_leaf(),
        Some(registration_van)
    );
    let binding = derived.proxy().unwrap();
    assert!(binding.carries_registration());
    assert_eq!(
        binding.dc_slots,
        fixture
            .batch
            .dcs
            .iter()
            .map(|dc| dc.dc_slot)
            .collect::<Vec<_>>()
    );
}

#[test]
fn re_signing_keeps_the_generation_and_changes_only_the_request() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let digest = sign_attempt(&fixture, 1_000, false);
    let first = derive(&fixture.db, digest).unwrap();

    assert_eq!(sign_attempt(&fixture, 1_500, false), digest);
    let second = derive(&fixture.db, digest).unwrap();

    assert_eq!(
        first.generation().digest().as_bytes(),
        second.generation().digest().as_bytes()
    );
    assert_ne!(first.request(), second.request());
}

#[test]
fn a_chain_anchored_batch_binds_no_registration_and_probes_no_split_off() {
    let fixture = proved_batch(FixtureAnchor::Chain);
    let digest = sign_attempt(&fixture, 1_000, false);

    let derived = derive(&fixture.db, digest).unwrap();

    let ChainSubmissionRequest::ProxyDelegationBatch(wire) = derived.request() else {
        panic!("a proxy batch derives a proxy batch request")
    };
    assert!(wire.registration.is_none());
    assert_eq!(derived.expected_layout().split_off_leaf(), None);
    assert!(!derived.proxy().unwrap().carries_registration());
}

#[test]
fn derivation_refuses_another_batch_and_a_forged_signature() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let digest = sign_attempt(&fixture, 1_000, false);

    let mut other = digest;
    other[0] ^= 1;
    assert!(derive(&fixture.db, other).is_err());

    let forged: Vec<u8> = fixture
        .db
        .conn()
        .query_row(
            "SELECT dc_signatures FROM proxy_batch_attempts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut tampered = forged.clone();
    tampered[0] ^= 1;
    fixture
        .db
        .conn()
        .execute(
            "UPDATE proxy_batch_attempts SET dc_signatures = ?1",
            [tampered],
        )
        .unwrap();
    assert!(derive(&fixture.db, digest).is_err());
    fixture
        .db
        .conn()
        .execute(
            "UPDATE proxy_batch_attempts SET dc_signatures = ?1",
            [forged],
        )
        .unwrap();
    derive(&fixture.db, digest).unwrap();
}
