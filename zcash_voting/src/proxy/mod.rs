//! Proxy delegation: handing part of a round's voting weight to public
//! delegates through delegation commitments (DCs), per proxy delegation
//! contracts-v1.
//!
//! The feature is dormant (see [`PROXY_DELEGATION_ENABLED`]). No existing flow
//! calls into this module, so the schema version 26 tables stay empty until a
//! wallet commits an allocation.
//!
//! A delegator's round runs in this order.
//! 1. **Allocate.** [`preview_allocation`] turns 1 to [`MAX_PROXY_DELEGATES`]
//!    percentage choices into whole-ballot quotas packed into the wallet's
//!    bundles. [`commit_allocation`] re-plans under the write lock, refuses
//!    a plan that moved since its preview or a kept share without answers,
//!    and stores the round's one immutable allocation with every DC's slot,
//!    so nothing a proof depends on is chosen later. From then on each
//!    bundle with slots is frozen and held from casts until its batch
//!    lands.
//! 2. **Derive secrets.** [`ProxyRoundKey`] derives each DC's seed from the
//!    round hotkey, never from wallet keys.
//! 3. **Plan and prove.** The `ProxyDelegate` step plans each bundle's batch
//!    once and proves it (ZKP1 when it carries the registration, then every
//!    ZKP4), stopping at a stored, proved, unsigned batch.
//! 4. **Submit.** The `AdvanceProxyBatch` step signs each batch with an
//!    expiry and submits it, re-signing an expired attempt. A batch the chain
//!    refuses stays pending and is sent again until it lands or voting ends,
//!    so a committed delegation is final. The bundle casts its remainder once
//!    the batch lands.
//!
//! [`registry`] reads the delegate lists and checks each chosen delegate's
//! proven entry before step 1 commits. The client
//! API calls [`availability`], [`preview`] and [`commit`] take and return
//! the [`crate::wire`] types.

pub(crate) mod batch;
pub mod delegate;
mod delegator;
mod planner;
pub mod registry;
mod secrets;

/// Whether this build offers proxy delegation to hosts.
///
/// While false no availability check reports the feature, so no allocation
/// is committed and round planning plans no proxy work. Only the unstable,
/// test-only `unstable-proxy-delegation` cargo feature sets it, which also
/// makes `WalletCapabilities::proxy_delegation` offer `"v1"`.
pub const PROXY_DELEGATION_ENABLED: bool = cfg!(feature = "unstable-proxy-delegation");

/// Most delegates one allocation may name, and so the most DCs one bundle's
/// batch carries (packing never gives a bundle two DCs to one delegate).
pub const MAX_PROXY_DELEGATES: usize = 8;

/// Number of DC slots in one bundle's batch: `dc_slot` is in
/// `0..MAX_DC_SLOTS` (contract §10, `DC_SLOT_BITS = 3`).
pub const MAX_DC_SLOTS: u8 = 8;

/// The whole round's weight in basis points.
pub const WHOLE_ALLOCATION_BPS: u16 = 10_000;

/// Blocks a signed batch attempt stays valid past the vote chain's height at
/// signing. Fixed, so the expiry never identifies a wallet.
pub const PROXY_BATCH_EXPIRY_BLOCKS: u64 = 500;

/// The current Unix time in seconds, or 0 if the clock is before 1970.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The share layout a stored `layout` column names (`layout_u8`), if any.
pub(crate) fn layout_from_column(value: i64) -> Option<DcShareLayout> {
    match value {
        0 => Some(DcShareLayout::Standard),
        1 => Some(DcShareLayout::SingleShare),
        _ => None,
    }
}

pub use delegator::{
    availability, commit, preview, GovNullifierProof, ProxyAvailabilityInputs,
    CIRCUIT_FINGERPRINTS, PROXY_DELEGATION_CHAIN_VERSION, PROXY_DELEGATION_WALLET_VERSION,
};
pub use planner::{
    batch_layout, commit_allocation, load_allocation, plan_allocation, preview_allocation,
    AllocationPlan, AllocationPlanError, AllocationRequest, BundleBallots, CommittedAllocation,
    DelegateChoice, DelegateQuota, PlannedDc, ProxyAllocationError, ProxySlot,
};
pub use secrets::{DcSeed, ProxyRoundKey};
pub use zcash_vote_delegate::{DcSeedInputs, DcShareLayout, DelegateKey, POOL_PROPOSAL_ID};

#[cfg(test)]
pub(crate) mod tests;
