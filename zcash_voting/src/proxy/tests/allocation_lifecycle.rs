//! Preview, commit and lock: one allocation per round, persisted with every
//! slot before any proof, and never amended.

use super::fixtures::{
    answered_db_with_bundle_weights, dump_table, request, seeded_rng, set_bundle_note_total,
    FIXTURE_ROSTER, ROUND_ID, WALLET_ID,
};
use crate::{
    proxy::{
        commit_allocation, load_allocation, preview_allocation, AllocationRequest, DcShareLayout,
        ProxyAllocationError,
    },
    wire::ProxyAllocationPreview,
};

/// A request splitting a wallet across two delegates and keeping a fifth.
fn two_delegates() -> AllocationRequest {
    request(&[(17, 5_000), (3, 3_000)])
}

/// A plan that keeps ballots commits only once every roster proposal has an
/// answer, so the kept share is voted in the same submit. A plan that
/// delegates every ballot needs no answers. A refused commit writes nothing.
#[test]
fn a_commit_that_keeps_ballots_needs_the_kept_share_answered() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let attempt = |request: &AllocationRequest, roster: &[u32]| {
        let plan = preview_allocation(&db, ROUND_ID, request, DcShareLayout::Standard).unwrap();
        commit_allocation(
            &db,
            ROUND_ID,
            request,
            DcShareLayout::Standard,
            &ProxyAllocationPreview::from(&plan),
            roster,
            &mut seeded_rng(10),
        )
    };
    for (roster, unanswered) in [(&[1, 2][..], vec![2]), (&[][..], vec![])] {
        assert!(
            matches!(
                attempt(&two_delegates(), roster),
                Err(ProxyAllocationError::KeptShareUnanswered {
                    kept_ballots: 20,
                    unanswered: ref missing,
                }) if *missing == unanswered
            ),
            "{roster:?}"
        );
    }
    assert!(load_allocation(&db, ROUND_ID).unwrap().is_none());

    db.conn().execute("DELETE FROM ballot_intent", []).unwrap();
    attempt(&request(&[(17, 10_000)]), &[1, 2]).unwrap();
}

/// A commit stores exactly the previewed plan, and loading returns it
/// unchanged.
#[test]
fn a_committed_allocation_is_its_preview_and_reads_back_unchanged() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let plan =
        preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();
    // Each bundle stores a sub-ballot remainder of zatoshi, which is dropped.
    assert_eq!(plan.total_ballots(), 100);

    let committed = commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(1),
    )
    .unwrap();

    assert_eq!(committed.round_id(), ROUND_ID);
    assert_eq!(committed.request(), &two_delegates());
    assert_eq!(committed.layout(), DcShareLayout::Standard);
    assert_eq!(committed.quotas(), plan.quotas());
    let mut planned: Vec<_> = plan
        .dcs()
        .iter()
        .map(|dc| (dc.bundle_index, dc.delegate_index, dc.weight_ballots))
        .collect();
    let mut slotted: Vec<_> = committed
        .slots()
        .iter()
        .map(|slot| (slot.bundle_index, slot.delegate_index, slot.weight_ballots))
        .collect();
    planned.sort_unstable();
    slotted.sort_unstable();
    assert_eq!(planned, slotted);

    assert_eq!(
        load_allocation(&db, ROUND_ID).unwrap(),
        Some(committed),
        "the stored allocation reads back exactly as committed"
    );
}

/// After a commit, a retried commit, a different commit and a new preview
/// are all refused, and the stored rows do not change.
#[test]
fn no_call_amends_a_committed_allocation() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let plan =
        preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();
    commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(3),
    )
    .unwrap();
    let allocation_rows = dump_table(&db, "proxy_allocations");
    let slot_rows = dump_table(&db, "proxy_slots");

    // A retry of the same commit, a different allocation and a new preview
    // are all refused.
    let retried = commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(4),
    );
    let different = request(&[(9, 10_000)]);
    let different_plan = crate::proxy::plan_allocation(
        &different,
        &super::fixtures::bundles(&[60, 40]),
        DcShareLayout::Standard,
    )
    .unwrap();
    let replaced = commit_allocation(
        &db,
        ROUND_ID,
        &different,
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&different_plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(5),
    );
    let previewed = preview_allocation(&db, ROUND_ID, &different, DcShareLayout::Standard);
    for refusal in [
        retried.map(|_| ()),
        replaced.map(|_| ()),
        previewed.map(|_| ()),
    ] {
        assert!(
            matches!(refusal, Err(ProxyAllocationError::AlreadyCommitted)),
            "{refusal:?}"
        );
    }

    assert_eq!(dump_table(&db, "proxy_allocations"), allocation_rows);
    assert_eq!(dump_table(&db, "proxy_slots"), slot_rows);
}

