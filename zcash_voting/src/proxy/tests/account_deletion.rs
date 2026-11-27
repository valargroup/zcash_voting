//! Account deletion removes every proxy table's rows for that account, and
//! round deletion removes the round's allocation.

use super::fixtures::{
    add_round, answer_roster, db_with_bundle_weights, dump_table, request, seeded_rng,
    FIXTURE_ROSTER, ROUND_ID, WALLET_ID,
};
use crate::{
    proxy::{commit_allocation, load_allocation, preview_allocation, DcShareLayout},
    storage::VotingDb,
};

/// A second account sharing the database.
const OTHER_WALLET_ID: &str = "proxy-other-wallet";

/// Every table schema version 26 adds.
const PROXY_TABLES: [&str; 4] = [
    "proxy_allocations",
    "proxy_slots",
    "proxy_delegate_keys",
    "proxy_delegate_ballots",
];

/// Commits a one-delegate allocation for `db`'s wallet and stores a
/// delegate-mode key and signed ballot for it.
fn delegate_and_hold_delegate_state(db: &VotingDb, wallet_id: &str) {
    let request = request(&[(17, 6_000)]);
    answer_roster(db, wallet_id);
    let plan = preview_allocation(db, ROUND_ID, &request, DcShareLayout::Standard).unwrap();
    commit_allocation(
        db,
        ROUND_ID,
        &request,
        DcShareLayout::Standard,
        &crate::wire::ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(11),
    )
    .unwrap();

    let conn = db.conn();
    conn.execute(
        "INSERT INTO proxy_delegate_keys
             (wallet_id, network, delegate_key, delegate_index, created_at)
         VALUES (?1, 'testnet', ?2, 17, 5)",
        rusqlite::params![wallet_id, vec![0xD1_u8; 32]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO proxy_delegate_ballots
             (wallet_id, network, round_id, delegate_index, delegate_key, options,
              signature, signed_at)
         VALUES (?1, 'testnet', ?2, 17, ?3, ?4, ?5, 6)",
        rusqlite::params![
            wallet_id,
            ROUND_ID,
            vec![0xD1_u8; 32],
            [1u32, 0, 2]
                .iter()
                .flat_map(|option| option.to_le_bytes())
                .collect::<Vec<u8>>(),
            vec![0x51_u8; 64]
        ],
    )
    .unwrap();
}

/// The number of `table`'s rows that belong to `wallet_id`.
fn rows_for(db: &VotingDb, table: &str, wallet_id: &str) -> i64 {
    db.conn()
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE wallet_id = ?1"),
            [wallet_id],
            |row| row.get(0),
        )
        .unwrap()
}

/// Account deletion empties all four proxy tables for that account and leaves
/// another account's rows alone.
#[test]
fn deleting_an_account_removes_its_rows_from_every_proxy_table() {
    let db = db_with_bundle_weights(&[60, 40]);
    delegate_and_hold_delegate_state(&db, WALLET_ID);
    let other = db.scoped(OTHER_WALLET_ID).unwrap();
    add_round(&other, OTHER_WALLET_ID, &[30]);
    delegate_and_hold_delegate_state(&other, OTHER_WALLET_ID);
    for table in PROXY_TABLES {
        assert!(rows_for(&db, table, WALLET_ID) > 0, "{table}");
    }

    db.clear_wallet_state().unwrap();

    for table in PROXY_TABLES {
        assert_eq!(rows_for(&db, table, WALLET_ID), 0, "{table}");
        assert!(
            rows_for(&db, table, OTHER_WALLET_ID) > 0,
            "{table}: another account keeps its rows"
        );
    }
    assert_eq!(load_allocation(&db, ROUND_ID).unwrap(), None);
    assert!(load_allocation(&other, ROUND_ID).unwrap().is_some());
}

/// Round deletion cascades to the allocation and its slots; delegate-mode
/// rows belong to the account and stay.
#[test]
fn deleting_a_round_removes_its_allocation_but_not_the_account_s_delegate_state() {
    let db = db_with_bundle_weights(&[60, 40]);
    delegate_and_hold_delegate_state(&db, WALLET_ID);

    db.clear_round(ROUND_ID).unwrap();

    assert!(dump_table(&db, "proxy_allocations").is_empty());
    assert!(dump_table(&db, "proxy_slots").is_empty());
    assert_eq!(rows_for(&db, "proxy_delegate_keys", WALLET_ID), 1);
    assert_eq!(rows_for(&db, "proxy_delegate_ballots", WALLET_ID), 1);
}
