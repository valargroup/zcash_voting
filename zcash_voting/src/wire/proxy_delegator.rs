//! Delegator types of the client API (proxy delegation contracts-v1 §15.1),
//! with the contract's names and shapes. Vizor's bridge exposes them to Dart
//! unchanged.

use serde::{Deserialize, Serialize};

use super::ProxyDelegateProvider;

/// One delegate the delegator chose and its share of the round's weight.
///
/// `provider`, `handle` and `dk` are the listing's the delegator picked. An
/// entry keeps all three for life, so the pre-proving check requires the
/// proven entry at `delegate_index` to hold exactly them, and a lying or
/// stale registry cache cannot redirect the delegation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyDelegateChoice {
    /// The delegate's registry index, at least 1.
    pub delegate_index: u32,
    /// The delegate's share of the round's weight, in basis points.
    pub percent_bps: u16,
    /// The provider of the delegate's account.
    pub provider: ProxyDelegateProvider,
    /// The delegate's canonical handle, without `@`.
    pub handle: String,
    /// The delegate's key, `zvdk1…`.
    pub dk: String,
}

/// A delegator's request for one round. The delegator votes the weight the
/// choices leave unnamed with ordinary casts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyAllocationRequest {
    /// The round, as 64 lowercase hex characters.
    pub round_id: String,
    /// 1 to 8 choices totalling at most 10,000 basis points, in the order
    /// the delegator listed them.
    pub choices: Vec<ProxyDelegateChoice>,
}
