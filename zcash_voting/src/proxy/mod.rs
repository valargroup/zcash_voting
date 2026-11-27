//! Proxy delegation: handing part of a round's voting weight to public
//! delegates through delegation commitments (DCs), per proxy delegation
//! contracts-v1.
//!
//! The feature is dormant (see [`PROXY_DELEGATION_ENABLED`]), so the schema
//! version 26 tables stay empty until a wallet commits an allocation.
//!
//! A delegator's round runs in this order.
//! 1. **Allocate.** [`preview_allocation`] packs 1 to [`MAX_PROXY_DELEGATES`]
//!    percentage choices into the wallet's bundles, and [`commit_allocation`]
//!    stores that plan once, with every DC's slot. From then on each bundle
//!    with slots is frozen and held from casts until its batch lands.
//! 2. **Derive secrets.** Each DC's seed comes from the round hotkey
//!    (`proxy_round_key`), never from wallet keys.
//! 3. **Plan and prove.** The `ProxyDelegate` step plans each bundle's batch
//!    once and proves it (ZKP1 when it carries the registration, then every
//!    ZKP4), stopping at a stored, proved, unsigned batch.
//! 4. **Submit.** The `AdvanceProxyBatch` step signs each batch with an
//!    expiry and submits it, re-signing an expired attempt. A batch the chain
//!    refuses stays pending and is sent again until it lands or voting ends,
//!    so a committed delegation is final. The bundle casts its remainder once
//!    the batch lands.
//! 5. **Deliver shares.** The `SubmitShares` step delivers each landed DC's
//!    shares. Background tracking then confirms each handed-off share's
//!    reveal and resends one no helper reveals, as it does a vote share's
//!    (module `shares`).
//!
//! [`registry`] reads the delegate lists and checks each chosen delegate's
//! proven entry before step 1 commits. The client
//! API calls [`availability`], [`preview`], [`commit`], [`progress`] and
//! [`status`] take and return the [`crate::wire`] types.
//! [`delegate`] is the other side: a delegate's own device registers its key,
//! signs one ballot per round and retires.

mod ballot_pages;
pub(crate) mod batch;
pub mod delegate;
mod delegator;
mod planner;
pub mod registry;
pub(crate) mod shares;

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

/// Runs `body` in one transaction of `behavior`, committing only when it
/// succeeds, for a body whose error type takes a
/// [`VotingError`](crate::types::VotingError).
pub(crate) fn in_transaction<T, E: From<crate::types::VotingError>>(
    db: &crate::storage::VotingDb,
    behavior: rusqlite::TransactionBehavior,
    context: &str,
    body: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, E>,
) -> Result<T, E> {
    let sqlite_error =
        |error: rusqlite::Error| crate::types::VotingError::from_sqlite(context, &error);
    let mut conn = db.conn();
    let tx = conn
        .transaction_with_behavior(behavior)
        .map_err(sqlite_error)?;
    let value = body(&tx)?;
    tx.commit().map_err(sqlite_error)?;
    Ok(value)
}

/// The round hotkey's proxy key `hrk` for one round (contract §11.1), the
/// root of every DC secret in it, which [`zcash_vote_delegate::dc_seed`]
/// expands into one DC's seed. `hotkey_secret` is the 32 bytes of the round
/// hotkey's Orchard spending key, derived as the vote flow derives it for
/// ZKP2. Nothing comes from the wallet seed, OVK or FVK, so a viewing-key
/// holder cannot read a delegation. Zeroed on drop.
pub(crate) fn proxy_round_key(
    hotkey_secret: &[u8; 32],
    network: crate::types::Network,
    round_id: &[u8; 32],
) -> zeroize::Zeroizing<[u8; 32]> {
    use crate::types::Network;
    // `net_u8` matches the chain-submission network rank.
    let network = match network {
        Network::Mainnet => zcash_vote_delegate::ZcashNetwork::Mainnet,
        Network::Testnet => zcash_vote_delegate::ZcashNetwork::Testnet,
        Network::Regtest => zcash_vote_delegate::ZcashNetwork::Regtest,
    };
    zeroize::Zeroizing::new(zcash_vote_delegate::hotkey_round_key(
        hotkey_secret,
        network,
        round_id,
    ))
}

/// The share layout a stored `layout` column names (`layout_u8`), if any.
pub(crate) fn layout_from_column(value: i64) -> Option<DcShareLayout> {
    match value {
        0 => Some(DcShareLayout::Standard),
        1 => Some(DcShareLayout::SingleShare),
        _ => None,
    }
}

/// A stored BLOB as `[u8; N]`, refusing another length with
/// `corrupt("a {name} that is not {N} bytes")`.
pub(in crate::proxy) fn blob_array<const N: usize, E>(
    name: &str,
    bytes: impl AsRef<[u8]>,
    corrupt: impl FnOnce(&str) -> E,
) -> Result<[u8; N], E> {
    bytes
        .as_ref()
        .try_into()
        .map_err(|_| corrupt(&format!("a {name} that is not {N} bytes")))
}

pub use delegator::{
    availability, commit, preview, progress, status, DelegateBallotList, GovNullifierProof,
    ProxyAvailabilityInputs, ProxyRoundPhase, CIRCUIT_FINGERPRINTS, PROXY_DELEGATION_CHAIN_VERSION,
    PROXY_DELEGATION_WALLET_VERSION,
};
pub use planner::{
    batch_layout, commit_allocation, load_allocation, plan_allocation, preview_allocation,
    AllocationPlan, BundleBallots, CommittedAllocation, PlannedDc, ProxySlot,
};
pub use zcash_vote_delegate::{DcSeedInputs, DcShareLayout, DelegateKey, POOL_PROPOSAL_ID};

#[cfg(test)]
pub(crate) mod tests;
