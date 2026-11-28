//! Batch planning and building without proving: every slot's nullifier is the
//! circuit's, planning writes it and the randomizer once, and the same slots
//! always rebuild byte-identical DCs in the committed layout.

use voting_circuits::proxy_delegation::{dc_slot_nullifier_hash, ProxyDelegationBundle};
use voting_crypto_deps::CircuitValue as _;

use super::fixtures::{
    commit, db_with_hotkey_setup, host_at, hotkey_van, request, van_comm_rand, HOTKEY_SECRET,
    NETWORK, ROUND_ID, WALLET_ID,
};
use crate::wire::ProxyDelegateChoice;
use crate::{
    backend::{
        orchard::keys::FullViewingKey,
        pasta_curves::{group::ff::PrimeField, pallas},
    },
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    proxy::{
        batch::{build_dc_chain, load_chain_inputs, plan_batch_slots, BatchAnchor, BuiltDc},
        batch_layout, load_allocation, DcShareLayout,
    },
    storage::VotingDb,
    types::VotingError,
};

/// Three delegates over one bundle, keeping a remainder, so the batch has
/// three DCs whose weights no single share denomination covers.
fn three_delegates() -> Vec<ProxyDelegateChoice> {
    request(&[(17, 2_300), (5, 1_700), (9, 700)])
}

