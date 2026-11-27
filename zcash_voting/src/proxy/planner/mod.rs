//! The allocation planner. A round's delegation choices become whole-ballot
//! quotas, then DCs in the wallet's bundles, and the result is committed once
//! and never amended (D2).
//!
//! [`plan_allocation`] is pure. `request_validation` admits the inputs,
//! `rounding` apportions quotas and `packing` lays them into bundles. At
//! commit, `allocation_lifecycle` re-plans, requires the preview the
//! delegator confirmed, and stores the allocation with the slots
//! `slot_positions` draws, in one transaction.

mod allocation_lifecycle;
mod allocation_store;
mod packing;
mod request_validation;
mod rounding;
mod slot_positions;

use thiserror::Error;
use zcash_vote_delegate::DcShareLayout;

use super::MAX_PROXY_DELEGATES;
use crate::{types::VotingError, vote_work::RoundHostContext};

pub use allocation_lifecycle::{commit_allocation, load_allocation, preview_allocation};
pub(crate) use slot_positions::assign_dc_slots;

/// The planner takes a delegator's choices in their client API type.
pub use crate::wire::ProxyDelegateChoice as DelegateChoice;

/// A delegator's request for one round, with 1 to [`MAX_PROXY_DELEGATES`]
/// choices in the delegator's order. The delegator keeps the weight the
/// choices leave unnamed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationRequest {
    /// The chosen delegates. Order breaks rounding and packing ties.
    pub choices: Vec<DelegateChoice>,
}

/// One bundle's weight, in ballots of 12,500,000 zatoshi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundleBallots {
    pub bundle_index: u32,
    pub ballots: u64,
}

/// One delegate's whole-ballot quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegateQuota {
    pub delegate_index: u32,
    /// Ballots received across every bundle.
    pub ballots: u64,
}

/// One planned DC, the part of a delegate's quota that one bundle carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedDc {
    /// The bundle whose VAN funds the DC.
    pub bundle_index: u32,
    /// The delegate the DC is for.
    pub delegate_index: u32,
    /// Weight `w`, at least 1 ballot.
    pub weight_ballots: u64,
}

/// A planned allocation, as a preview shows it. Only [`plan_allocation`]
/// builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationPlan {
    quotas: Vec<DelegateQuota>,
    kept_ballots: u64,
    total_ballots: u64,
    layout: DcShareLayout,
    dcs: Vec<PlannedDc>,
}

impl AllocationPlan {
    /// Each choice's quota, in request order.
    pub fn quotas(&self) -> &[DelegateQuota] {
        &self.quotas
    }

    /// The ballots the delegator keeps.
    pub fn kept_ballots(&self) -> u64 {
        self.kept_ballots
    }

    /// Every quota plus the kept ballots.
    pub fn total_ballots(&self) -> u64 {
        self.total_ballots
    }

    /// The share layout every DC of every batch uses.
    pub fn layout(&self) -> DcShareLayout {
        self.layout
    }

    /// Whether the batches use the one-share last-moment layout.
    pub fn single_share(&self) -> bool {
        self.layout == DcShareLayout::SingleShare
    }

    /// The planned DCs in fill order (see `packing`).
    pub fn dcs(&self) -> &[PlannedDc] {
        &self.dcs
    }

    /// The number of DCs, from `D` delegates up to `D + B − 1` for `B`
    /// bundles.
    pub fn dc_count(&self) -> usize {
        self.dcs.len()
    }

    /// The number of bundles carrying a DC, one batch each.
    pub fn bundle_count(&self) -> usize {
        let mut bundles: Vec<u32> = self.dcs.iter().map(|dc| dc.bundle_index).collect();
        bundles.sort_unstable();
        bundles.dedup();
        bundles.len()
    }
}

/// One committed DC, a slot of one bundle's batch. Every field but the
/// nullifier is written with the allocation and never changes, so each
/// re-prove rebuilds a byte-identical DC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxySlot {
    pub bundle_index: u32,
    /// Position in the batch, below [`MAX_DC_SLOTS`](super::MAX_DC_SLOTS).
    /// The circuit binds it into the slot nullifier (contract §10).
    pub dc_slot: u8,
    pub delegate_index: u32,
    /// Weight `w` in ballots.
    pub weight_ballots: u64,
    /// The batch's share layout, the same for every slot.
    pub layout: DcShareLayout,
    /// Written once by batch planning before the first proof. `None` until
    /// then.
    pub dc_slot_nullifier: Option<[u8; 32]>,
}

/// A round's committed allocation, as stored. No API changes its choices
/// or slot plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedAllocation {
    round_id: String,
    request: AllocationRequest,
    layout: DcShareLayout,
    slots: Vec<ProxySlot>,
    committed_at: u64,
}

impl CommittedAllocation {
    /// The round the allocation belongs to.
    pub fn round_id(&self) -> &str {
        &self.round_id
    }

    /// The choices exactly as committed.
    pub fn request(&self) -> &AllocationRequest {
        &self.request
    }

    /// The share layout of every batch.
    pub fn layout(&self) -> DcShareLayout {
        self.layout
    }

    /// Every slot, ordered by bundle and then by slot.
    pub fn slots(&self) -> &[ProxySlot] {
        &self.slots
    }

    /// Each choice's quota (the sum of its slots), in request order.
    pub fn quotas(&self) -> Vec<DelegateQuota> {
        self.request
            .choices
            .iter()
            .map(|choice| DelegateQuota {
                delegate_index: choice.delegate_index,
                ballots: self
                    .slots
                    .iter()
                    .filter(|slot| slot.delegate_index == choice.delegate_index)
                    .map(|slot| slot.weight_ballots)
                    .sum(),
            })
            .collect()
    }
}

