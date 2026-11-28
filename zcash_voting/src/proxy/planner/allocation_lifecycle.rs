//! The allocation lifecycle: preview, commit, then locked, and the client
//! API calls that run it.
//!
//! A round has at most one allocation per wallet. Commit re-plans in one
//! `BEGIN IMMEDIATE` transaction and writes every slot, so each DC's
//! `(d, w, slot, layout)` is durable before the first proof. After that no
//! API amends it, a second commit is refused, and storage triggers refuse
//! any change.

use rand::{rngs::OsRng, CryptoRng, RngCore};
use rusqlite::{Transaction, TransactionBehavior};
use zcash_vote_delegate::DcShareLayout;

use super::{
    allocation_store, assign_dc_slots, batch_layout, plan_allocation, AllocationPlan,
    BundleBallots, CommittedAllocation,
};
use crate::{
    proxy::in_transaction,
    round_planning::{first_unprovable_proxy_batch, UnprovableProxyBatch},
    session::{classify_ballot_intents, load_ballot_intents},
    share_tracking::lowest_bundle_with_vote_share_plan,
    storage::{queries, VotingDb},
    types::validate_vote_round_id_hex,
    vote_work::RoundHostContext,
    wire::{
        DelegateCheck, ProxyAllocationPreview, ProxyAllocationRequest, ProxyDelegateChoice,
        ProxyDelegationError, ProxyDelegationErrorKind,
    },
};

/// Plans `request` for the current wallet's bundles without recording it,
/// using the host's round timing for the layout. Refused without timing,
/// after the vote end, and once the round has an allocation.
pub fn preview(
    db: &VotingDb,
    request: &ProxyAllocationRequest,
    host: &RoundHostContext,
) -> Result<ProxyAllocationPreview, ProxyDelegationError> {
    admit(request, host)?;
    let plan = preview_allocation(db, &request.round_id, &request.choices, batch_layout(host))?;
    Ok(ProxyAllocationPreview::from(&plan))
}

/// Records `request` as the round's one, final allocation (D2).
///
/// `confirmed` is the preview the delegator confirmed. The commit plans
/// again under `host`'s round timing, and a plan that no longer previews the
/// same is refused with [`ProxyDelegationErrorKind::PlanChanged`]. `checks`
/// are the [`check_delegates`](crate::proxy::registry::check_delegates)
/// verdicts made just before, one per choice, and each must be `Ok`
/// (`CannotVerify` refuses with that kind). The commit's own transaction can
/// still refuse with `AlreadyVoted` or `RegistrationUnusable`, and with
/// `KeptShareUnanswered` when the plan keeps ballots and some proposal in
/// `roster`, the round's authenticated proposal ids, has no recorded ballot
/// intent (see [`commit_allocation`]). Slots are drawn from the OS CSPRNG.
pub fn commit(
    db: &VotingDb,
    request: &ProxyAllocationRequest,
    confirmed: &ProxyAllocationPreview,
    checks: &[DelegateCheck],
    host: &RoundHostContext,
    roster: &[u32],
) -> Result<(), ProxyDelegationError> {
    admit(request, host)?;
    require_allowed(request, checks)?;
    commit_allocation(
        db,
        &request.round_id,
        &request.choices,
        batch_layout(host),
        confirmed,
        roster,
        &mut OsRng,
    )?;
    Ok(())
}

/// Refuses a malformed round id, missing timing or an ended vote.
fn admit(
    request: &ProxyAllocationRequest,
    host: &RoundHostContext,
) -> Result<(), ProxyDelegationError> {
    validate_vote_round_id_hex(&request.round_id)?;
    let (Some(_), Some(vote_end)) = (host.ceremony_start_seconds, host.vote_end_time_seconds)
    else {
        return Err(ProxyDelegationError::other(
            "the round's timing is unknown, so its last-moment window is unknown",
        ));
    };
    if host.now_seconds >= vote_end {
        return Err(ProxyDelegationError::new(
            ProxyDelegationErrorKind::VoteEnded,
            format!(
                "voting ended at {vote_end}; the host clock reads {}",
                host.now_seconds
            ),
        ));
    }
    Ok(())
}

