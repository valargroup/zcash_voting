//! Delivering a landed DC's shares to the helpers.
//!
//! A DC is a vote commitment for proposal 0, so its shares travel as vote
//! shares with `proposal_id = 0` and `vote_decision = d`. Once its batch
//! lands, a DC is planned once (`plan`), its shares are derived again from
//! the round hotkey on every pass (`payloads`), fanned out and journaled as a
//! vote share's are (`delivery`, `journal`). Once handed off, a share is
//! tracked in the background as a vote share is (`tracking`), which
//! confirms its reveal (`confirmation`) and resends it when no helper
//! reveals it.
//!
//! The planning, secrets, delivery, hand-off and completion rules
//! are specified in `docs/helper_submission_invariants.md`, "Delegation
//! commitment shares".

mod confirmation;
mod delivery;
mod journal;
mod payloads;
mod plan;
mod tracking;

use zcash_vote_delegate::DcShareLayout;

use crate::{share_policy::VOTE_COMMITMENT_SHARE_COUNT, share_tracking::ConfiguredHelperFleet};

pub(crate) use confirmation::confirm_dc_share;
pub(crate) use delivery::{deliver_dc_shares, DcDeliveryRequest};
pub(crate) use journal::landed_dc_shares_on;
#[cfg(test)]
pub(crate) use payloads::{derive_dc_share_payloads, load_landed_dc};
#[cfg(test)]
pub(crate) use plan::plan_dc_shares;
pub(crate) use tracking::tracked_dc_shares;

/// One planned share of a landed DC and its helper delivery state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DcShareRecord {
    /// The share's position in the DC.
    pub(crate) share_index: u32,
    /// The share's reveal nullifier, which helpers report status under.
    pub(crate) nullifier: [u8; 32],
    /// The definite acceptances the plan aims for.
    pub(crate) target_count: u32,
    /// When helpers reveal the share, or 0 for at once.
    pub(crate) submit_at: u64,
    /// Helpers that definitely accepted the share.
    pub(crate) sent_to_urls: Vec<String>,
    /// Helpers whose acceptance is unknown.
    pub(crate) ambiguous_urls: Vec<String>,
    /// Helpers reserved before a POST whose outcome is not yet recorded.
    pub(crate) attempting_urls: Vec<String>,
    /// The configured helper quorum observed the share's reveal.
    pub(crate) confirmed: bool,
    /// A delivery pass found the share held by its reachable target of
    /// configured helpers (see `delivery::record_hand_offs`).
    pub(crate) handed_off_to_reachable_target: bool,
    /// When the share was planned.
    pub(crate) created_at: u64,
}

impl DcShareRecord {
    /// Whether the share is handed off: its reveal is confirmed, or a pass
    /// recorded its reachable target holding it. `sent_to_urls` alone never
    /// counts, since it may name helpers no longer configured.
    pub(crate) fn handed_off(&self) -> bool {
        self.confirmed || self.handed_off_to_reachable_target
    }

    /// Whether some helper of `configured` may hold the share without a
    /// definite answer, so a later pass re-posts it. A helper that left the
    /// fleet is never posted to again, so its unknown outcome doesn't count.
    pub(crate) fn outcome_unknown(&self, configured: &ConfiguredHelperFleet) -> bool {
        self.ambiguous_urls
            .iter()
            .chain(&self.attempting_urls)
            .any(|url| configured.contains(url))
    }
}

/// How far one landed DC's delivery has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DcDeliveryProgress {
    /// No share of the DC is planned yet.
    Unplanned,
    /// Some planned share is not yet handed off.
    Delivering,
    /// Every share is handed off, and background tracking owns the ones not
    /// yet confirmed.
    Complete,
}

/// One landed DC of a round, with its planned shares once it has a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LandedDcShares {
    /// The bundle whose batch carried the DC.
    pub(crate) bundle_index: u32,
    /// The DC's slot in that batch.
    pub(crate) dc_slot: u8,
    /// The share layout the DC's slot committed.
    pub(crate) layout: DcShareLayout,
    /// The DC's leaf in the round's commitment tree.
    pub(crate) tree_position: u64,
    /// The planned shares in share order; `None` before the DC is planned.
    pub(crate) shares: Option<Vec<DcShareRecord>>,
}

impl LandedDcShares {
    /// The share indexes the DC's layout delivers.
    pub(crate) fn delivered_share_indexes(&self) -> Vec<u32> {
        delivered_share_indexes(self.layout)
    }

    /// Share 0 when it was planned with `submit_at = 0`.
    pub(crate) fn immediate_share(&self) -> Option<&DcShareRecord> {
        self.shares
            .as_ref()?
            .iter()
            .find(|share| share.share_index == 0 && share.submit_at == 0)
    }

    /// How far the DC's delivery has come: it completes at hand-off.
    pub(crate) fn progress(&self) -> DcDeliveryProgress {
        match &self.shares {
            None => DcDeliveryProgress::Unplanned,
            Some(shares) if shares.iter().all(DcShareRecord::handed_off) => {
                DcDeliveryProgress::Complete
            }
            Some(_) => DcDeliveryProgress::Delivering,
        }
    }

    /// The shares background tracking owns: handed off, not yet confirmed.
    pub(crate) fn tracked_shares(&self) -> impl Iterator<Item = &DcShareRecord> {
        self.shares
            .iter()
            .flatten()
            .filter(|share| share.handed_off() && !share.confirmed)
    }
}

/// The share indexes `layout` delivers: share 0 alone for a single-share
/// DC, all 16 otherwise.
pub(crate) fn delivered_share_indexes(layout: DcShareLayout) -> Vec<u32> {
    match layout {
        DcShareLayout::SingleShare => vec![0],
        DcShareLayout::Standard => (0..VOTE_COMMITMENT_SHARE_COUNT as u32).collect(),
    }
}
