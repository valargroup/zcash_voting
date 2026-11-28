//! The delegator's client API calls (contracts-v1 §15.1), taking and
//! returning the [`crate::wire`] types Vizor's bridge exposes to Dart.
//!
//! In round order: [`availability`] (before [`preview`] and again just before
//! [`commit`]), [`preview`], then [`commit`].
//! Every call fails with a [`ProxyDelegationError`](crate::wire::ProxyDelegationError).

mod availability;

pub use super::planner::{commit, preview};
pub use availability::{
    availability, GovNullifierProof, ProxyAvailabilityInputs, CIRCUIT_FINGERPRINTS,
    PROXY_DELEGATION_CHAIN_VERSION, PROXY_DELEGATION_WALLET_VERSION,
};

#[cfg(test)]
pub(crate) use availability::evaluate_availability;