/// Triggers refuse any rewrite of a committed allocation or slot plan, a
/// second layout in one batch, and a second slot nullifier.
#[test]
fn storage_refuses_to_rewrite_a_committed_allocation() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let plan =
        preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();
    commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(6),
    )
    .unwrap();
    let conn = db.conn();
    let refused = |sql: &str| {
        let error = conn.execute(sql, []).expect_err(sql);
        assert!(error.to_string().contains("proxy"), "{sql}: {error}");
    };

    refused("UPDATE proxy_allocations SET choices_json = '[]'");
    refused("UPDATE proxy_slots SET weight_ballots = weight_ballots + 1");
    // A batch has one layout, so a slot of the other layout cannot join it.
    refused(&format!(
        "INSERT INTO proxy_slots
             (round_id, wallet_id, bundle_index, dc_slot, delegate_index, weight_ballots, layout)
         VALUES ('{ROUND_ID}', '{WALLET_ID}', 0, 7, 99, 1, 1)"
    ));

    // The slot nullifier and randomizer are the one later write, made
    // together and once.
    conn.execute(
        "UPDATE proxy_slots SET dc_slot_nf = ?1, alpha_v = ?2
          WHERE bundle_index = 0 AND dc_slot = 0",
        [vec![0x5A_u8; 32], vec![0x5B_u8; 32]],
    )
    .expect("batch planning writes the slot nullifier and randomizer once");
    refused("UPDATE proxy_slots SET dc_slot_nf = zeroblob(32) WHERE dc_slot_nf IS NOT NULL");
    let unpaired = conn
        .execute(
            "UPDATE proxy_slots SET dc_slot_nf = ?1
              WHERE bundle_index = 0 AND dc_slot_nf IS NULL",
            [vec![0x5C_u8; 32]],
        )
        .expect_err("a slot nullifier is written with its randomizer");
    assert!(unpaired.to_string().contains("CHECK"), "{unpaired}");
}

/// A layout or bundle weight that changed since the preview, so the plan no
/// longer previews as the delegator confirmed, makes the commit fail with
/// `PlanChanged` and write nothing. A changed weight that leaves every quota
/// and the kept ballots as confirmed still commits.
#[test]
fn a_commit_refuses_a_plan_that_moved_since_its_preview() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let plan =
        preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();

    // The last-moment window opened between preview and commit.
    let moved_layout = commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::SingleShare,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(7),
    );
    assert!(
        matches!(moved_layout, Err(ProxyAllocationError::PlanChanged)),
        "{moved_layout:?}"
    );

    // A bundle's stored weight changed between preview and commit.
    set_bundle_note_total(&db.conn(), WALLET_ID, 1, Some(41 * crate::BALLOT_DIVISOR));
    let moved_weight = commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(8),
    );
    assert!(
        matches!(moved_weight, Err(ProxyAllocationError::PlanChanged)),
        "{moved_weight:?}"
    );
    assert_eq!(load_allocation(&db, ROUND_ID).unwrap(), None);
    assert!(dump_table(&db, "proxy_slots").is_empty());

    // The sub-ballot remainder moved, which changes no whole ballot.
    set_bundle_note_total(&db.conn(), WALLET_ID, 1, Some(40 * crate::BALLOT_DIVISOR));
    commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(9),
    )
    .expect("the plan previews as confirmed");
}