/// Plans bundle 0's batch under the fixture hotkey and rebuilds its DCs from
/// the stored plan, carrying the registration.
fn plan_and_build(db: &VotingDb) -> Vec<BuiltDc> {
    plan_batch_slots(db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap();
    let inputs = db
        .read_transaction("test", |tx| {
            load_chain_inputs(tx, ROUND_ID, WALLET_ID, 0, NETWORK)
        })
        .unwrap();
    build_dc_chain(&inputs, &BatchAnchor::Registration, &HOTKEY_SECRET).unwrap()
}

/// Every public input, the randomized key and every share of a built DC.
fn dc_bytes(bundle: &ProxyDelegationBundle) -> Vec<Vec<u8>> {
    let mut bytes: Vec<Vec<u8>> = bundle
        .instance
        .to_halo2_instance()
        .iter()
        .map(|field| field.to_repr().to_vec())
        .collect();
    bytes.push(bundle.r_vpk_bytes.to_vec());
    bytes.push(bundle.shares_hash.to_repr().to_vec());
    for share in &bundle.encrypted_shares {
        bytes.push(share.c1.to_vec());
        bytes.push(share.c2.to_vec());
    }
    bytes
}

/// Each stored slot nullifier is the circuit helper's
/// `Poseidon(nk, TAG + slot, round, van_comm_rand)`, and the proof's public
/// nullifier is the stored one.
#[test]
fn every_slot_nullifier_is_the_circuit_s() {
    let db = db_with_hotkey_setup(&[100]);
    commit(&db, &three_delegates(), DcShareLayout::Standard);

    let built = plan_and_build(&db);

    let spending_key =
        spending_key_from_hotkey_seed(&HOTKEY_SECRET, NETWORK, VOTING_HOTKEY_ACCOUNT_INDEX)
            .unwrap();
    let nk =
        voting_crypto_deps::nullifier_key(&FullViewingKey::from(&spending_key)).circuit_value();
    let round =
        pallas::Base::from_repr(hex::decode(ROUND_ID).unwrap().try_into().unwrap()).unwrap();
    let rand = pallas::Base::from_repr(van_comm_rand(0)).unwrap();
    let stored = load_allocation(&db, ROUND_ID).unwrap().unwrap();
    assert_eq!(stored.slots().len(), 3);
    let mut nullifiers = Vec::new();
    for slot in stored.slots() {
        let expected = dc_slot_nullifier_hash(nk, round, rand, slot.dc_slot).to_repr();
        assert_eq!(
            slot.dc_slot_nullifier,
            Some(expected),
            "slot {}",
            slot.dc_slot
        );
        nullifiers.push(expected);
    }
    nullifiers.sort_unstable();
    nullifiers.dedup();
    assert_eq!(nullifiers.len(), 3, "every slot has its own nullifier");
    for dc in &built {
        assert_eq!(
            dc.bundle.instance.dc_slot_nullifier.to_repr(),
            dc.slot.dc_slot_nullifier
        );
    }
}

/// Planning again neither redraws a randomizer nor rewrites a nullifier, and
/// a hotkey that cannot reproduce the stored nullifiers is refused.
#[test]
fn a_batch_is_planned_once() {
    let db = db_with_hotkey_setup(&[100]);
    commit(&db, &three_delegates(), DcShareLayout::Standard);

    let first = plan_batch_slots(&db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap();
    let again = plan_batch_slots(&db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap();
    assert_eq!(first, again);
    let mut randomizers: Vec<_> = first.iter().map(|slot| slot.alpha_v).collect();
    randomizers.sort_unstable();
    randomizers.dedup();
    assert_eq!(
        randomizers.len(),
        first.len(),
        "each DC has its own randomizer"
    );

    let other_hotkey = [0x22; 64];
    match plan_batch_slots(&db, ROUND_ID, 0, &other_hotkey, NETWORK).unwrap_err() {
        VotingError::InvalidInput { message } => {
            assert!(message.contains("do not reproduce"), "{message}")
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
    assert_eq!(
        plan_batch_slots(&db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap(),
        first
    );
}

/// The same slots rebuild byte-identical DCs, successor VANs, nullifiers,
/// keys and shares, and the chain ends at a VAN holding the kept weight.
#[test]
fn rebuilding_the_same_slots_yields_byte_identical_dcs() {
    let db = db_with_hotkey_setup(&[100]);
    let allocation = commit(&db, &three_delegates(), DcShareLayout::Standard);

    let first = plan_and_build(&db);
    let second = plan_and_build(&db);

    assert_eq!(first.len(), 3);
    for (one, other) in first.iter().zip(&second) {
        assert_eq!(one.slot, other.slot);
        assert_eq!(dc_bytes(&one.bundle), dc_bytes(&other.bundle));
    }
    // DCs follow slot order and each spends its predecessor's successor; the
    // last successor holds what the allocation kept on the bundle.
    let slots: Vec<u8> = first.iter().map(|dc| dc.slot.dc_slot).collect();
    assert_eq!(slots, vec![0, 1, 2]);
    let delegated: u64 = allocation
        .slots()
        .iter()
        .map(|slot| slot.weight_ballots)
        .sum();
    let last = first.last().unwrap();
    assert_eq!(
        last.bundle
            .instance
            .vote_authority_note_new
            .to_repr()
            .to_vec(),
        hotkey_van(100 - delegated, &van_comm_rand(0))
    );
}

/// A batch committed outside the last-moment window keeps the standard
/// layout when it is planned and rebuilt once the window has opened.
#[test]
fn the_committed_layout_survives_a_re_plan_in_the_last_moment_window() {
    let db = db_with_hotkey_setup(&[100]);
    // A 1,000-second round: committed before its window, rebuilt inside it.
    let early = host_at(1_100, Some(1_000), Some(2_000));
    let late = host_at(1_900, Some(1_000), Some(2_000));
    assert_eq!(batch_layout(&early), DcShareLayout::Standard);
    assert_eq!(batch_layout(&late), DcShareLayout::SingleShare);
    commit(&db, &three_delegates(), batch_layout(&early));

    // Planning reads the stored layout; the window it now runs in does not
    // enter planning at all.
    for dc in plan_and_build(&db) {
        assert_eq!(dc.slot.layout, DcShareLayout::Standard);
        let funded_shares = dc
            .bundle
            .encrypted_shares
            .iter()
            .filter(|share| share.plaintext_value != 0)
            .count();
        assert!(
            funded_shares > 1,
            "a standard DC splits its weight across shares"
        );
    }
}

/// A batch whose registration already landed spends the confirmed VAN through
/// its tree witness and binds the anchor height in every DC, while the DCs
/// and successor VANs are those of the registration-anchored batch: the
/// anchor enters the proofs, never the leaves.
#[test]
fn a_chain_anchored_batch_binds_its_anchor_and_keeps_its_dcs() {
    let db = db_with_hotkey_setup(&[100]);
    commit(&db, &three_delegates(), DcShareLayout::Standard);
    plan_batch_slots(&db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap();
    let inputs = db
        .read_transaction("test", |tx| {
            load_chain_inputs(tx, ROUND_ID, WALLET_ID, 0, NETWORK)
        })
        .unwrap();
    // The confirmed VAN as the only leaf of a tree synced at height 7.
    let witness = crate::vote::VanWitness {
        auth_path: crate::vote::single_leaf_auth_path(inputs.registration_van)
            .unwrap()
            .iter()
            .map(|sibling| sibling.to_vec())
            .collect(),
        position: 0,
        anchor_height: 7,
    };

    let chained = build_dc_chain(&inputs, &BatchAnchor::Chain(witness), &HOTKEY_SECRET).unwrap();
    let registered = build_dc_chain(&inputs, &BatchAnchor::Registration, &HOTKEY_SECRET).unwrap();

    for (chain, registration) in chained.iter().zip(&registered) {
        let (chain, registration) = (&chain.bundle.instance, &registration.bundle.instance);
        assert_eq!(chain.vote_comm_tree_anchor_height, pallas::Base::from(7u64));
        assert_eq!(
            registration.vote_comm_tree_anchor_height,
            pallas::Base::zero()
        );
        assert_eq!(
            chain.delegation_commitment,
            registration.delegation_commitment
        );
        assert_eq!(
            chain.vote_authority_note_new,
            registration.vote_authority_note_new
        );
        assert_eq!(chain.van_nullifier, registration.van_nullifier);
    }
}
