//! The delegator's preview and commit calls with the client API's types: a
//! preview is the planner's exact split and records nothing, and a commit
//! records it once and only while the plan still previews the same, every
//! chosen delegate's registry check is `Ok` and the vote has not ended.

use super::fixtures::{answered_db_with_bundle_weights, choice, host_at, FIXTURE_ROSTER, ROUND_ID};
use crate::{
    proxy::{
        batch_layout, commit, load_allocation, preview, preview_allocation, AllocationRequest,
    },
    storage::VotingDb,
    vote_work::RoundHostContext,
    wire::{
        DelegateCheck, ProxyAllocationPreview, ProxyAllocationRequest, ProxyDelegationError,
        ProxyDelegationErrorKind,
    },
};

/// The vote end of every timed host here.
const VOTE_END: u64 = 100_000;

/// A host well before the round's last-moment window.
fn early() -> RoundHostContext {
    host_at(10, Some(0), Some(VOTE_END))
}

/// Delegates 3 and 4 with half and a quarter of the weight, keeping the rest.
fn request() -> ProxyAllocationRequest {
    ProxyAllocationRequest {
        round_id: ROUND_ID.to_owned(),
        choices: vec![choice(3, 5_000), choice(4, 2_500)],
    }
}

/// A verdict allowing every choice of [`request`].
fn allowed() -> Vec<DelegateCheck> {
    vec![DelegateCheck::Ok, DelegateCheck::Ok]
}

/// The error kind of a failed call.
fn kind<T: std::fmt::Debug>(result: Result<T, ProxyDelegationError>) -> ProxyDelegationErrorKind {
    result.expect_err("the call is refused").kind
}

/// Whether the round has an allocation.
fn committed(db: &VotingDb) -> bool {
    load_allocation(db, ROUND_ID).unwrap().is_some()
}

#[test]
fn a_preview_is_the_planner_s_split_and_records_nothing() {
    let db = answered_db_with_bundle_weights(&[100, 50]);
    for host in [early(), host_at(90_000, Some(0), Some(VOTE_END))] {
        let previewed = preview(&db, &request(), &host).unwrap();

        let plan = preview_allocation(
            &db,
            ROUND_ID,
            &AllocationRequest::from(&request()),
            batch_layout(&host),
        )
        .unwrap();
        assert_eq!(previewed, ProxyAllocationPreview::from(&plan));
        let delegations: Vec<_> = previewed
            .delegations
            .iter()
            .map(|delegation| (delegation.delegate_index, delegation.ballots))
            .collect();
        let quotas: Vec<_> = plan
            .quotas()
            .iter()
            .map(|quota| (quota.delegate_index, quota.ballots))
            .collect();
        assert_eq!(delegations, quotas);
        assert_eq!(previewed.kept_ballots, plan.kept_ballots());
        assert_eq!(previewed.dc_count as usize, plan.dc_count());
        assert_eq!(previewed.bundle_count as usize, plan.bundle_count());
        assert_eq!(previewed.single_share, host.is_last_moment());
    }
    assert!(!committed(&db));
}

/// A commit that keeps ballots with a roster proposal unanswered is refused
/// with its own kind and records nothing.
#[test]
fn a_commit_with_the_kept_share_unanswered_is_refused() {
    let db = answered_db_with_bundle_weights(&[100, 50]);
    let previewed = preview(&db, &request(), &early()).unwrap();
    assert_eq!(
        kind(commit(
            &db,
            &request(),
            &previewed,
            &allowed(),
            &early(),
            &[1, 2],
        )),
        ProxyDelegationErrorKind::KeptShareUnanswered
    );
    assert!(!committed(&db));
}

