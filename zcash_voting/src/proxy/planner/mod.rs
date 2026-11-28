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

use zcash_vote_delegate::DcShareLayout;

use crate::{
    vote_work::RoundHostContext,
    wire::{ProxyDelegateChoice, ProxyDelegationError, ProxyPreviewDelegation},
};

pub use allocation_lifecycle::{
    commit, commit_allocation, load_allocation, preview, preview_allocation,
};
pub(crate) use slot_positions::assign_dc_slots;

/// One bundle's weight, in ballots of 12,500,000 zatoshi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundleBallots {
    pub bundle_index: u32,
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
    quotas: Vec<ProxyPreviewDelegation>,
    kept_ballots: u64,
    layout: DcShareLayout,
    dcs: Vec<PlannedDc>,
}

impl AllocationPlan {
    /// Each choice's whole-ballot quota, in request order.
    pub fn quotas(&self) -> &[ProxyPreviewDelegation] {
        &self.quotas
    }

    /// The ballots the delegator keeps.
    pub fn kept_ballots(&self) -> u64 {
        self.kept_ballots
    }

    /// The share layout every DC of every batch uses.
    pub fn layout(&self) -> DcShareLayout {
        self.layout
    }

    /// The planned DCs in fill order (see `packing`), from `D` delegates up
    /// to `D + B − 1` for `B` bundles.
    pub fn dcs(&self) -> &[PlannedDc] {
        &self.dcs
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
    choices: Vec<ProxyDelegateChoice>,
    slots: Vec<ProxySlot>,
}

impl CommittedAllocation {
    /// The choices exactly as committed, in the delegator's order.
    pub fn choices(&self) -> &[ProxyDelegateChoice] {
        &self.choices
    }

    /// Every slot, ordered by bundle and then by slot. Every slot shares
    /// one share layout.
    pub fn slots(&self) -> &[ProxySlot] {
        &self.slots
    }
}

/// Plans `choices`, 1 to [`MAX_PROXY_DELEGATES`](super::MAX_PROXY_DELEGATES)
/// in the delegator's order, over `bundles` (any order), without touching
/// storage. Choice order breaks rounding and packing ties, and the
/// delegator keeps the weight the choices leave unnamed. Fails with the
/// first rule broken (`request_validation`, `rounding`).
pub fn plan_allocation(
    choices: &[ProxyDelegateChoice],
    bundles: &[BundleBallots],
    layout: DcShareLayout,
) -> Result<AllocationPlan, ProxyDelegationError> {
    let admitted = request_validation::admit(choices, bundles)?;
    let apportionment = rounding::apportion(admitted.total_ballots, choices)?;
    let quotas: Vec<ProxyPreviewDelegation> = choices
        .iter()
        .zip(&apportionment.quotas)
        .map(|(choice, &ballots)| ProxyPreviewDelegation {
            delegate_index: choice.delegate_index,
            ballots,
        })
        .collect();
    let dcs = packing::sequential_fill(&admitted.bundles, &quotas);
    Ok(AllocationPlan {
        quotas,
        kept_ballots: apportionment.kept_ballots,
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
