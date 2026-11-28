//! A committed proxy allocation's bundles: `ProxyDelegate` is planned for a
//! batch not yet proved, and the two-way hold keeps a bundle's proxy batch and
//! its casts from spending the same VAN.

use super::fixtures::*;
use crate::phases::{DelegationPhase, VotePhase};
use crate::round_planning::{
    blocking_prerequisite, classify_round, plan_from_snapshot, Obligation,
};
use crate::session::{Decision, NextStep, ProxyBatchStage, RoundPlanAction};

/// The bundles `obligations` plan a proxy batch for.
fn proxy_bundles(obligations: &[Obligation]) -> Vec<u32> {
    obligations
        .iter()
        .filter_map(|obligation| match obligation {
            Obligation::ProxyDelegate { bundle_index } => Some(*bundle_index),
            _ => None,
        })
        .collect()
}

/// Whether `obligations` reconcile any unit on `bundle`.
fn reconciles(obligations: &[Obligation], bundle: u32) -> bool {
    obligations.iter().any(|obligation| {
        matches!(
            obligation,
            Obligation::ReconcileChain { bundle_index, .. } if *bundle_index == bundle
        )
    })
}

/// A proxy-only round, with no ballot recorded, still plans the batch and
/// with it the registration.
#[test]
fn a_proxy_only_round_plans_its_batch_without_a_ballot() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Prepared)
        .proxy_batch(0, ProxyBatchStage::Planned)
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1, 2]).unwrap();

    assert_eq!(
        plan.next_steps,
        vec![NextStep::ProxyDelegate { bundle_index: 0 }]
    );
    assert_eq!(plan.primary_action, RoundPlanAction::Delegate);
    assert_eq!(plan.delegation_bundles_needing_work, vec![0]);
    assert!(
        plan.delegation_bundles_needing_signing.is_empty(),
        "proving a batch asks the voter for no signature"
    );
}

/// An undispatched vote on a bundle whose batch is pending is not dispatched:
/// the batch spends the VAN first.
#[test]
fn a_pending_batch_holds_an_undispatched_vote() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .vote(0, 1, 1, VotePhase::Committed)
        .proxy_batch(0, ProxyBatchStage::Planned)
        .intent(1, Decision::Choice(1))
        .build();

    let obligations = classify_round(&snapshot, &[1]).unwrap();

    assert!(!reconciles(&obligations.obligations, 0));
    assert_eq!(proxy_bundles(&obligations.obligations), vec![0]);
    assert!(obligations.withheld_casts.contains(&1));
}

/// A vote that may have spent the VAN holds the batch: it is still driven to
/// resolution, and no proof is planned while it is on the wire, landed, or
/// possibly landed. A definite rejection spent nothing and holds nothing.
#[test]
fn a_cast_that_may_have_spent_the_van_holds_the_batch() {
    for (phase, holds) in [
        (VotePhase::Submitted, true),
        (VotePhase::SubmissionManaged, true),
        (VotePhase::SubmittedWithoutHash, true),
        (VotePhase::Confirmed, true),
        (VotePhase::SubmissionRejected, false),
    ] {
        let snapshot = snapshot()
            .bundle(0, DelegationPhase::Confirmed)
            .vote(0, 1, 1, phase)
            .proxy_batch(0, ProxyBatchStage::Planned)
            .intent(1, Decision::Choice(1))
            .build();

        let obligations = classify_round(&snapshot, &[1]).unwrap();

        assert_eq!(
            proxy_bundles(&obligations.obligations).is_empty(),
            holds,
            "{phase:?}"
        );
        if matches!(phase, VotePhase::Submitted | VotePhase::SubmissionManaged) {
            assert!(
                reconciles(&obligations.obligations, 0),
                "{phase:?} is still reconciled"
            );
        }
    }
}

