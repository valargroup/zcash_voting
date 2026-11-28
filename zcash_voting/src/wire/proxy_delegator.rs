//! Delegator types of the client API (proxy delegation contracts-v1 §15.1),
//! with the contract's names and shapes. Vizor's bridge exposes them to Dart
//! unchanged.

use serde::{Deserialize, Serialize};

use super::ProxyDelegateProvider;

/// Whether the wallet may preview and commit a new allocation in a round.
/// The first result that applies wins, in contract §11.4's order:
/// `Unsupported`, `Paused`, `AlreadyVoted`, `TimingUnavailable`,
/// `VoteEnded`, then `Available`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAvailability {
    /// None of the other results applies.
    Available,
    /// This build, the chain or the round does not offer proxy delegation.
    Unsupported,
    /// Coordinators paused new delegations on the chain.
    Paused,
    /// The wallet's weight in the round may already be spent.
    AlreadyVoted {
        /// The weight was spent from another install.
        used_elsewhere: bool,
    },
    /// The host has no round timing.
    TimingUnavailable,
    /// The round's vote end has passed.
    VoteEnded,
}

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

/// One delegate's whole-ballot quota in a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyPreviewDelegation {
    /// The delegate's registry index.
    pub delegate_index: u32,
    /// The ballots the delegate receives.
    pub ballots: u64,
}

/// The planner's exact split of a request; nothing is recorded. A commit
/// takes the preview the delegator confirmed and requires the same split.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyAllocationPreview {
    /// Each choice's quota, in request order.
    pub delegations: Vec<ProxyPreviewDelegation>,
    /// The ballots the delegator keeps.
    pub kept_ballots: u64,
    /// The delegation commitments the plan creates.
    pub dc_count: u32,
    /// The bundles that carry a DC: one batch each.
    pub bundle_count: u32,
    /// The batches use the one-share last-moment layout.
    pub single_share: bool,
}

/// The failures the delegator's screens tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyDelegationErrorKind {
    /// A chosen delegate's entry could not be proven.
    CannotVerify,
    /// The round's vote end has passed on the host clock.
    VoteEnded,
    /// The wallet holds fewer ballots than delegates chosen.
    NotEnoughVotingPower,
    /// The plan changed since its preview.
    PlanChanged,
    /// A vote may already have spent a bundle's VAN.
    AlreadyVoted,
    /// A bundle's registration failed, so no proxy batch can use it.
    RegistrationUnusable,
    /// The plan keeps ballots, but the kept share's answers are not recorded
    /// for every proposal. Answer them or delegate every ballot.
    KeptShareUnanswered,
    /// Any other failure.
    Other,
}

/// A failed delegator call.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyDelegationError {
    /// What failed.
    pub kind: ProxyDelegationErrorKind,
    /// Diagnostic text, never shown to people for a known kind.
    pub detail: Option<String>,
}