/// Refuses unless `checks` hold one allowing verdict per choice.
fn require_allowed(
    request: &ProxyAllocationRequest,
    checks: &[DelegateCheck],
) -> Result<(), ProxyDelegationError> {
    if checks.len() != request.choices.len() {
        return Err(ProxyDelegationError::other(format!(
            "{} registry checks for {} choices",
            checks.len(),
            request.choices.len()
        )));
    }
    for (choice, check) in request.choices.iter().zip(checks) {
        match check {
            DelegateCheck::Ok => {}
            DelegateCheck::CannotVerify => {
                return Err(ProxyDelegationError::new(
                    ProxyDelegationErrorKind::CannotVerify,
                    format!(
                        "delegate {}'s entry could not be verified",
                        choice.delegate_index
                    ),
                ))
            }
            DelegateCheck::Suspended
            | DelegateCheck::Retired
            | DelegateCheck::NotFound
            | DelegateCheck::Mismatch => {
                return Err(ProxyDelegationError::other(format!(
                    "delegate {} is refused: {check:?}",
                    choice.delegate_index
                )))
            }
        }
    }
    Ok(())
}

/// Plans `choices` for the current wallet's bundles in `round_id`, without
/// writing. `layout` comes from [`batch_layout`]. See `plan_uncommitted` for
/// the weights and refusals.
pub fn preview_allocation(
    db: &VotingDb,
    round_id: &str,
    choices: &[ProxyDelegateChoice],
    layout: DcShareLayout,
) -> Result<AllocationPlan, ProxyDelegationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Deferred,
        "preview proxy allocation",
        |tx| plan_uncommitted(tx, round_id, &wallet_id, choices, layout),
    )
}

/// Commits the current wallet's one allocation for the round. In one write
/// transaction it re-plans `choices` under `layout`, refuses with
/// [`ProxyDelegationErrorKind::PlanChanged`] unless the plan previews as
/// `expected`, the preview the delegator confirmed, draws slots from `rng`,
/// and writes the allocation and slots. Refusals are as in
/// `plan_uncommitted`. Returns what [`load_allocation`] reads back.
///
/// A plan that keeps ballots is refused with
/// [`ProxyDelegationErrorKind::KeptShareUnanswered`] unless every proposal
/// in `roster`, the round's authenticated proposal ids, has a recorded
/// ballot intent (a choice or a skip), read in the same transaction. So a
/// delegation never leaves the kept share to answer later.
pub fn commit_allocation<R: RngCore + CryptoRng>(
    db: &VotingDb,
    round_id: &str,
    choices: &[ProxyDelegateChoice],
    layout: DcShareLayout,
    expected: &ProxyAllocationPreview,
    roster: &[u32],
    rng: &mut R,
) -> Result<CommittedAllocation, ProxyDelegationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Immediate,
        "commit proxy allocation",
        |tx| {
            let plan = plan_uncommitted(tx, round_id, &wallet_id, choices, layout)?;
            if ProxyAllocationPreview::from(&plan) != *expected {
                return Err(ProxyDelegationError::new(
                    ProxyDelegationErrorKind::PlanChanged,
                    "the plan changed since its preview",
                ));
            }
            require_kept_share_answered(tx, round_id, &wallet_id, &plan, roster)?;
            let allocation = CommittedAllocation {
                choices: choices.to_vec(),
                slots: assign_dc_slots(&plan, rng),
            };
            allocation_store::insert_allocation(tx, round_id, &allocation, &wallet_id)?;
            Ok(allocation)
        },
    )
}

/// The current wallet's committed allocation for `round_id`, if any.
pub fn load_allocation(
    db: &VotingDb,
    round_id: &str,
) -> Result<Option<CommittedAllocation>, ProxyDelegationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Deferred,
        "load proxy allocation",
        |tx| allocation_store::load_allocation(tx, round_id, &wallet_id),
    )
}