/// A delegation the lifecycle still owns holds the batch until it resolves;
/// once confirmed, the batch is planned against the chain.
#[test]
fn a_delegation_in_flight_holds_the_batch_until_it_resolves() {
    for (phase, plans_batch) in [
        (DelegationPhase::Submitted, false),
        (DelegationPhase::SubmissionManaged, false),
        (DelegationPhase::SubmittedWithoutHash, false),
        (DelegationPhase::SubmissionRejected, false),
        (DelegationPhase::Prepared, true),
        (DelegationPhase::PcztBuilt, true),
        (DelegationPhase::Proved, true),
        (DelegationPhase::Confirmed, true),
    ] {
        let snapshot = snapshot()
            .bundle(0, phase)
            .proxy_batch(0, ProxyBatchStage::Planned)
            .build();

        let obligations = classify_round(&snapshot, &[1]).unwrap();

        assert_eq!(
            proxy_bundles(&obligations.obligations),
            if plans_batch { vec![0] } else { vec![] },
            "{phase:?}"
        );
    }
}

/// The step projects back to its obligation, sorts with delegation work, and
/// is a prerequisite of nothing on another bundle.
#[test]
fn a_proxy_step_resolves_to_its_obligation() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::PcztBuilt)
        .bundle(1, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchStage::Planned)
        .intent(1, Decision::Choice(1))
        .build();
    let obligations = classify_round(&snapshot, &[1]).unwrap();
    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    let step = NextStep::ProxyDelegate { bundle_index: 0 };
    assert_eq!(plan.next_steps.first(), Some(&step));
    assert!(matches!(
        crate::round_planning::resolve_step(&obligations.obligations, &step),
        Some(Obligation::ProxyDelegate { bundle_index: 0 })
    ));
    let cast = NextStep::CastVote {
        bundle_index: 1,
        proposal_id: 1,
        choice: 1,
    };
    assert!(plan.next_steps.contains(&cast));
    assert_eq!(blocking_prerequisite(&plan.next_steps, &cast), None);
    assert_eq!(blocking_prerequisite(&plan.next_steps, &step), None);
}

/// The snapshot reads every bundle's batch progress in its one transaction:
/// committed slots plan a batch, and a bundle without slots has none.
#[test]
fn the_snapshot_reads_each_bundle_s_batch_progress() {
    use crate::round::VotingDb;
    use crate::round_planning::load_round_snapshot;
    use crate::storage::queries;

    const WALLET: &str = "proxy-hold";
    let db = VotingDb::open_in_memory().unwrap();
    db.set_wallet_id(WALLET);
    {
        let conn = db.conn();
        queries::insert_round(
            &conn,
            WALLET,
            crate::Network::Testnet,
            &crate::types::VotingRoundParams {
                vote_round_id: ROUND.to_string(),
                snapshot_height: 1_000,
                ea_pk: vec![0xEA; 32],
                nc_root: vec![0xAA; 32],
                nullifier_imt_root: vec![0xBB; 32],
            },
            None,
        )
        .unwrap();
        for bundle_index in 0..3 {
            queries::insert_bundle(&conn, ROUND, WALLET, bundle_index, &[]).unwrap();
        }
        conn.execute(
            "INSERT INTO proxy_allocations
                 (round_id, wallet_id, choices_json, committed_at)
             VALUES (?1, ?2, '[{}]', 1)",
            rusqlite::params![ROUND, WALLET],
        )
        .unwrap();
        for (bundle_index, dc_slot) in [(0, 0), (0, 1), (1, 0)] {
            conn.execute(
                "INSERT INTO proxy_slots
                     (round_id, wallet_id, bundle_index, dc_slot, delegate_index,
                      weight_ballots, layout)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, 0)",
                rusqlite::params![ROUND, WALLET, bundle_index, dc_slot, 7 + dc_slot],
            )
            .unwrap();
        }
    }
    let progress = || {
        db.read_transaction("test", |tx| load_round_snapshot(tx, WALLET, ROUND))
            .unwrap()
            .proxy_batches
            .into_iter()
            .map(|(bundle_index, status)| (bundle_index, status.progress))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        progress(),
        vec![(0, ProxyBatchStage::Planned), (1, ProxyBatchStage::Planned)],
        "bundle 2 carries no slot"
    );
}
