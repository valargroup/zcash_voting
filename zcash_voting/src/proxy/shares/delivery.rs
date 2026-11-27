//! One delivery pass over a landed DC's shares: derive them, plan them once,
//! then fan every share that is not handed off out to its helpers.

use std::time::Duration;

use futures_util::future::join_all;
use tokio::time::Instant;

use super::{
    journal::{load_dc_shares, record_dc_share_hand_offs, DcShareJournal},
    payloads::{derive_dc_share_payloads, load_landed_dc, LandedDc},
    plan::{check_nullifiers, load_dc_share_plan, prepare_dc_share_plan, DcSharePlanning},
    DcDeliveryProgress, DcShareRecord, LandedDcShares,
};
use crate::{
    helper::{client::HelperClient, url::canonical_helper_url_list},
    share::{ShareDeliveryAttemptOutcome, ShareOperationScope},
    share_policy::{effective_share_submission_target_count, ShareSubmissionPlan},
    share_tracking::{
        dispatch_share_to_canonical_helpers, lock_share_operation_or_cancel, ConfiguredHelperFleet,
        ShareAttemptJournal, ShareFanOut,
    },
    storage::VotingDb,
    types::{SharePayload, VotingError},
};

/// What one delivery pass over a landed DC needs.
#[derive(Clone, Copy)]
pub(crate) struct DcDeliveryRequest<'a> {
    pub(crate) round_id: &'a str,
    pub(crate) bundle_index: u32,
    pub(crate) dc_slot: u8,
    /// The round hotkey's stored secret, which derives the shares.
    pub(crate) hotkey_seed: &'a [u8],
    /// The complete helper fleet from authenticated configuration.
    pub(crate) configured_server_urls: &'a [String],
    /// The host clock, a whole-second sample taken before the pass.
    pub(crate) now_seconds: u64,
    /// The device's wall clock as the pass starts, since the Unix epoch.
    pub(crate) wall_clock: Duration,
    /// The authenticated vote end. A first plan without it uses
    /// `now_seconds`.
    pub(crate) vote_end_time_seconds: Option<u64>,
    /// The round's last-moment buffer, when its timing is authenticated.
    pub(crate) last_moment_buffer_seconds: Option<u64>,
}

/// What one delivery pass left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DcDeliveryPass {
    /// How far the DC's delivery has come.
    pub(crate) progress: DcDeliveryProgress,
    /// A short share has a configured helper whose acceptance a later pass
    /// reconciles.
    pub(crate) outcome_unknown: bool,
    /// The host's cancellation stopped the pass.
    pub(crate) cancelled: bool,
    /// The clock reached the authenticated vote end during the pass, so no
    /// new POST started after it.
    pub(crate) vote_ended: bool,
}

