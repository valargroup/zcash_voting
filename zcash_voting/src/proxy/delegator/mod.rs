//! The delegator's client API calls (contracts-v1 §15.1), taking and
//! returning the [`crate::wire`] types Vizor's bridge exposes to Dart.
//!
//! In round order: [`availability`] (before [`preview`] and again just before
//! [`commit`]), [`preview`], [`commit`], then [`progress`] and [`status`]
//! while the delegation runs.
//! Every call fails with a [`ProxyDelegationError`](crate::wire::ProxyDelegationError).

mod availability;
mod ballots;
mod status;

pub use super::planner::{commit, preview};
pub use availability::{
    availability, GovNullifierProof, ProxyAvailabilityInputs, CIRCUIT_FINGERPRINTS,
    PROXY_DELEGATION_CHAIN_VERSION, PROXY_DELEGATION_WALLET_VERSION,
};
pub use ballots::DelegateBallotList;
pub use status::{progress, status, ProxyRoundPhase};

#[cfg(test)]
pub(crate) use availability::evaluate_availability;
#[cfg(test)]
pub(crate) use status::{dc_stage, delegation_stage};