/// A bundle without stored delegation setup has no known weight, so planning
/// stops and names it.
#[test]
fn planning_needs_every_bundles_stored_weight() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    set_bundle_note_total(&db.conn(), WALLET_ID, 1, None);
    let refusal = preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard);
    assert!(
        matches!(
            refusal,
            Err(ProxyAllocationError::BundleWeightUnknown { bundle_index: 1 })
        ),
        "{refusal:?}"
    );
}

/// Runs `sql` against the fixture round with CHECK enforcement lifted: only
/// the rows' kind and state matter to the commit, so they stand in for the
/// lifecycle without the hashes and positions a real row carries.
fn record_round_fact(db: &crate::storage::VotingDb, sql: &str) {
    let conn = db.conn();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(sql, rusqlite::params![ROUND_ID, WALLET_ID])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .unwrap();
}

/// A vote of bundle 1 for proposal 1, storing `values` in `columns` beside
/// the required ones; both start with a comma when given.
fn vote_on_bundle_1(columns: &str, values: &str) -> String {
    format!(
        "INSERT INTO votes (round_id, wallet_id, bundle_index, proposal_id, choice, commitment,
                            created_at{columns})
         VALUES (?1, ?2, 1, 1, 0, zeroblob(32), 1{values})"
    )
}

/// A lifecycle row of `kind` in `state` for bundle 1.
fn lifecycle_row_on_bundle_1(kind: &str, proposal: &str, state: &str) -> String {
    format!(
        "INSERT INTO chain_submissions
             (identity_key, round_id, wallet_id, network, bundle_index, kind, proposal_id,
              generation_digest, state, committed_post_reservations, created_at, updated_at)
         VALUES (randomblob(32), ?1, ?2, 'testnet', 1, '{kind}', {proposal}, zeroblob(32),
                 '{state}', 1, 1, 1)"
    )
}

/// The commit re-reads every bundle inside its transaction and refuses one
/// whose batch could never be proved, since every batch waits for every
/// bundle's proof: a vote may have spent its VAN or reduced its proposal
/// authority, or its registration can never admit a batch. The preview came
/// first, so only the commit's own reads see it.
#[test]
fn a_commit_refuses_a_bundle_whose_batch_could_never_be_proved() {
    let vote_may_have_spent = [
        vote_on_bundle_1(", tx_hash", ", 'ab'"),
        vote_on_bundle_1(", vc_tree_position", ", 9"),
        lifecycle_row_on_bundle_1("vote", "1", "recovering"),
        lifecycle_row_on_bundle_1("vote", "1", "submitted_without_hash"),
    ];
    let registration_unusable = [
        lifecycle_row_on_bundle_1("delegation", "NULL", "rejected"),
        lifecycle_row_on_bundle_1("delegation", "NULL", "submitted_without_hash"),
    ];
    for (sql, expect_vote) in vote_may_have_spent
        .iter()
        .map(|sql| (sql, true))
        .chain(registration_unusable.iter().map(|sql| (sql, false)))
    {
        let db = answered_db_with_bundle_weights(&[60, 40]);
        let plan =
            preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();
        record_round_fact(&db, sql);

        let refused = commit_allocation(
            &db,
            ROUND_ID,
            &two_delegates(),
            DcShareLayout::Standard,
            &ProxyAllocationPreview::from(&plan),
            &FIXTURE_ROSTER,
            &mut seeded_rng(8),
        );

        match refused {
            Err(ProxyAllocationError::AlreadyVoted { bundle_index: 1 }) if expect_vote => {}
            Err(ProxyAllocationError::RegistrationUnusable { bundle_index: 1 }) if !expect_vote => {
            }
            other => panic!("{sql}: {other:?}"),
        }
        assert!(dump_table(&db, "proxy_slots").is_empty(), "{sql}");
    }

    // An undispatched vote without a saved share plan spent nothing: the
    // commit holds its bundle instead.
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let plan =
        preview_allocation(&db, ROUND_ID, &two_delegates(), DcShareLayout::Standard).unwrap();
    record_round_fact(&db, &vote_on_bundle_1("", ""));
    commit_allocation(
        &db,
        ROUND_ID,
        &two_delegates(),
        DcShareLayout::Standard,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(8),
    )
    .unwrap();
}
