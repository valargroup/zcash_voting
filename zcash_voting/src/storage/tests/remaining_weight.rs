//! The remaining-weight loader: a bundle's current VAN holds
//! `floor(total_note_value / 12,500,000)` less every dispatched or confirmed
//! proxy slot's ballots, and every reader of VAN weight goes through it.

use super::*;
use crate::governance::BALLOT_DIVISOR;

const ROUND: &str = "test-round-1";

/// A round whose bundle 0 holds 10 ballots and a sub-ballot remainder, with a
/// committed allocation placing DCs of 3 and 2 ballots on it.
fn db_with_slots() -> VotingDb {
    let db = test_db();
    db.set_wallet_id(W);
    let conn = db.conn();
    queries::insert_round(&conn, W, Network::Testnet, &test_params(), None).unwrap();
    queries::insert_bundle(&conn, ROUND, W, 0, &[]).unwrap();
    conn.execute(
        "UPDATE bundles SET total_note_value = ?1, van_comm_rand = ?2, gov_comm = ?3,
                address_index = 0
          WHERE round_id = ?4 AND wallet_id = ?5 AND bundle_index = 0",
        rusqlite::params![
            (10 * BALLOT_DIVISOR + BALLOT_DIVISOR / 2) as i64,
            vec![0x11_u8; 32],
            vec![0u8; 32],
            ROUND,
            W
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO proxy_allocations
             (round_id, wallet_id, choices_json, committed_at)
         VALUES (?1, ?2, '[{}]', 1)",
        rusqlite::params![ROUND, W],
    )
    .unwrap();
    for (dc_slot, delegate_index, weight_ballots) in [(0, 17, 3), (1, 5, 2)] {
        conn.execute(
            "INSERT INTO proxy_slots
                 (round_id, wallet_id, bundle_index, dc_slot, delegate_index, weight_ballots,
                  layout)
             VALUES (?1, ?2, 0, ?3, ?4, ?5, 0)",
            rusqlite::params![ROUND, W, dc_slot, delegate_index, weight_ballots],
        )
        .unwrap();
    }
    drop(conn);
    db
}

/// Runs `sql` with CHECK enforcement lifted.
///
/// Only the lifecycle row's kind and state matter to the loader, so the rows
/// below stand in for that lifecycle without the candidate hashes, tracking
/// times and positions the state CHECKs require of a real row.
fn without_checks(db: &VotingDb, sql: &str, params: impl rusqlite::Params) {
    let conn = db.conn();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(sql, params).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .unwrap();
}

/// Records the bundle's 0x09 batch in the lifecycle in `state`.
fn record_proxy_batch_submission(db: &VotingDb, state: &str) {
    without_checks(
        db,
        "INSERT INTO chain_submissions
             (identity_key, round_id, wallet_id, network, bundle_index, kind,
              ordered_batch_digest, generation_digest, state, committed_post_reservations,
              created_at, updated_at)
         VALUES (zeroblob(32), ?1, ?2, 'testnet', 0, ?3, zeroblob(32), zeroblob(32), ?4, 1, 1, 1)",
        rusqlite::params![ROUND, W, queries::PROXY_BATCH_SUBMISSION_KIND, state],
    );
}

/// Moves the recorded batch submission to `state`.
fn set_proxy_batch_state(db: &VotingDb, state: &str) {
    without_checks(
        db,
        "UPDATE chain_submissions SET state = ?1 WHERE kind = ?2",
        rusqlite::params![state, queries::PROXY_BATCH_SUBMISSION_KIND],
    );
}

/// Committed but undispatched slots take nothing: the VAN still holds the
/// registered ballots, and ZKP2 and the original note total both read as
/// stored.
#[test]
fn undispatched_slots_leave_the_registered_weight() {
    let db = db_with_slots();
    let conn = db.conn();

    let state = queries::load_van_state(&conn, ROUND, W, 0).unwrap();
    assert_eq!(state.registered_ballots().unwrap(), 10);
    assert_eq!(state.delegated_ballots, 0);
    assert_eq!(state.remaining_ballots().unwrap(), 10);
    assert!(!state.registration_van_spent);

    let zkp2 = queries::load_zkp2_inputs(&conn, ROUND, W, 0).unwrap();
    assert_eq!(zkp2.van_ballots, 10);
    assert_eq!(
        zkp2.total_note_value,
        10 * BALLOT_DIVISOR + BALLOT_DIVISOR / 2
    );
}

/// Once the batch may be on chain, its slots' ballots leave the VAN; a
/// definite rejection spent nothing and gives them back.
#[test]
fn dispatched_or_confirmed_slots_leave_the_van() {
    let db = db_with_slots();
    record_proxy_batch_submission(&db, "submitting");

    for (state, remaining) in [
        ("submitting", 5),
        ("recovering", 5),
        ("confirmed", 5),
        ("rejected", 10),
    ] {
        set_proxy_batch_state(&db, state);
        let conn = db.conn();
        let van = queries::load_van_state(&conn, ROUND, W, 0).unwrap();
        assert_eq!(van.remaining_ballots().unwrap(), remaining, "{state}");
        assert_eq!(van.registration_van_spent, remaining == 5, "{state}");
        assert_eq!(
            queries::load_zkp2_inputs(&conn, ROUND, W, 0)
                .unwrap()
                .van_ballots,
            remaining,
            "{state}"
        );
        assert_eq!(
            queries::load_zkp2_inputs(&conn, ROUND, W, 0)
                .unwrap()
                .total_note_value,
            10 * BALLOT_DIVISOR + BALLOT_DIVISOR / 2,
            "the registration's note total never moves"
        );
    }
}

/// Tree reconstruction stops expecting the registration VAN at its leaf once
/// the batch may have spent it.
#[test]
fn a_dispatched_batch_retires_the_registration_van_expectation() {
    let db = db_with_slots();
    queries::store_van_position(&db.conn(), ROUND, W, 0, 3).unwrap();

    let entries = queries::load_van_tree_entries(&db.conn(), ROUND, W).unwrap();
    assert!(entries[0].expected_delegation_van.is_some());

    record_proxy_batch_submission(&db, "tracking");
    let entries = queries::load_van_tree_entries(&db.conn(), ROUND, W).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].position, 3);
    assert!(entries[0].expected_delegation_van.is_none());
}