/// Why a request cannot be planned.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AllocationPlanError {
    #[error("an allocation needs at least one delegate")]
    NoDelegates,
    #[error("an allocation names at most {MAX_PROXY_DELEGATES} delegates, got {count}")]
    TooManyDelegates { count: usize },
    /// A choice names delegate index 0, which is not a registry entry.
    #[error("delegate index 0 is not a registry entry")]
    ZeroDelegateIndex,
    #[error("delegate {delegate_index} is chosen more than once")]
    DuplicateDelegate { delegate_index: u32 },
    #[error("delegate {delegate_index} needs a share of at least 1 basis point")]
    ZeroBasisPoints { delegate_index: u32 },
    /// A choice's handle is not canonical for its provider, or its key is
    /// not a canonical `zvdk1…` delegate key, so no entry could hold them.
    #[error("delegate {delegate_index}'s handle or key is malformed")]
    InvalidDelegateIdentity { delegate_index: u32 },
    /// The choices total more than 10,000 basis points.
    #[error("choices total {total_bps} basis points, more than 10000")]
    BasisPointsExceedWhole { total_bps: u32 },
    #[error("the round has no bundle to delegate from")]
    NoBundles,
    #[error("bundle {bundle_index} is listed twice")]
    DuplicateBundle { bundle_index: u32 },
    #[error("bundle {bundle_index} holds no ballots")]
    EmptyBundle { bundle_index: u32 },
    #[error("the bundles' total weight overflows u64 ballots")]
    WeightOverflow,
    /// Fewer ballots than delegates, so the one-ballot floor fails.
    #[error(
        "{delegates} delegates need at least {delegates} ballots, the bundles hold {total_ballots}"
    )]
    InsufficientWeight {
        total_ballots: u64,
        delegates: usize,
    },
}

/// Why a durable allocation step failed.
#[derive(Debug, Error)]
pub enum ProxyAllocationError {
    #[error(transparent)]
    Plan(#[from] AllocationPlanError),
    /// A bundle's delegation setup is not stored yet, so its weight is
    /// unknown.
    #[error("bundle {bundle_index} has no stored delegation setup, so its weight is unknown")]
    BundleWeightUnknown { bundle_index: u32 },
    /// A vote may have spent `bundle_index`'s VAN or reduced its proposal
    /// authority. Delegation needs full authority (plan §4.4 Gating), and
    /// every batch waits for every bundle's. Also the lowest bundle with a
    /// saved vote share plan, which fixed the round's immediate share.
    #[error(
        "a vote of bundle {bundle_index} may already have spent its VAN or has a saved share plan, so the wallet cannot delegate"
    )]
    AlreadyVoted {
        /// The first such bundle.
        bundle_index: u32,
    },
    /// `bundle_index`'s registration ended unconfirmed or the chain keeps
    /// refusing its combined cast, so it can never fund a batch. Every batch
    /// waits for every bundle's.
    #[error("bundle {bundle_index}'s registration cannot carry or fund a proxy batch")]
    RegistrationUnusable {
        /// The first such bundle.
        bundle_index: u32,
    },
    /// The plan keeps ballots, but some proposal of the round's roster has
    /// no recorded answer. A commit that keeps ballots carries the kept
    /// share's answers, so one submit sends both.
    #[error(
        "the allocation keeps {kept_ballots} ballots, but proposals {unanswered:?} have no \
         answer: record the kept share's answers or delegate every ballot"
    )]
    KeptShareUnanswered {
        kept_ballots: u64,
        /// The roster's proposals without an answer, ascending. Empty when
        /// the caller passed no roster.
        unanswered: Vec<u32>,
    },
    /// The round already has a final allocation, such as a retried
    /// commit's own.
    #[error("the round already has a committed proxy allocation")]
    AlreadyCommitted,
    /// The bundle weights or batch layout changed since the preview, so the
    /// plan no longer gives the quotas the delegator confirmed. The host
    /// previews again.
    #[error("the plan changed since its preview")]
    PlanChanged,
    /// A stored allocation or slot cannot be read back as written.
    #[error("stored proxy allocation is unreadable: {message}")]
    CorruptStoredAllocation { message: String },
    #[error(transparent)]
    Storage(#[from] VotingError),
}

/// Plans `request` over `bundles` (any order), without touching storage.
/// Fails with the first [`AllocationPlanError`] rule broken.
pub fn plan_allocation(
    request: &AllocationRequest,
    bundles: &[BundleBallots],
    layout: DcShareLayout,
) -> Result<AllocationPlan, AllocationPlanError> {
    let admitted = request_validation::admit(request, bundles)?;
    let apportionment = rounding::apportion(admitted.total_ballots, request)?;
    let quotas: Vec<DelegateQuota> = request
        .choices
        .iter()
        .zip(&apportionment.quotas)
        .map(|(choice, &ballots)| DelegateQuota {
            delegate_index: choice.delegate_index,
            ballots,
        })
        .collect();
    let dcs = packing::sequential_fill(&admitted.bundles, &quotas);
    Ok(AllocationPlan {
        quotas,
        kept_ballots: apportionment.kept_ballots,
        total_ballots: admitted.total_ballots,
        layout,
        dcs,
    })
}

/// The batch layout under `host`. Single share inside the last-moment
/// window (the casts' rule), otherwise standard, including without
/// authenticated round timing.
pub fn batch_layout(host: &RoundHostContext) -> DcShareLayout {
    if host.is_last_moment() {
        DcShareLayout::SingleShare
    } else {
        DcShareLayout::Standard
    }
}