/// Runs one delivery pass over the landed DC in `request.bundle_index`'s
/// slot `request.dc_slot`.
///
/// Derives the payloads from the hotkey, plans the DC on its first pass
/// after probing the fleet, then delivers every share not handed off
/// ([`deliver_share`]) and records the hand-offs ([`record_hand_offs`]).
/// Shares are
/// delivered concurrently, and the first error is returned once all have
/// finished, so the others' effects are kept. Cancellation, or the clock
/// reaching the vote end, stops new POSTs; one in flight keeps its outcome.
pub(crate) async fn deliver_dc_shares(
    db: &VotingDb,
    client: &HelperClient,
    request: DcDeliveryRequest<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<DcDeliveryPass, VotingError> {
    // Tokio's monotonic clock, so a paused test clock drives it.
    let started_at = Instant::now();
    let time_left = request
        .vote_end_time_seconds
        .map(|vote_end| time_left_to_vote_end(vote_end, request.now_seconds, request.wall_clock));
    let vote_ended = || time_left.is_some_and(|time_left| started_at.elapsed() >= time_left);
    let stop = || cancel() || vote_ended();
    let pass_end = || PassEnd {
        cancelled: cancel(),
        vote_ended: vote_ended(),
    };
    let scope = ShareOperationScope::capture(db);
    let configured = ConfiguredHelperFleet::new(request.configured_server_urls)?;
    let dc = db.read_transaction("load landed proxy DC", |tx| {
        load_landed_dc(
            tx,
            request.round_id,
            scope.wallet_id(),
            request.bundle_index,
            request.dc_slot,
        )
    })?;
    let derived = derive_dc_share_payloads(&dc, request.hotkey_seed)?;
    let stored = match load_dc_share_plan(db, &scope, request.round_id, &dc)? {
        Some(stored) => {
            check_nullifiers(&stored, &dc, &derived.nullifiers)?;
            stored
        }
        None => {
            let fleet = client.preflight_fleet(configured.urls()).await?;
            // The probe may have waited on unreachable helpers; plan nothing
            // for a step the host has since left or a vote that has ended.
            if stop() {
                return current_pass(db, &scope, request, &dc, &configured, pass_end());
            }
            prepare_dc_share_plan(
                db,
                &scope,
                request.round_id,
                &dc,
                &derived.nullifiers,
                DcSharePlanning {
                    fleet: &fleet,
                    now_seconds: request.now_seconds,
                    vote_end_time_seconds: request
                        .vote_end_time_seconds
                        .unwrap_or(request.now_seconds),
                    last_moment_buffer_seconds: request.last_moment_buffer_seconds,
                },
            )?
        }
    };

    let deliveries = stored
        .shares
        .iter()
        .zip(&stored.plans)
        .zip(&derived.payloads)
        .filter(|((share, _), _)| !share.handed_off())
        .map(|((share, plan), payload)| {
            deliver_share(
                db,
                &scope,
                client,
                ShareDelivery {
                    round_id: request.round_id,
                    dc: &dc,
                    share,
                    plan,
                    payload,
                    configured: &configured,
                    now_seconds: request.now_seconds,
                },
                &stop,
            )
        });
    for delivered in join_all(deliveries).await {
        delivered?;
    }
    record_hand_offs(db, &scope, request, &dc, &configured)?;
    current_pass(db, &scope, request, &dc, &configured, pass_end())
}

/// How long after the pass starts it may still start a POST for a vote
/// ending at `vote_end` seconds. Whichever clock says the vote ends first
/// decides: the host's `host_now`, a whole-second sample that may trail its
/// clock by up to a second, so it is read as late as it may be, or the
/// device's `wall_clock`, read as the pass starts, which also counts any
/// time between the host's sample and the pass.
fn time_left_to_vote_end(vote_end: u64, host_now: u64, wall_clock: Duration) -> Duration {
    let vote_end = Duration::from_secs(vote_end);
    let host_latest = Duration::from_secs(host_now).saturating_add(Duration::from_millis(999));
    vote_end
        .saturating_sub(host_latest)
        .min(vote_end.saturating_sub(wall_clock))
}

/// What a pass observed as it ended.
#[derive(Clone, Copy)]
struct PassEnd {
    cancelled: bool,
    vote_ended: bool,
}

/// One share's delivery inputs.
#[derive(Clone, Copy)]
struct ShareDelivery<'a> {
    round_id: &'a str,
    dc: &'a LandedDc,
    share: &'a DcShareRecord,
    plan: &'a ShareSubmissionPlan,
    payload: &'a SharePayload,
    configured: &'a ConfiguredHelperFleet,
    now_seconds: u64,
}

/// Fans one share out to the configured helpers, planned helpers first,
/// then reconciles it when it still falls short.
async fn deliver_share(
    db: &VotingDb,
    scope: &ShareOperationScope,
    client: &HelperClient,
    delivery: ShareDelivery<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(), VotingError> {
    let ShareDelivery {
        round_id,
        dc,
        share,
        plan,
        payload,
        configured,
        now_seconds,
    } = delivery;
    let share_wire_json = payload.to_wire_json(Some(dc.tree_position), share.submit_at)?;
    let planned: Vec<String> = canonical_helper_url_list(&plan.target_servers)?
        .into_iter()
        .filter(|url| configured.contains(url))
        .collect();
    let fallback: Vec<String> = configured
        .urls()
        .iter()
        .filter(|url| !planned.contains(url))
        .cloned()
        .collect();
    let target_count =
        effective_share_submission_target_count(share.target_count, configured.len());
    let journal = DcShareJournal {
        db,
        scope,
        round_id,
        bundle_index: dc.bundle_index,
        dc_slot: dc.dc_slot,
        share_index: share.share_index,
        nullifier: share.nullifier,
    };
    let report = dispatch_share_to_canonical_helpers(
        &journal,
        scope,
        client,
        ShareFanOut {
            round_id,
            share_wire_json: &share_wire_json,
            target_count,
            now_seconds,
            planned: &planned,
            fallback: &fallback,
        },
        cancel,
    )
    .await?;
    if report.accepted_urls.len() >= target_count || cancel() {
        return Ok(());
    }
    reconcile_share(
        &journal,
        scope,
        client,
        Reconciliation {
            round_id,
            share_wire_json: &share_wire_json,
            target_count,
            earlier: share,
            configured,
            now_seconds,
        },
        cancel,
    )
    .await
}

/// One share's reconciliation inputs.
#[derive(Clone, Copy)]
struct Reconciliation<'a> {
    round_id: &'a str,
    share_wire_json: &'a str,
    /// The share's effective target under the configured fleet.
    target_count: usize,
    /// The share as the pass began.
    earlier: &'a DcShareRecord,
    configured: &'a ConfiguredHelperFleet,
    now_seconds: u64,
}