#[test]
fn a_commit_records_the_previewed_plan_once() {
    let db = answered_db_with_bundle_weights(&[100, 50]);
    let previewed = preview(&db, &request(), &early()).unwrap();

    commit(
        &db,
        &request(),
        &previewed,
        &allowed(),
        &early(),
        &FIXTURE_ROSTER,
    )
    .unwrap();

    let allocation = load_allocation(&db, ROUND_ID).unwrap().unwrap();
    let quotas: Vec<_> = allocation
        .quotas()
        .iter()
        .map(|quota| (quota.delegate_index, quota.ballots))
        .collect();
    assert_eq!(
        quotas,
        previewed
            .delegations
            .iter()
            .map(|delegation| (delegation.delegate_index, delegation.ballots))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        kind(commit(
            &db,
            &request(),
            &previewed,
            &allowed(),
            &early(),
            &FIXTURE_ROSTER,
        )),
        ProxyDelegationErrorKind::Other,
        "the allocation is final"
    );
    assert_eq!(
        kind(preview(&db, &request(), &early())),
        ProxyDelegationErrorKind::Other
    );
}

/// A commit needs an `Ok` verdict for every choice. The plan moving since
/// the preview is `commit_allocation`'s rule, tested with it.
#[test]
fn a_commit_is_refused_unless_the_registry_check_allows_every_choice() {
    let db = answered_db_with_bundle_weights(&[100, 50]);
    let confirmed = preview(&db, &request(), &early()).unwrap();
    for (case, checks, expected) in [
        (
            "a missing verdict",
            vec![DelegateCheck::Ok],
            ProxyDelegationErrorKind::Other,
        ),
        (
            "an unverifiable entry",
            vec![DelegateCheck::Ok, DelegateCheck::CannotVerify],
            ProxyDelegationErrorKind::CannotVerify,
        ),
        (
            "a refused delegate",
            vec![DelegateCheck::Suspended, DelegateCheck::Ok],
            ProxyDelegationErrorKind::Other,
        ),
    ] {
        assert_eq!(
            kind(commit(
                &db,
                &request(),
                &confirmed,
                &checks,
                &early(),
                &FIXTURE_ROSTER,
            )),
            expected,
            "{case}: {checks:?}"
        );
        assert!(!committed(&db), "{case}");
    }
}

/// A commit takes the round's timing from the host, as a preview does: it
/// is refused without it and after the vote end, and a host now inside the
/// last-moment window plans another layout than the one confirmed.
#[test]
fn a_commit_needs_the_round_s_timing_before_its_vote_end() {
    let db = answered_db_with_bundle_weights(&[100, 50]);
    let confirmed = preview(&db, &request(), &early()).unwrap();
    for (case, host, expected) in [
        (
            "no ceremony start",
            host_at(10, None, Some(VOTE_END)),
            ProxyDelegationErrorKind::Other,
        ),
        (
            "no vote end",
            host_at(10, Some(0), None),
            ProxyDelegationErrorKind::Other,
        ),
        (
            "an ended vote",
            host_at(VOTE_END, Some(0), Some(VOTE_END)),
            ProxyDelegationErrorKind::VoteEnded,
        ),
        (
            "the last-moment window opened",
            host_at(VOTE_END - 1, Some(0), Some(VOTE_END)),
            ProxyDelegationErrorKind::PlanChanged,
        ),
    ] {
        assert_eq!(
            kind(commit(
                &db,
                &request(),
                &confirmed,
                &allowed(),
                &host,
                &FIXTURE_ROSTER,
            )),
            expected,
            "{case}"
        );
        assert!(!committed(&db), "{case}");
    }
}

#[test]
fn a_request_the_wallet_cannot_fund_or_read_is_refused() {
    let db = answered_db_with_bundle_weights(&[1]);
    assert_eq!(
        kind(preview(&db, &request(), &early())),
        ProxyDelegationErrorKind::NotEnoughVotingPower,
        "two delegates need two ballots"
    );

    let mut misnamed = request();
    misnamed.round_id = "01".to_owned();
    assert_eq!(
        kind(preview(&db, &misnamed, &early())),
        ProxyDelegationErrorKind::Other
    );
}
