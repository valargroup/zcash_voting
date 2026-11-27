//! A committed allocation freezes the delegation setup of every bundle it
//! places DCs on: each slot's nullifier and the VAN its DC spends derive from
//! that setup. Every path that clears, replaces or prunes setup refuses such a
//! bundle with its own error, and a proved batch counts as broadcast evidence.

use super::fixtures::{commit, db_with_bundle_weights, request, ROUND_ID, WALLET_ID};
use crate::{
    proxy::DcShareLayout,
    storage::{queries, VotingDb},
    types::VotingError,
};

/// Commits one delegate's allocation; packing places its only DC on the
/// largest bundle, bundle 0, and leaves bundle 1 kept.
fn commit_on_bundle_zero(db: &VotingDb) {
    let allocation = commit(db, &request(&[(17, 1_000)]), DcShareLayout::Standard);
    assert!(
        allocation.slots().iter().all(|slot| slot.bundle_index == 0),
        "the fixture expects every slot on bundle 0"
    );
}

/// Bundle `bundle_index`'s stored `van_comm_rand` and note total.
fn setup_of(db: &VotingDb, bundle_index: u32) -> (Option<Vec<u8>>, Option<i64>) {
    db.conn()
        .query_row(
            "SELECT van_comm_rand, total_note_value FROM bundles
              WHERE round_id = ?1 AND wallet_id = ?2 AND bundle_index = ?3",
            rusqlite::params![ROUND_ID, WALLET_ID, bundle_index],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

/// Gives every bundle a stored `van_comm_rand`, as delegation setup does.
fn store_van_comm_rand(db: &VotingDb) {
    db.conn()
        .execute(
            "UPDATE bundles SET van_comm_rand = ?1 WHERE round_id = ?2",
            rusqlite::params![vec![0x11_u8; 32], ROUND_ID],
        )
        .unwrap();
}

/// Asserts `error` is the frozen-setup refusal for `bundle_index` and `action`.
fn assert_frozen(error: VotingError, bundle_index: u32, action: &str) {
    match error {
        VotingError::DelegationSetupFrozen {
            bundle_index: refused,
            action: refused_action,
        } => {
            assert_eq!(refused, bundle_index);
            assert_eq!(refused_action, action);
        }
        other => panic!("expected DelegationSetupFrozen, got {other:?}"),
    }
}

/// A session reset that would clear a bundle with slots refuses and clears
/// nothing, not even the kept bundle beside it.
#[test]
fn a_session_reset_refuses_a_bundle_with_committed_slots() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);
    commit_on_bundle_zero(&db);
    let before = (setup_of(&db, 0), setup_of(&db, 1));

    let error = db
        .clear_unsigned_delegation_setup_fields(ROUND_ID)
        .unwrap_err();

    assert_frozen(error, 0, "clear unsigned delegation setup");
    assert_eq!((setup_of(&db, 0), setup_of(&db, 1)), before);
}

/// Without an allocation the reset clears unsigned setup as before.
#[test]
fn a_session_reset_without_an_allocation_still_clears_unsigned_setup() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);

    db.clear_unsigned_delegation_setup_fields(ROUND_ID).unwrap();

    assert_eq!(setup_of(&db, 0), (None, None));
    assert_eq!(setup_of(&db, 1), (None, None));
}

/// A frozen bundle that the reset already preserves, because its ZKP1 proof
/// is stored, does not refuse the reset of its kept sibling.
#[test]
fn a_session_reset_clears_a_kept_sibling_of_a_proved_frozen_bundle() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);
    commit_on_bundle_zero(&db);
    queries::store_proof(&db.conn(), ROUND_ID, WALLET_ID, 0, &[0x01; 8], None).unwrap();
    let frozen = setup_of(&db, 0);

    db.clear_unsigned_delegation_setup_fields(ROUND_ID).unwrap();

    assert_eq!(setup_of(&db, 0), frozen);
    assert_eq!(setup_of(&db, 1), (None, None));
}