/// Re-posts one share, duplicate-safe and once each, to the configured
/// helpers whose attempt had an unknown outcome when the pass
/// began, interrupted ones first, until `target_count` configured helpers
/// hold it.
///
/// An interrupted helper that does not accept becomes explicitly ambiguous.
/// An ambiguous one is unchanged, since a failed re-post says nothing about
/// the original. The share is read again before every POST, so a confirmed
/// or replaced share is not posted again.
async fn reconcile_share(
    journal: &DcShareJournal<'_>,
    scope: &ShareOperationScope,
    client: &HelperClient,
    reconciliation: Reconciliation<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(), VotingError> {
    let Reconciliation {
        round_id,
        share_wire_json,
        target_count,
        earlier,
        configured,
        now_seconds,
    } = reconciliation;
    let configured_only = |urls: &[String]| {
        urls.iter()
            .filter(|url| configured.contains(url))
            .cloned()
            .collect::<Vec<_>>()
    };
    let interrupted = configured_only(&earlier.attempting_urls);
    let ambiguous = configured_only(&earlier.ambiguous_urls);
    if interrupted.is_empty() && ambiguous.is_empty() {
        return Ok(());
    }
    let Some(_operation_guard) = lock_share_operation_or_cancel(
        scope,
        round_id,
        journal.share(),
        cancel,
        &client.observation_scope(),
    )
    .await?
    else {
        return Ok(());
    };
    let stale = || VotingError::InvalidInput {
        message: format!(
            "a DC share changed while helper delivery was in flight for round={round_id}, \
             bundle={}, dc_slot={}, share={}",
            journal.bundle_index, journal.dc_slot, journal.share_index
        ),
    };
    for server_url in interrupted.iter().chain(&ambiguous) {
        if cancel() {
            break;
        }
        let Some(current) = journal.record()? else {
            return Err(stale());
        };
        if current.confirmed || configured_only(&current.sent_to_urls).len() >= target_count {
            break;
        }
        if current.sent_to_urls.contains(server_url) {
            continue;
        }
        let outcome = match client
            .resubmit_share(server_url, share_wire_json, now_seconds, cancel)
            .await
        {
            Ok(_) => ShareDeliveryAttemptOutcome::Accepted,
            Err(_) if interrupted.contains(server_url) => ShareDeliveryAttemptOutcome::Ambiguous,
            Err(_) => continue,
        };
        if !journal.resolve_attempt(server_url, outcome, false)? {
            return Err(stale());
        }
    }
    Ok(())
}

/// Records each share of `dc` not yet handed off as handed off once its
/// reachable target of configured helpers holds it:
/// `effective_share_submission_target_count` of its stored target and the
/// fleet's size. This is the only hand-off short of a confirmed reveal, so a
/// share on its full target is recorded too, since round planning holds no
/// fleet to judge it by. Helpers removed from the fleet do not count.
fn record_hand_offs(
    db: &VotingDb,
    scope: &ShareOperationScope,
    request: DcDeliveryRequest<'_>,
    dc: &LandedDc,
    configured: &ConfiguredHelperFleet,
) -> Result<(), VotingError> {
    let shares = db.read_transaction("load DC shares", |tx| {
        load_dc_shares(
            tx,
            request.round_id,
            scope.wallet_id(),
            dc.bundle_index,
            dc.dc_slot,
        )
    })?;
    let hand_offs: Vec<(u32, [u8; 32])> = shares
        .unwrap_or_default()
        .iter()
        .filter(|share| !share.handed_off())
        .filter(|share| {
            let configured_holders = share
                .sent_to_urls
                .iter()
                .filter(|url| configured.contains(url))
                .count();
            let reachable_target =
                effective_share_submission_target_count(share.target_count, configured.len())
                    .max(1);
            configured_holders >= reachable_target
        })
        .map(|share| (share.share_index, share.nullifier))
        .collect();
    if hand_offs.is_empty() {
        return Ok(());
    }
    record_dc_share_hand_offs(
        db,
        scope,
        request.round_id,
        dc.bundle_index,
        dc.dc_slot,
        &hand_offs,
        request.now_seconds,
    )
}

/// The DC's delivery as durable state now describes it, plus what the pass
/// observed.
fn current_pass(
    db: &VotingDb,
    scope: &ShareOperationScope,
    request: DcDeliveryRequest<'_>,
    dc: &LandedDc,
    configured: &ConfiguredHelperFleet,
    end: PassEnd,
) -> Result<DcDeliveryPass, VotingError> {
    let shares = db.read_transaction("load DC shares", |tx| {
        load_dc_shares(
            tx,
            request.round_id,
            scope.wallet_id(),
            dc.bundle_index,
            dc.dc_slot,
        )
    })?;
    let outcome_unknown = shares
        .iter()
        .flatten()
        .filter(|share| !share.handed_off())
        .any(|share| share.outcome_unknown(configured));
    let landed = LandedDcShares {
        bundle_index: dc.bundle_index,
        dc_slot: dc.dc_slot,
        layout: dc.layout,
        tree_position: dc.tree_position,
        shares,
    };
    Ok(DcDeliveryPass {
        progress: landed.progress(),
        outcome_unknown,
        cancelled: end.cancelled,
        vote_ended: end.vote_ended,
    })
}
