//! The proxy share planner: one landed DC's helper plan, keyed by `(round,
//! wallet, bundle, dc_slot)` and made once, before its first share POST.

use zcash_vote_delegate::DcShareLayout;

use super::{
    journal::{insert_dc_share_plan, load_dc_share_plans, load_dc_shares},
    payloads::LandedDc,
    DcShareRecord,
};
use crate::{
    helper::client::HelperFleetPreflight,
    share::ShareOperationScope,
    share_policy::{ShareSubmissionPlan, VOTE_COMMITMENT_SHARE_COUNT},
    share_tracking::plan_shares_with_os_entropy,
    storage::VotingDb,
    types::VotingError,
};

/// Plans the helpers and `submit_at` of one DC's shares under its
/// committed `layout`, never inferred from a payload count.
///
/// The single-share layout plans share 0 alone with `submit_at = 0`. The
/// standard layout plans all 16 shares on the standard schedule. Each share
/// goes to as many helpers as a vote share (`share_submission_target_count`),
/// and none is the round's immediate share (PRV-3). Entropy comes from the
/// operating system.
pub(crate) fn plan_dc_shares(
    layout: DcShareLayout,
    fleet: &HelperFleetPreflight,
    now_seconds: u64,
    vote_end_time_seconds: u64,
    last_moment_buffer_seconds: Option<u64>,
) -> Result<Vec<ShareSubmissionPlan>, VotingError> {
    let single_share = layout == DcShareLayout::SingleShare;
    let share_count = if single_share {
        1
    } else {
        VOTE_COMMITMENT_SHARE_COUNT
    };
    plan_shares_with_os_entropy(
        share_count,
        fleet,
        now_seconds,
        vote_end_time_seconds,
        last_moment_buffer_seconds,
        single_share,
        None,
    )
}

/// A DC's durable plan and share rows, in share order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StoredDcSharePlan {
    /// One plan per delivered share.
    pub(super) plans: Vec<ShareSubmissionPlan>,
    /// One row per delivered share.
    pub(super) shares: Vec<DcShareRecord>,
}

/// Returns the DC's stored plan, first making and storing one against the
/// probed `fleet` when it has none. `vote_end_time_seconds` is the
/// authenticated vote end, or `now_seconds` without one, and
/// `last_moment_buffer_seconds` the round's buffer when its timing is
/// authenticated.
///
/// In one immediate transaction a plan another writer stored first wins.
/// Otherwise the plan and a row per share, carrying `nullifiers`, the reveal
/// nullifiers in share order, are written together. The caller checks the
/// result with [`check_nullifiers`].
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_dc_share_plan(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: &str,
    dc: &LandedDc,
    nullifiers: &[[u8; 32]],
    fleet: &HelperFleetPreflight,
    now_seconds: u64,
    vote_end_time_seconds: u64,
    last_moment_buffer_seconds: Option<u64>,
) -> Result<StoredDcSharePlan, VotingError> {
    let plans = plan_dc_shares(
        dc.layout,
        fleet,
        now_seconds,
        vote_end_time_seconds,
        last_moment_buffer_seconds,
    )?;
    let shares: Vec<(u32, [u8; 32])> = super::delivered_share_indexes(dc.layout)
        .into_iter()
        .zip(nullifiers.iter().copied())
        .collect();
    db.write_transaction("plan DC shares", |tx| {
        if let Some(stored) = stored_plan(tx, round_id, scope.wallet_id(), dc)? {
            return Ok(stored);
        }
        insert_dc_share_plan(
            tx,
            round_id,
            scope.wallet_id(),
            dc.bundle_index,
            dc.dc_slot,
            fleet.configured_server_urls(),
            &plans,
            &shares,
            now_seconds,
        )?;
        stored_plan(tx, round_id, scope.wallet_id(), dc)?.ok_or_else(|| VotingError::Internal {
            message: "a newly stored DC share plan was not found".to_string(),
        })
    })
}

/// Refuses a stored plan whose rows do not carry `nullifiers`, the shares
/// this hotkey derives for `dc`.
pub(super) fn check_nullifiers(
    stored: &StoredDcSharePlan,
    dc: &LandedDc,
    nullifiers: &[[u8; 32]],
) -> Result<(), VotingError> {
    let stored_nullifiers: Vec<[u8; 32]> =
        stored.shares.iter().map(|share| share.nullifier).collect();
    if stored_nullifiers != nullifiers {
        return Err(VotingError::Internal {
            message: format!(
                "the stored shares of the DC in slot {} of bundle {} are not the shares the round \
                 hotkey derives",
                dc.dc_slot, dc.bundle_index
            ),
        });
    }
    Ok(())
}

/// The DC's plan and rows on `conn`, checked to describe one plan.
pub(super) fn stored_plan(
    conn: &rusqlite::Connection,
    round_id: &str,
    wallet_id: &str,
    dc: &LandedDc,
) -> Result<Option<StoredDcSharePlan>, VotingError> {
    let Some(plans) = load_dc_share_plans(conn, round_id, wallet_id, dc.bundle_index, dc.dc_slot)?
    else {
        return Ok(None);
    };
    let shares =
        load_dc_shares(conn, round_id, wallet_id, dc.bundle_index, dc.dc_slot)?.unwrap_or_default();
    let indexes: Vec<u32> = shares.iter().map(|share| share.share_index).collect();
    if plans.len() != shares.len() || indexes != super::delivered_share_indexes(dc.layout) {
        return Err(VotingError::Internal {
            message: format!(
                "the stored share plan of the DC in slot {} of bundle {} does not cover its layout",
                dc.dc_slot, dc.bundle_index
            ),
        });
    }
    Ok(Some(StoredDcSharePlan { plans, shares }))
}