/// Discarding unbroadcast setup refuses a frozen bundle, whether the call
/// names it or covers the round.
#[test]
fn discarding_unbroadcast_setup_refuses_a_frozen_bundle() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);
    commit_on_bundle_zero(&db);
    let before = setup_of(&db, 0);

    for scope in [Some(0), None] {
        let error = db
            .discard_unbroadcast_delegation(ROUND_ID, scope)
            .unwrap_err();
        assert_frozen(error, 0, "discard delegation setup");
    }
    assert_eq!(setup_of(&db, 0), before);
    // The kept bundle is not frozen and still discards on its own.
    assert_eq!(
        db.discard_unbroadcast_delegation(ROUND_ID, Some(1))
            .unwrap(),
        1
    );
}

/// Rebuilding a frozen bundle's setup for another target is refused before
/// anything is replaced.
#[test]
fn replacing_a_frozen_bundle_s_setup_is_refused() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);
    commit_on_bundle_zero(&db);
    let before = setup_of(&db, 0);

    let error = queries::replace_unbroadcast_delegation_setup(
        &db.conn(),
        ROUND_ID,
        WALLET_ID,
        0,
        None,
        &[0x99; 32],
        &[vec![0x22; 32]],
        &[0x33; 32],
        &[vec![0x44; 32]],
        &[0x55; 32],
        &[0x66; 32],
        &[0x77; 32],
        &[0x88; 32],
        &[0x99; 32],
        &[0xAA; 32],
        1_000,
        0,
        &[(vec![0xBB; 32], vec![0xCC; 32])],
        &[0xDD; 32],
        &crate::tx1::placeholder_tx1_effects(),
        &[],
        &[0xEE; 32],
        &[vec![0xFF; 32]],
    )
    .unwrap_err();

    assert_frozen(error, 0, "replace delegation setup");
    assert_eq!(setup_of(&db, 0), before);
}

/// Pruning bundles refuses a range holding a frozen bundle and prunes a range
/// without one.
#[test]
fn pruning_refuses_a_frozen_bundle() {
    let db = db_with_bundle_weights(&[60, 40]);
    commit_on_bundle_zero(&db);

    let error = db.delete_skipped_bundles(ROUND_ID, 0).unwrap_err();
    assert_frozen(error, 0, "prune bundles");
    assert_eq!(db.get_bundle_count(ROUND_ID).unwrap(), 2);

    assert_eq!(db.delete_skipped_bundles(ROUND_ID, 1).unwrap(), 1);
}

/// A proved proxy batch is broadcast evidence: round deletion refuses it and
/// names it, and the discard reports it ahead of the freeze.
#[test]
fn a_proved_batch_is_evidence_that_keeps_the_round() {
    let db = db_with_bundle_weights(&[60, 40]);
    store_van_comm_rand(&db);
    commit_on_bundle_zero(&db);
    db.conn()
        .execute(
            "INSERT INTO proxy_batches (round_id, wallet_id, bundle_index, anchor_height, proved_at)
             VALUES (?1, ?2, 0, 0, 1)",
            rusqlite::params![ROUND_ID, WALLET_ID],
        )
        .unwrap();

    assert_eq!(
        db.delegation_broadcast_evidence(ROUND_ID, None).unwrap(),
        Some((0, queries::DelegationBroadcastEvidence::ProxyBatch))
    );
    match db.clear_round(ROUND_ID).unwrap_err() {
        VotingError::DelegationAlreadyBroadcast {
            bundle_index,
            evidence,
        } => {
            assert_eq!(bundle_index, 0);
            assert!(
                evidence.contains("a proved proxy delegation batch"),
                "{evidence}"
            );
        }
        other => panic!("expected DelegationAlreadyBroadcast, got {other:?}"),
    }
    assert!(db.has_round(ROUND_ID).unwrap());
    assert!(matches!(
        db.discard_unbroadcast_delegation(ROUND_ID, Some(0)),
        Err(VotingError::DelegationAlreadyBroadcast { .. })
    ));
}
