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

/// Where one delegation stands. The DC share rules behind these stages are
/// in `docs/helper_submission_invariants.md`, "Delegation commitment shares".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyDelegationStage {
    /// Committed, and its batch is not submitted yet.
    Preparing,
    /// Its batch is submitted and has not landed.
    Submitting,
    /// Its batch landed, and some share is not handed off yet.
    Included,
    /// Every share is handed off to the helpers. It stays here after the vote
    /// end, whether or not an immediate share 0's reveal was confirmed.
    HandedOff,
    /// The delegation will not count: voting ended before it was handed off.
    NotCounted,
}

/// A chosen delegate's ballot as published for the round.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateBallotView {
    /// The round is active and no ballot landed.
    NoBallotYet,
    /// The round is active and the ballot landed.
    Submitted {
        /// One option per proposal, in proposal order.
        options: Vec<u32>,
    },
    /// The tally transition counted the delegate's pool into this ballot.
    Counted {
        /// One option per proposal, in proposal order.
        options: Vec<u32>,
    },
    /// No ballot landed before the tally, so the pool was not counted.
    NotCounted,
}

/// One chosen delegate's delegation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyDelegationView {
    /// The delegate's registry index.
    pub delegate_index: u32,
    /// The provider of the delegate's account.
    pub provider: ProxyDelegateProvider,
    /// The delegate's handle, without `@`.
    pub handle: String,
    /// The ballots in its DCs.
    pub ballots: u64,
    /// Where the delegation stands.
    pub stage: ProxyDelegationStage,
    /// The delegate's ballot as published.
    pub ballot: DelegateBallotView,
}

/// The round's committed allocation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyAllocationStatus {
    /// One view per chosen delegate, in request order.
    pub delegations: Vec<ProxyDelegationView>,
    /// The ballots the wallet votes itself.
    pub kept_ballots: u64,
}

/// The delegation job's view of a committed allocation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyDelegationProgress {
    /// One view per chosen delegate, in request order.
    pub delegations: Vec<ProxyDelegationView>,
    /// A batch may have reached the network, so the job can't be cancelled.
    pub broadcast_started: bool,
    /// Some landed DC's share 0 was planned with `submit_at = 0`, so the
    /// job completes only on its confirmed reveal.
    pub planned_immediate: bool,
    /// `planned_immediate`, and every such share 0's reveal is confirmed.
    pub immediate_reveal_confirmed: bool,
    /// The chain refused a sent batch's last attempt. The job keeps sending
    /// it until it lands or voting ends.
    pub refused_by_chain: bool,
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