/// Only counted slots that take every ballot make a bundle fully delegated: a
/// kept ballot, or no counted slot at all, leaves it voting whatever its
/// weight, and a bundle without setup is not refused while nothing counts.
#[test]
fn only_slots_that_take_every_ballot_make_a_bundle_fully_delegated() {
    let state = queries::VanState {
        bundle_index: 4,
        van_comm_rand: None,
        total_note_value: Some(7 * BALLOT_DIVISOR + 1),
        address_index: Some(0),
        registration_van: None,
        van_position: None,
        ea_pk: Vec::new(),
        proposal_authority: 0,
        registration_van_spent: true,
        delegated_ballots: 7,
    };
    assert!(state.fully_delegated().unwrap());

    let keeping_one = queries::VanState {
        delegated_ballots: 6,
        ..state.clone()
    };
    assert!(!keeping_one.fully_delegated().unwrap());

    let without_proxy_below_one_ballot = queries::VanState {
        total_note_value: Some(BALLOT_DIVISOR - 1),
        delegated_ballots: 0,
        ..state.clone()
    };
    assert_eq!(
        without_proxy_below_one_ballot.remaining_ballots().unwrap(),
        0
    );
    assert!(!without_proxy_below_one_ballot.fully_delegated().unwrap());

    let without_setup = queries::VanState {
        total_note_value: None,
        delegated_ballots: 0,
        ..state.clone()
    };
    assert!(!without_setup.fully_delegated().unwrap());

    let heavier = queries::VanState {
        delegated_ballots: 8,
        ..state
    };
    assert!(matches!(
        heavier.fully_delegated(),
        Err(VotingError::Internal { .. })
    ));
}

/// No vote is prepared from a VAN whose dispatched slots took every ballot,
/// and a definite rejection gives the weight, and the vote, back.
#[test]
fn a_fully_delegated_van_proves_no_vote() {
    let db = db_with_slots();
    // A third DC takes the 5 ballots the fixture's two leave.
    db.conn()
        .execute(
            "INSERT INTO proxy_slots
                 (round_id, wallet_id, bundle_index, dc_slot, delegate_index, weight_ballots,
                  layout)
             VALUES (?1, ?2, 0, 2, 9, 5, 0)",
            rusqlite::params![ROUND, W],
        )
        .unwrap();
    record_proxy_batch_submission(&db, "tracking");

    let van = queries::load_van_state(&db.conn(), ROUND, W, 0).unwrap();
    assert!(van.fully_delegated().unwrap());
    let refusal = queries::load_zkp2_inputs(&db.conn(), ROUND, W, 0).unwrap_err();
    assert!(
        matches!(&refusal, VotingError::InvalidInput { message } if message.contains("whole weight")),
        "{refusal}"
    );

    set_proxy_batch_state(&db, "rejected");
    assert_eq!(
        queries::load_zkp2_inputs(&db.conn(), ROUND, W, 0)
            .unwrap()
            .van_ballots,
        10
    );
}

/// A bundle with proxy slots and no landed 0x09 batch is held from casts, so
/// vote preparation, which runs before proving and again inside the
/// transaction that saves the vote, refuses it whatever the batch's
/// lifecycle says. A landed batch lifts the hold.
#[test]
fn a_bundle_held_by_its_proxy_batch_prepares_no_vote() {
    let db = db_with_slots();
    let prepare = |db: &VotingDb| queries::load_vote_preparation_state(&db.conn(), ROUND, W, 0, 1);
    let assert_held = |db: &VotingDb, case: &str| {
        let refusal = prepare(db).expect_err(case);
        assert!(
            matches!(&refusal, VotingError::InvalidInput { message } if message.contains("proxy batch holds")),
            "{case}: {refusal}"
        );
    };

    assert_held(&db, "planned or proved");
    record_proxy_batch_submission(&db, "submitting");
    for state in ["submitting", "rejected"] {
        set_proxy_batch_state(&db, state);
        assert_held(&db, state);
    }

    set_proxy_batch_state(&db, "confirmed");
    assert_eq!(prepare(&db).unwrap().zkp2.van_ballots, 5);
}
