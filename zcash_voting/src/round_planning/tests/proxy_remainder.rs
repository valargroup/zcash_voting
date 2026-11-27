//! A bundle whose dispatched or landed proxy DCs took its whole weight is
//! fully delegated: its zero-weight VAN is never cast from (plan CF-2), so it
//! owes no ballot and plans no cast, and a round of nothing else has nothing
//! left to vote. A bundle with weight left votes exactly as before.

use super::fixtures::*;
use crate::phases::{DelegationPhase, VotePhase};
use crate::proxy::batch::ProxyBatchProgress;
use crate::round_planning::{classify_round, plan_from_snapshot, Obligation};
use crate::session::{Decision, NextStep};
use crate::share_policy::ImmediateShareKey;
use crate::share_tracking::CommitmentRef;

/// The roster every test plans against.
const ROSTER: [u32; 2] = [1, 2];

/// Adds `bundle_index` with its delegation confirmed and its batch landed.
fn landed(builder: SnapshotBuilder, bundle_index: u32) -> SnapshotBuilder {
    builder
        .bundle(bundle_index, DelegationPhase::Confirmed)
        .proxy_batch(bundle_index, ProxyBatchProgress::Landed)
}

/// Records a choice for both roster proposals.
fn decided(builder: SnapshotBuilder) -> SnapshotBuilder {
    builder
        .intent(1, Decision::Choice(0))
        .intent(2, Decision::Choice(1))
}

/// Every `(bundle, proposal)` the steps cast.
fn casts(steps: &[NextStep]) -> Vec<(u32, u32)> {
    steps
        .iter()
        .filter_map(|step| match *step {
            NextStep::CastVote {
                bundle_index,
                proposal_id,
                ..
            } => Some((bundle_index, proposal_id)),
            _ => None,
        })
        .collect()
}

/// Whether `obligations` hold any fresh-cast work: a cast, a cast withheld
/// for the ballot, or the delegation prerequisite of one.
fn owes_cast_work(obligations: &[Obligation]) -> bool {
    obligations.iter().any(|obligation| {
        matches!(
            obligation,
            Obligation::Cast { .. } | Obligation::Blocked { .. } | Obligation::Delegate { .. }
        )
    })
}

/// Delegating everything leaves the round nothing to vote: no proposal is
/// open, and recorded choices plan no cast and withhold nothing.
#[test]
fn delegating_everything_owes_no_ballot_and_no_cast_even_with_intents() {
    let undecided = landed(snapshot(), 0).fully_delegated(0).build();
    let plan = plan_from_snapshot(&undecided, &ROSTER).unwrap();
    assert!(plan.open_proposals.is_empty(), "no ballot is owed");
    assert!(plan.nothing_left_to_vote);
    assert_eq!(plan.fully_delegated_bundles, vec![0]);
    assert!(!plan.needs_draft_setup);
    assert_eq!(plan.next_steps, Vec::new(), "no step is owed");

    let decided = decided(landed(snapshot(), 0).fully_delegated(0)).build();
    let obligations = classify_round(&decided, &ROSTER).unwrap();
    assert!(!owes_cast_work(&obligations.obligations));
    assert!(obligations.withheld_casts.is_empty(), "nothing is owed");
    assert!(
        obligations.choice_proposals.is_empty(),
        "no bundle casts the recorded choices"
    );
    let plan = plan_from_snapshot(&decided, &ROSTER).unwrap();
    assert_eq!(casts(&plan.next_steps), Vec::new());
    assert!(plan.nothing_left_to_vote);
    assert!(plan.open_proposals.is_empty());
    assert_eq!(
        plan.immediate_share_key, None,
        "no vote will exist to carry the round's immediate share"
    );
}

/// A kept remainder leaves weight on the bundle, which owes the ballot and
/// casts it as any bundle does.
#[test]
fn a_kept_remainder_still_votes() {
    let undecided = landed(snapshot(), 0).build();
    let plan = plan_from_snapshot(&undecided, &ROSTER).unwrap();
    assert_eq!(plan.open_proposals, vec![1, 2]);
    assert!(!plan.nothing_left_to_vote);
    assert!(plan.fully_delegated_bundles.is_empty());

    let decided = decided(landed(snapshot(), 0)).build();
    let plan = plan_from_snapshot(&decided, &ROSTER).unwrap();
    assert_eq!(casts(&plan.next_steps), vec![(0, 1), (0, 2)]);
}

