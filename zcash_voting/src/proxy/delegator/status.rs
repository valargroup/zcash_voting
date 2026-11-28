//! The status view (plan §4.4 Status view, §4.5 Job), a plain view model
//! with no proofs and no reveal counter (decisions 35 and 42).
//!
//! **A DC's stage:**
//!
//! - while active, `Preparing` before submission and `Submitting` while on
//!   the wire, refused or not;
//! - once landed, `Included` until every share is handed off (LIV-2), then
//!   `HandedOff`;
//! - once closed, `HandedOff` if every share was handed off, otherwise
//!   `NotCounted`, including when its batch never landed (contracts-v1
//!   §15.1). A pending share 0 reveal only holds back the job's completion.
//!
//! **A delegation** (one delegate's DCs) is `NotCounted` if any DC is, else
//! its least advanced stage. Its ballots are its DCs'.
//!
//! **A delegate's ballot** is `Submitted` or `NoBallotYet` while active, and
//! `Counted` or `NotCounted` once closed.

use std::collections::BTreeMap;

use rusqlite::TransactionBehavior;

use super::DelegateBallotList;
use crate::{
    proxy::{
        batch::{proxy_batch_statuses_on, ProxyBatchStatus},
        in_transaction,
        planner::load_allocation_on,
        shares::{landed_dc_shares_on, DcDeliveryProgress, LandedDcShares},
        CommittedAllocation, ProxySlot,
    },
    session::ProxyBatchStage,
    storage::{queries, VotingDb},
    types::VotingError,
    wire::{
        DelegateBallotView, ProxyAllocationStatus, ProxyDelegationError, ProxyDelegationProgress,
        ProxyDelegationStage, ProxyDelegationView,
    },
};

/// Where the round stands on chain, from the status the host last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyRoundPhase {
    /// `PENDING` or `ACTIVE`: ballots and shares may still land.
    Active,
    /// `TALLYING` or `FINALIZED`. Each pool was counted or dropped.
    Closed,
}

/// The delegator's view of the wallet's allocation for `round_id`, or `None`
/// before one is committed. `ballots` must be the round's complete list.
/// Reads in one transaction and writes nothing.
pub fn status(
    db: &VotingDb,
    round_id: &str,
    ballots: &DelegateBallotList,
    phase: ProxyRoundPhase,
) -> Result<Option<ProxyAllocationStatus>, ProxyDelegationError> {
    let Some(round) = RoundDelegations::load(db, round_id, ballots)? else {
        return Ok(None);
    };
    Ok(Some(ProxyAllocationStatus {
        delegations: round.views(ballots, phase),
        kept_ballots: round.kept_ballots()?,
    }))
}

/// The delegation job's view, with [`status`]'s delegations from the same
/// inputs. Broadcast has started once a batch may be on the wire or landed.
/// The job completes once every delegation is handed off and, if
/// `planned_immediate`, `immediate_reveal_confirmed` (plan §4.5 Job).
pub fn progress(
    db: &VotingDb,
    round_id: &str,
    ballots: &DelegateBallotList,
    phase: ProxyRoundPhase,
) -> Result<Option<ProxyDelegationProgress>, ProxyDelegationError> {
    let Some(round) = RoundDelegations::load(db, round_id, ballots)? else {
        return Ok(None);
    };
    let reveals: Vec<bool> = round
        .landed
        .values()
        .filter_map(LandedDcShares::immediate_share)
        .map(|share| share.confirmed)
        .collect();
    let batch = |wanted: fn(ProxyBatchStage) -> bool| {
        round.batches.values().any(|status| wanted(status.progress))
    };
    Ok(Some(ProxyDelegationProgress {
        delegations: round.views(ballots, phase),
        broadcast_started: batch(|progress| {
            matches!(
                progress,
                ProxyBatchStage::Dispatched | ProxyBatchStage::Refused | ProxyBatchStage::Landed
            )
        }),
        planned_immediate: !reveals.is_empty(),
        immediate_reveal_confirmed: !reveals.is_empty()
            && reveals.iter().all(|confirmed| *confirmed),
        refused_by_chain: batch(|progress| progress == ProxyBatchStage::Refused),
    }))
}

impl ProxyDelegationProgress {
    /// Whether the allocation's job is done: there is a delegation, every
    /// one is `HandedOff`, and, if `planned_immediate`,
    /// `immediate_reveal_confirmed`.
    pub fn is_complete(&self) -> bool {
        !self.delegations.is_empty()
            && self
                .delegations
                .iter()
                .all(|view| view.stage == ProxyDelegationStage::HandedOff)
            && (!self.planned_immediate || self.immediate_reveal_confirmed)
    }

    /// Whether a running round's allocation still owes work: it is not
    /// [complete](Self::is_complete).
    pub fn owes_work(&self) -> bool {
        !self.is_complete()
    }
}

/// One round's allocation with everything its views read, from one snapshot.
struct RoundDelegations {
    allocation: CommittedAllocation,
    batches: BTreeMap<u32, ProxyBatchStatus>,
    /// Every landed DC, by `(bundle_index, dc_slot)`.
    landed: BTreeMap<(u32, u8), LandedDcShares>,
    /// Every bundle's registered weight, delegated or not.
    registered_ballots: u64,
}