/// Plans `choices` against each bundle's VAN weight in whole ballots, which
/// needs every bundle's delegation setup stored. Refuses once the round has
/// an allocation (including a retried commit), with `AlreadyVoted` or
/// `RegistrationUnusable` when a batch could never be proved, and with
/// `AlreadyVoted` once a vote's share plan is saved, since that plan fixed
/// the round's immediate share and a delegation could move it. A commit
/// reads these in its own write transaction, so a vote dispatched or planned
/// since the preview is seen.
fn plan_uncommitted(
    tx: &Transaction<'_>,
    round_id: &str,
    wallet_id: &str,
    choices: &[ProxyDelegateChoice],
    layout: DcShareLayout,
) -> Result<AllocationPlan, ProxyDelegationError> {
    if allocation_store::is_committed(tx, round_id, wallet_id)? {
        return Err(ProxyDelegationError::other(
            "the round already has a committed proxy allocation",
        ));
    }
    // A vote may have spent the bundle's VAN or reduced its proposal
    // authority, and delegation needs full authority (plan §4.4 Gating). A
    // bundle whose registration ended unconfirmed, or whose combined cast
    // the chain keeps refusing, can never fund a batch. Every batch waits
    // for every bundle's, so the first such bundle refuses the allocation.
    match first_unprovable_proxy_batch(tx, wallet_id, round_id)? {
        Some((bundle_index, UnprovableProxyBatch::VanMayBeSpent)) => {
            return Err(already_voted(bundle_index))
        }
        Some((bundle_index, UnprovableProxyBatch::RegistrationUnusable)) => {
            return Err(ProxyDelegationError::new(
                ProxyDelegationErrorKind::RegistrationUnusable,
                format!("bundle {bundle_index}'s registration cannot carry or fund a proxy batch"),
            ))
        }
        None => {}
    }
    if let Some(bundle_index) = lowest_bundle_with_vote_share_plan(tx, round_id, wallet_id)? {
        return Err(already_voted(bundle_index));
    }
    let bundles = queries::load_van_states(tx, round_id, wallet_id)?
        .into_iter()
        .map(|van| {
            if van.total_note_value.is_none() {
                return Err(ProxyDelegationError::other(format!(
                    "bundle {} has no stored delegation setup, so its weight is unknown",
                    van.bundle_index
                )));
            }
            Ok(BundleBallots {
                bundle_index: van.bundle_index,
                ballots: van.remaining_ballots()?,
            })
        })
        .collect::<Result<Vec<_>, ProxyDelegationError>>()?;
    plan_allocation(choices, &bundles, layout)
}

/// The refusal for the first bundle a vote may have spent, or whose saved
/// vote share plan fixed the round's immediate share.
fn already_voted(bundle_index: u32) -> ProxyDelegationError {
    ProxyDelegationError::new(
        ProxyDelegationErrorKind::AlreadyVoted,
        format!(
            "a vote of bundle {bundle_index} may already have spent its VAN or has a saved share \
             plan, so the wallet cannot delegate"
        ),
    )
}

/// Refuses a plan that keeps ballots while a `roster` proposal has no
/// ballot intent, or while `roster` is empty. The kept share's answers ride
/// with the commit, so one submit sends both.
fn require_kept_share_answered(
    tx: &Transaction<'_>,
    round_id: &str,
    wallet_id: &str,
    plan: &AllocationPlan,
    roster: &[u32],
) -> Result<(), ProxyDelegationError> {
    let kept_ballots = plan.kept_ballots();
    if kept_ballots == 0 {
        return Ok(());
    }
    let intents = load_ballot_intents(tx, round_id, wallet_id)?
        .into_iter()
        .collect();
    let unanswered = classify_ballot_intents(roster, &intents)?.open_proposals;
    if roster.is_empty() || !unanswered.is_empty() {
        return Err(ProxyDelegationError::new(
            ProxyDelegationErrorKind::KeptShareUnanswered,
            format!(
                "the allocation keeps {kept_ballots} ballots, but proposals {unanswered:?} have \
                 no answer: record the kept share's answers or delegate every ballot"
            ),
        ));
    }
    Ok(())
}
