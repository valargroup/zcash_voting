//! Benchmark checks for the helper submission schedule a Vizor vote creates.
//!
//! This inspects the SDK's durable result rather than reproducing its planning
//! policy. The SDK remains the sole authority for choosing `submit_at`; the
//! benchmark only refuses to call an all-immediate result Vizor-equivalent.

use anyhow::{Context, Result};
use zcash_voting::types::ShareDelegationRecord;

use crate::run_config::{ShareIdentity, ShareScheduleSummary};

/// Summarizes and validates one completed initial helper-delivery plan.
///
/// A faithful normal Vizor run has one immutable immediate designation. Its
/// record is immediate, while every other record carries a passive schedule.
/// The check deliberately uses the designation as identity: `submit_at == 0`
/// alone is not the protocol's immediate-share selection.
pub fn validate_vizor_schedule(
    shares: &[ShareDelegationRecord],
    designated: Option<ShareIdentity>,
    observed_at_seconds: u64,
    tracking_budget_seconds: u64,
) -> Result<ShareScheduleSummary> {
    let designated = designated.context("the round created no designated immediate share")?;
    anyhow::ensure!(
        shares.len() > 1,
        "Vizor schedule fidelity needs passive shares, but the round planned only {} share",
        shares.len()
    );

    let mut designated_found = false;
    let mut submit_at_zero_shares = 0usize;
    let mut passive_submit_at = Vec::new();
    let mut passive_delays = Vec::new();

    for share in shares {
        let is_designated = share.bundle_index == designated.bundle_index
            && share.proposal_id == designated.proposal_id
            && share.share_index == designated.share_index;
        if is_designated {
            designated_found = true;
            anyhow::ensure!(
                share.submit_at == 0,
                "designated immediate share ({}, {}, {}) was scheduled at {}",
                designated.bundle_index,
                designated.proposal_id,
                designated.share_index,
                share.submit_at
            );
        } else {
            anyhow::ensure!(
                share.submit_at != 0,
                "non-designated share ({}, {}, {}) was scheduled for immediate submission",
                share.bundle_index,
                share.proposal_id,
                share.share_index
            );
            passive_submit_at.push(share.submit_at);
            passive_delays.push(share.submit_at.saturating_sub(share.created_at));
        }
        if share.submit_at == 0 {
            submit_at_zero_shares += 1;
        }
    }

    anyhow::ensure!(
        designated_found,
        "designated immediate share ({}, {}, {}) has no durable helper record",
        designated.bundle_index,
        designated.proposal_id,
        designated.share_index
    );
    anyhow::ensure!(
        submit_at_zero_shares == 1,
        "expected exactly one immediate helper submission, found {submit_at_zero_shares}"
    );

    passive_submit_at.sort_unstable();
    passive_delays.sort_unstable();
    let tracking_deadline = observed_at_seconds.saturating_add(tracking_budget_seconds);
    let future_shares = passive_submit_at
        .iter()
        .filter(|submit_at| **submit_at > observed_at_seconds)
        .count();
    anyhow::ensure!(
        future_shares > 0,
        "every helper submission was already due when the benchmark inspected the plan"
    );

    Ok(ShareScheduleSummary {
        observed_at_seconds,
        total_shares: shares.len(),
        designated_immediate_shares: 1,
        submit_at_zero_shares,
        passive_shares: passive_submit_at.len(),
        future_shares,
        due_within_tracking_budget: passive_submit_at
            .iter()
            .filter(|submit_at| **submit_at <= tracking_deadline)
            .count(),
        earliest_submit_at_seconds: passive_submit_at.first().copied(),
        p50_delay_seconds: percentile(&passive_delays, 50),
        p95_delay_seconds: percentile(&passive_delays, 95),
        max_delay_seconds: passive_delays.last().copied().unwrap_or_default(),
    })
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() - 1).saturating_mul(percentile) / 100;
    sorted[index]
}