impl RoundDelegations {
    /// Reads everything in one read transaction. `None` without an
    /// allocation. `ballots` must be the round's complete list.
    fn load(
        db: &VotingDb,
        round_id: &str,
        ballots: &DelegateBallotList,
    ) -> Result<Option<Self>, ProxyDelegationError> {
        if !ballots.is_for_round(round_id) {
            return Err(ProxyDelegationError::other(format!(
                "the delegate ballot list is not round {round_id}'s"
            )));
        }
        if !ballots.is_complete() {
            return Err(ProxyDelegationError::other(
                "the delegate ballot list is missing pages",
            ));
        }
        let wallet_id = db.wallet_id();
        in_transaction(
            db,
            TransactionBehavior::Deferred,
            "load proxy delegation status",
            |tx| {
                let Some(allocation) = load_allocation_on(tx, round_id, &wallet_id)? else {
                    return Ok(None);
                };
                let batches = proxy_batch_statuses_on(tx, &wallet_id, round_id)?;
                let landed = landed_dc_shares_on(tx, &wallet_id, round_id)?;
                let registered_ballots = queries::load_van_states(tx, round_id, &wallet_id)?
                    .iter()
                    .try_fold(0u64, |total, van| {
                        let ballots = van.registered_ballots()?;
                        total
                            .checked_add(ballots)
                            .ok_or_else(|| VotingError::Internal {
                                message: "the round's bundles hold more than u64 ballots"
                                    .to_owned(),
                            })
                    })?;
                Ok(Some(Self {
                    allocation,
                    batches,
                    landed,
                    registered_ballots,
                }))
            },
        )
    }

    /// One view per chosen delegate, in request order.
    fn views(
        &self,
        ballots: &DelegateBallotList,
        phase: ProxyRoundPhase,
    ) -> Vec<ProxyDelegationView> {
        self.allocation
            .choices()
            .iter()
            .map(|choice| {
                let slots: Vec<&ProxySlot> = self
                    .allocation
                    .slots()
                    .iter()
                    .filter(|slot| slot.delegate_index == choice.delegate_index)
                    .collect();
                ProxyDelegationView {
                    delegate_index: choice.delegate_index,
                    provider: choice.provider,
                    handle: choice.handle.clone(),
                    ballots: slots.iter().map(|slot| slot.weight_ballots).sum(),
                    stage: delegation_stage(slots.iter().map(|slot| {
                        let batch = self
                            .batches
                            .get(&slot.bundle_index)
                            .map_or(ProxyBatchStage::Planned, |status| status.progress);
                        let delivery = self
                            .landed
                            .get(&(slot.bundle_index, slot.dc_slot))
                            .map(LandedDcShares::progress);
                        dc_stage(batch, delivery, phase)
                    })),
                    ballot: ballot_view(ballots.options(choice.delegate_index), phase),
                }
            })
            .collect()
    }

    /// The ballots the wallet votes itself: total weight less every slot's.
    fn kept_ballots(&self) -> Result<u64, ProxyDelegationError> {
        let delegated: u64 = self
            .allocation
            .slots()
            .iter()
            .map(|slot| slot.weight_ballots)
            .sum();
        self.registered_ballots
            .checked_sub(delegated)
            .ok_or_else(|| {
                ProxyDelegationError::other(format!(
                    "the allocation delegates {delegated} ballots, more than the {} its \
                     bundles hold",
                    self.registered_ballots
                ))
            })
    }
}

/// One DC's stage (see the module docs).
pub(crate) fn dc_stage(
    batch: ProxyBatchStage,
    delivery: Option<DcDeliveryProgress>,
    phase: ProxyRoundPhase,
) -> ProxyDelegationStage {
    use ProxyDelegationStage as Stage;
    let active = phase == ProxyRoundPhase::Active;
    match batch {
        ProxyBatchStage::Planned | ProxyBatchStage::Proved if active => Stage::Preparing,
        ProxyBatchStage::Dispatched | ProxyBatchStage::Refused if active => Stage::Submitting,
        ProxyBatchStage::Landed => match delivery {
            Some(DcDeliveryProgress::Complete) => Stage::HandedOff,
            _ if active => Stage::Included,
            _ => Stage::NotCounted,
        },
        _ => Stage::NotCounted,
    }
}

/// A delegation's stage from its DCs' (see the module docs).
pub(crate) fn delegation_stage(
    stages: impl IntoIterator<Item = ProxyDelegationStage>,
) -> ProxyDelegationStage {
    use ProxyDelegationStage as Stage;
    let rank = |stage: &Stage| match stage {
        Stage::NotCounted => 0,
        Stage::Preparing => 1,
        Stage::Submitting => 2,
        Stage::Included => 3,
        Stage::HandedOff => 4,
    };
    // A loaded allocation gives every choice at least one slot.
    stages
        .into_iter()
        .min_by_key(rank)
        .unwrap_or(Stage::Preparing)
}

/// A delegate's ballot view from its options in the round's list.
pub(in crate::proxy) fn ballot_view(
    options: Option<&[u32]>,
    phase: ProxyRoundPhase,
) -> DelegateBallotView {
    match (phase, options) {
        (ProxyRoundPhase::Active, Some(options)) => DelegateBallotView::Submitted {
            options: options.to_vec(),
        },
        (ProxyRoundPhase::Active, None) => DelegateBallotView::NoBallotYet,
        (ProxyRoundPhase::Closed, Some(options)) => DelegateBallotView::Counted {
            options: options.to_vec(),
        },
        (ProxyRoundPhase::Closed, None) => DelegateBallotView::NotCounted,
    }
}