/// In a round mixing both, the ballot is still owed for the bundle with
/// weight, which alone casts it; the choice is decided once that bundle's
/// votes confirm, and an unrostered intent still withholds its cast.
#[test]
fn a_mixed_round_casts_only_from_the_bundle_with_weight() {
    let mixed = || {
        landed(snapshot(), 0)
            .fully_delegated(0)
            .bundle(1, DelegationPhase::Confirmed)
    };

    let plan = plan_from_snapshot(&mixed().build(), &ROSTER).unwrap();
    assert_eq!(plan.open_proposals, vec![1, 2]);
    assert!(!plan.nothing_left_to_vote);
    assert_eq!(plan.fully_delegated_bundles, vec![0]);

    let decided_round = decided(mixed()).build();
    let obligations = classify_round(&decided_round, &ROSTER).unwrap();
    assert!(obligations.withheld_casts.is_empty());
    assert_eq!(obligations.choice_proposals, vec![1, 2]);
    let plan = plan_from_snapshot(&decided_round, &ROSTER).unwrap();
    assert_eq!(casts(&plan.next_steps), vec![(1, 1), (1, 2)]);
    assert!(!plan.all_decided);

    let voted = decided(mixed())
        .vote(1, 1, 0, VotePhase::Confirmed)
        .vote(1, 2, 1, VotePhase::Confirmed)
        .build();
    let plan = plan_from_snapshot(&voted, &ROSTER).unwrap();
    assert!(casts(&plan.next_steps).is_empty());
    assert!(
        plan.all_decided,
        "the fully delegated bundle has no vote to wait for"
    );

    let stale = decided(mixed()).intent(9, Decision::Choice(0)).build();
    let plan = plan_from_snapshot(&stale, &ROSTER).unwrap();
    assert_eq!(plan.unrostered_intents, vec![9]);
    assert!(casts(&plan.next_steps).is_empty());
}

/// With nothing left to vote, an intent for a proposal the roster no longer
/// lists withholds nothing, so it is not reported for the host to clear.
#[test]
fn an_unrostered_intent_does_not_hold_a_round_with_nothing_left_to_vote() {
    let round = decided(landed(snapshot(), 0).fully_delegated(0))
        .intent(9, Decision::Choice(0))
        .build();

    let plan = plan_from_snapshot(&round, &ROSTER).unwrap();

    assert!(plan.unrostered_intents.is_empty());
    assert!(plan.open_proposals.is_empty());
    assert!(plan.nothing_left_to_vote);
}

/// An undispatched vote on a fully delegated bundle is never dispatched:
/// dispatching it is the second half of a cast.
#[test]
fn a_fully_delegated_bundle_never_dispatches_an_undispatched_vote() {
    let round = decided(landed(snapshot(), 0).fully_delegated(0))
        .vote(0, 1, 0, VotePhase::Committed)
        .build();

    let obligations = classify_round(&round, &ROSTER).unwrap();

    assert!(!obligations
        .obligations
        .iter()
        .any(|obligation| matches!(obligation, Obligation::ReconcileChain { .. })));
    assert!(!owes_cast_work(&obligations.obligations));
    assert!(obligations.withheld_casts.is_empty());
}

/// A vote already on the chain lifecycle is driven as on any bundle: a
/// submitted one is reconciled and a confirmed one owes its shares. This
/// planner never casts on a fully delegated bundle, so only a build without
/// the rule could have left one there.
#[test]
fn a_vote_already_on_the_chain_lifecycle_is_still_driven() {
    let round = decided(landed(snapshot(), 0).fully_delegated(0))
        .vote(0, 1, 0, VotePhase::Submitted)
        .vote(0, 2, 1, VotePhase::Confirmed)
        .build();

    let plan = plan_from_snapshot(&round, &ROSTER).unwrap();

    assert!(plan.next_steps.contains(&NextStep::AdvanceVote {
        bundle_index: 0,
        proposal_id: 1,
    }));
    assert!(plan.next_steps.contains(&NextStep::SubmitShares {
        bundle_index: 0,
        commitment: CommitmentRef::Vote { proposal_id: 2 },
        share_index: 0,
    }));
    assert!(casts(&plan.next_steps).is_empty());
    assert!(plan.nothing_left_to_vote);
}

/// The round's immediate share belongs to the highest bundle that still
/// votes. With bundle 1 fully delegated, bundle 0 casts the ballot and
/// carries the designation; naming bundle 1 would name a share no vote will
/// ever have. A designation already persisted stands.
#[test]
fn the_immediate_share_skips_a_fully_delegated_higher_bundle() {
    let round =
        || decided(landed(snapshot().bundle(0, DelegationPhase::Confirmed), 1)).fully_delegated(1);

    let plan = plan_from_snapshot(&round().build(), &ROSTER).unwrap();

    assert_eq!(casts(&plan.next_steps), vec![(0, 1), (0, 2)]);
    assert_eq!(
        plan.immediate_share_key,
        Some(ImmediateShareKey {
            bundle_index: 0,
            proposal_id: 1,
            share_index: 0,
        })
    );

    let persisted = ImmediateShareKey {
        bundle_index: 0,
        proposal_id: 2,
        share_index: 0,
    };
    let mut designated = round().build();
    designated.persisted_immediate_share = Some(persisted);
    assert_eq!(
        plan_from_snapshot(&designated, &ROSTER)
            .unwrap()
            .immediate_share_key,
        Some(persisted)
    );
}
