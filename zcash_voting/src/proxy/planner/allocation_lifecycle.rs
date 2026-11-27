//! The allocation lifecycle: preview, commit, then locked.
//!
//! A round has at most one allocation per wallet. Commit re-plans in one
//! `BEGIN IMMEDIATE` transaction and writes every slot, so each DC's
//! `(d, w, slot, layout)` is durable before the first proof. After that no
//! API amends it, a second commit is refused, and storage triggers refuse
//! any change.

use rand::{CryptoRng, RngCore};
use rusqlite::{Transaction, TransactionBehavior};
use zcash_vote_delegate::DcShareLayout;

use super::{
    allocation_store, assign_dc_slots, plan_allocation, AllocationPlan, AllocationRequest,
    BundleBallots, CommittedAllocation, ProxyAllocationError,
};
use crate::{
    round_planning::{first_unprovable_proxy_batch, UnprovableProxyBatch},
    session::{classify_ballot_intents, load_ballot_intents},
    share_tracking::lowest_bundle_with_vote_share_plan,
    storage::{queries, VotingDb},
    types::VotingError,
    wire::ProxyAllocationPreview,
};

/// Plans `request` for the current wallet's bundles in `round_id`, without
/// writing. `layout` comes from [`batch_layout`](super::batch_layout). See
/// `plan_uncommitted` for the weights and refusals.
pub fn preview_allocation(
    db: &VotingDb,
    round_id: &str,
    request: &AllocationRequest,
    layout: DcShareLayout,
) -> Result<AllocationPlan, ProxyAllocationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Deferred,
        "preview proxy allocation",
        |tx| plan_uncommitted(tx, round_id, &wallet_id, request, layout),
    )
}

/// Commits the current wallet's one allocation for the round. In one write
/// transaction it re-plans `request` under `layout`, refuses
/// [`ProxyAllocationError::PlanChanged`] unless the plan previews as
/// `expected`, the preview the delegator confirmed, draws slots from `rng`,
/// and writes the allocation and slots. Refusals are as in `plan_uncommitted`. Returns what
/// [`load_allocation`] reads back.
///
/// A plan that keeps ballots is refused with
/// [`ProxyAllocationError::KeptShareUnanswered`] unless every proposal in
/// `roster`, the round's authenticated proposal ids, has a recorded ballot
/// intent (a choice or a skip), read in the same transaction. So a
/// delegation never leaves the kept share to answer later.
pub fn commit_allocation<R: RngCore + CryptoRng>(
    db: &VotingDb,
    round_id: &str,
    request: &AllocationRequest,
    layout: DcShareLayout,
    expected: &ProxyAllocationPreview,
    roster: &[u32],
    rng: &mut R,
) -> Result<CommittedAllocation, ProxyAllocationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Immediate,
        "commit proxy allocation",
        |tx| {
            let plan = plan_uncommitted(tx, round_id, &wallet_id, request, layout)?;
            if ProxyAllocationPreview::from(&plan) != *expected {
                return Err(ProxyAllocationError::PlanChanged);
            }
            require_kept_share_answered(tx, round_id, &wallet_id, &plan, roster)?;
            let allocation = CommittedAllocation {
                round_id: round_id.to_owned(),
                request: request.clone(),
                layout: plan.layout(),
                slots: assign_dc_slots(&plan, rng),
                committed_at: crate::proxy::unix_now(),
            };
            allocation_store::insert_allocation(tx, &allocation, &wallet_id)?;
            Ok(allocation)
        },
    )
}

/// The current wallet's committed allocation for `round_id`, if any.
pub fn load_allocation(
    db: &VotingDb,
    round_id: &str,
) -> Result<Option<CommittedAllocation>, ProxyAllocationError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Deferred,
        "load proxy allocation",
        |tx| allocation_store::load_allocation(tx, round_id, &wallet_id),
    )
}

/// Plans `request` against each bundle's VAN weight in whole ballots, which
/// needs every bundle's delegation setup stored. Refuses with
/// [`ProxyAllocationError::AlreadyCommitted`] once the round has an
/// allocation (including a retried commit), with `AlreadyVoted` or
/// `RegistrationUnusable` when a batch could never be proved, and with
/// `AlreadyVoted` once a vote's share plan is saved, since that plan fixed
/// the round's immediate share and a delegation could move it. A commit
/// reads these in its own write transaction, so a vote dispatched or planned
/// since the preview is seen.
fn plan_uncommitted(
    tx: &Transaction<'_>,
    round_id: &str,
    wallet_id: &str,
    request: &AllocationRequest,
    layout: DcShareLayout,
) -> Result<AllocationPlan, ProxyAllocationError> {
    if allocation_store::is_committed(tx, round_id, wallet_id)? {
        return Err(ProxyAllocationError::AlreadyCommitted);
    }
    match first_unprovable_proxy_batch(tx, wallet_id, round_id)? {
        Some((bundle_index, UnprovableProxyBatch::VanMayBeSpent)) => {
            return Err(ProxyAllocationError::AlreadyVoted { bundle_index })
        }
        Some((bundle_index, UnprovableProxyBatch::RegistrationUnusable)) => {
            return Err(ProxyAllocationError::RegistrationUnusable { bundle_index })
        }
        None => {}
    }
    if let Some(bundle_index) = lowest_bundle_with_vote_share_plan(tx, round_id, wallet_id)? {
        return Err(ProxyAllocationError::AlreadyVoted { bundle_index });
    }
    let bundles = queries::load_van_states(tx, round_id, wallet_id)?
        .into_iter()
        .map(|van| {
            if van.total_note_value.is_none() {
                return Err(ProxyAllocationError::BundleWeightUnknown {
                    bundle_index: van.bundle_index,
                });
            }
            Ok(BundleBallots {
                bundle_index: van.bundle_index,
                ballots: van.remaining_ballots()?,
            })
        })
        .collect::<Result<Vec<_>, ProxyAllocationError>>()?;
    Ok(plan_allocation(request, &bundles, layout)?)
}

/// Refuses a plan that keeps ballots while a `roster` proposal has no
/// ballot intent, or while `roster` is empty.
fn require_kept_share_answered(
    tx: &Transaction<'_>,
    round_id: &str,
    wallet_id: &str,
    plan: &AllocationPlan,
    roster: &[u32],
) -> Result<(), ProxyAllocationError> {
    if plan.kept_ballots() == 0 {
        return Ok(());
    }
    let intents = load_ballot_intents(tx, round_id, wallet_id)?
        .into_iter()
        .collect();
    let unanswered = classify_ballot_intents(roster, &intents)?.open_proposals;
    if roster.is_empty() || !unanswered.is_empty() {
        return Err(ProxyAllocationError::KeptShareUnanswered {
            kept_ballots: plan.kept_ballots(),
            unanswered,
        });
    }
    Ok(())
}

/// Runs `body` in one transaction, committing only when it succeeds.
fn in_transaction<T>(
    db: &VotingDb,
    behavior: TransactionBehavior,
    context: &str,
    body: impl FnOnce(&Transaction<'_>) -> Result<T, ProxyAllocationError>,
) -> Result<T, ProxyAllocationError> {
    let mut conn = db.conn();
    let tx = conn
        .transaction_with_behavior(behavior)
        .map_err(|error| VotingError::from_sqlite(context, &error))?;
    let value = body(&tx)?;
    tx.commit()
        .map_err(|error| VotingError::from_sqlite(context, &error))?;
    Ok(value)
}
