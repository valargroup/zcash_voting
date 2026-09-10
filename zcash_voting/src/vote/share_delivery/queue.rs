//! A single stream refills share slots across commitment boundaries.

use super::{
    capacity,
    immediate_gate::{GateWait, ImmediateGate},
    preparation::{self, PreparedVoteDelivery},
    reports::{ProposalDelivery, ShareResult},
    VoteDeliveryResult,
};
use crate::{
    helper::client::HelperClient,
    round::VotingDb,
    share::ShareOperationScope,
    share_tracking::{
        CommittedShareSubmissionRequest, ShareBatchDeliveryReport, ShareDeliveryOutcome,
        ShareDeliverySubmissionParams,
    },
    vote::CommittedVote,
};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::sync::Arc;

/// Lightweight queue entry; payloads and immutable plans are shared per vote.
struct ShareJob<'a> {
    proposal_position: usize,
    payload_position: usize,
    prepared: Arc<PreparedVoteDelivery<'a>>,
    observations: crate::ObservationScope,
    queued: crate::observability::ObservationStage,
}

/// Attribution retained when a share finishes ahead of earlier queue entries.
struct ShareJobCompletion {
    proposal_position: usize,
    payload_position: usize,
    delivery: ShareResult,
}

impl ShareJob<'_> {
    async fn deliver(
        self,
        db: &VotingDb,
        client: &HelperClient,
        scope: &ShareOperationScope,
        params: &ShareDeliverySubmissionParams<'_>,
        cancel: &(dyn Fn() -> bool + Send + Sync),
    ) -> ShareResult {
        let planned_target = self.prepared.plan.share_plans[self.payload_position].target_count;
        let admission = capacity::acquire(planned_target, cancel).await;
        self.queued.finish(
            match &admission {
                Ok(Some(_)) => crate::ObservationOutcome::Succeeded,
                Ok(None) => crate::ObservationOutcome::Cancelled,
                Err(_) => crate::ObservationOutcome::Failed,
            },
            admission
                .as_ref()
                .err()
                .map(crate::observability::voting_error_kind),
        );
        let Some(_permit) = admission? else {
            return Ok(None);
        };
        let active = self.observations.stage("helper::active_delivery");
        let observed_client = client.observing(active.scope());
        let client = &observed_client;
        let vote = self.prepared.vote;
        let share_index = vote.commit.share_payloads[self.payload_position]
            .enc_share
            .share_index;
        let submission = vote
            .submit_share_to_helpers_for_generation(
                db,
                client,
                CommittedShareSubmissionRequest {
                    share_index,
                    plan: &self.prepared.plan.share_plans[self.payload_position],
                    planning_server_urls: &self.prepared.plan.configured_server_urls,
                    configured_server_urls: params.configured_server_urls,
                    now_seconds: params.now_seconds,
                },
                &self.prepared.generation,
                scope,
                cancel,
            )
            .await;
        let outcome = match &submission {
            Err(_) => crate::ObservationOutcome::Failed,
            Ok(report) if !report.accepted_urls.is_empty() => crate::ObservationOutcome::Succeeded,
            Ok(report) if !report.ambiguous_urls.is_empty() => crate::ObservationOutcome::Pending,
            Ok(_) => crate::ObservationOutcome::Failed,
        };
        active.finish(
            outcome,
            submission
                .as_ref()
                .err()
                .map(crate::observability::voting_error_kind),
        );
        submission.map(|submission| {
            Some(ShareDeliveryOutcome {
                share_index,
                submission,
            })
        })
    }
}

/// Shared mechanism for the confirmed multi-vote boundary and the historical
/// singleton wrapper. Preparation revalidates durable confirmation even for
/// callers that already hold a ConfirmedVote.
pub(in crate::vote) async fn submit_votes<'a>(
    votes: impl IntoIterator<Item = &'a CommittedVote>,
    db: &VotingDb,
    client: &HelperClient,
    params: ShareDeliverySubmissionParams<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    on_report: &mut (dyn FnMut(&CommittedVote, &ShareBatchDeliveryReport) + Send),
) -> Vec<VoteDeliveryResult<'a>> {
    let scope = ShareOperationScope::capture(db);
    let mut proposals = votes
        .into_iter()
        .map(|vote| ProposalDelivery::new(vote, preparation::prepare(vote, db, &scope, &params)))
        .collect::<Vec<_>>();
    let jobs = proposals
        .iter()
        .enumerate()
        .flat_map(|(proposal_position, proposal)| {
            proposal.prepared.iter().flat_map(move |prepared| {
                (0..prepared.plan.share_plans.len()).map(move |payload_position| {
                    let observations =
                        client
                            .observation_scope()
                            .attributed(crate::ObservationAttribution {
                                bundle_index: Some(prepared.vote.bundle_index()),
                                proposal_id: Some(prepared.vote.proposal_id()),
                                share_index: Some(
                                    prepared.vote.commit.share_payloads[payload_position]
                                        .enc_share
                                        .share_index,
                                ),
                            });
                    let queued = observations.stage("helper::delivery_queue_wait");
                    ShareJob {
                        proposal_position,
                        payload_position,
                        prepared: Arc::clone(prepared),
                        observations,
                        queued,
                    }
                })
            })
        })
        .collect::<Vec<_>>();
    // Empty plans are normally rejected by validation, but keep accounting
    // total without requiring a stream item to finalize an empty proposal.
    for proposal in &mut proposals {
        let vote = proposal.vote;
        if let Some(report) = proposal.finish(cancel()) {
            on_report(vote, report);
        }
    }
    // The designated immediate share is dispatched before the round's other
    // shares and opens the round's gate once it has reached a helper. Bundles
    // that confirmed earlier and are delivering concurrently wait on that gate.
    // See `immediate_gate` for why the barrier spans calls, why every wait is
    // bounded, and why it stops at ordering inside one call.
    let mut jobs = jobs.into_iter().collect::<Vec<_>>();
    let round_id = proposals
        .first()
        .map(|proposal| proposal.vote.round_id().to_string());
    // Read here, beside the plan loads this call has already done, rather than
    // inside the gate. It answers whether an earlier pass or a run before a
    // restart already placed the share, which cannot change while this call
    // waits, and it is an ordinary synchronous storage read like every other
    // one on this path. `WAIT_BUDGET` bounds the wait; it does not and cannot
    // bound a blocking connection acquisition, which no timer can preempt.
    let already_accepted = designated_share_accepted(db, &scope, round_id.as_deref());
    let gate = round_gate(db, &scope, round_id.as_deref());
    let mut designated = None;
    if let Some(gate) = &gate {
        // Holder identity comes from the durable designation against this
        // call's votes, not from the job list. A designated proposal whose
        // preparation failed contributes no job, and treating that as "someone
        // else holds it" would make this call's own siblings wait out the budget
        // for a share that is already resolved and in front of them.
        match designated_position(db, &scope, round_id.as_deref(), &proposals, &jobs) {
            Holder::Dispatching(position) => {
                let job = jobs.remove(position);
                designated = Some((job.proposal_position, job.payload_position));
                jobs.insert(0, job);
            }
            // Held here, but it will never be dispatched. Release at once: a
            // share this call cannot send will not arrive by being waited for.
            Holder::Undeliverable => gate.open(),
            // A share accepted *now* is accepted by a sibling call in this
            // process, which signals through the gate rather than the row.
            Holder::Elsewhere if !already_accepted => {
                let waited = client
                    .observation_scope()
                    .stage("helper::immediate_gate_wait");
                let outcome = gate.wait(cancel).await;
                waited.finish(gate_outcome(outcome), None);
            }
            Holder::Elsewhere => {}
        }
    }

    let mut jobs = jobs.into_iter();
    let mut deliveries = FuturesUnordered::new();
    loop {
        while deliveries.len() < capacity::MAX_CONCURRENT_SHARE_DELIVERIES {
            let Some(job) = jobs.next() else {
                break;
            };
            deliveries.push(run_job(job, db, client, &scope, &params, cancel));
        }
        // A hard error must not drop live sibling POSTs or leave independent
        // proposals unsent. Cancelled jobs skip admission without touching storage.
        let Some(completion) = deliveries.next().await else {
            break;
        };
        // Opened on every outcome, not only acceptance. A share the helpers
        // refused will not arrive by being waited for, and the rest of the
        // round must not spend its budget discovering that.
        if designated == Some((completion.proposal_position, completion.payload_position)) {
            if let Some(gate) = &gate {
                gate.open();
            }
        }
        record_completion(&mut proposals, completion, cancel, on_report);
    }
    // A call that held the designation but never dispatched it — cancelled
    // before admission, or drained with the job unrun — must still release the
    // round rather than leave concurrent bundles waiting out their budget.
    if let (Some(gate), Some(_)) = (&gate, designated) {
        gate.open();
    }
    proposals
        .into_iter()
        .map(ProposalDelivery::into_result)
        .collect()
}

async fn run_job(
    job: ShareJob<'_>,
    db: &VotingDb,
    client: &HelperClient,
    scope: &ShareOperationScope,
    params: &ShareDeliverySubmissionParams<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> ShareJobCompletion {
    let proposal_position = job.proposal_position;
    let payload_position = job.payload_position;
    let delivery = job.deliver(db, client, scope, params, cancel).await;
    ShareJobCompletion {
        proposal_position,
        payload_position,
        delivery,
    }
}

/// The round's barrier, or `None` when there is nothing to order.
fn round_gate(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: Option<&str>,
) -> Option<Arc<ImmediateGate>> {
    Some(ImmediateGate::for_round(
        db.sidecar_id(),
        scope.wallet_id(),
        round_id?,
    ))
}

/// This call's relationship to the round's designated share.
enum Holder {
    /// Held here, with a job to dispatch. The position is into `jobs`.
    Dispatching(usize),
    /// Held here, but no job will carry it — its preparation failed, so it has
    /// already reached a terminal non-acceptance in this call.
    Undeliverable,
    /// Not this call's to send.
    Elsewhere,
}

/// Where the round's designated share sits relative to this call.
///
/// Resolved against the durable designation and this call's votes rather than
/// against the job list alone: a designated proposal whose preparation failed
/// contributes no job, and it must still be recognised as held here so the gate
/// is released instead of leaving this call's own siblings waiting.
fn designated_position(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: Option<&str>,
    proposals: &[ProposalDelivery<'_>],
    jobs: &[ShareJob<'_>],
) -> Holder {
    if let Some(position) = jobs
        .iter()
        .position(|job| job.prepared.plan.share_plans[job.payload_position].immediate)
    {
        return Holder::Dispatching(position);
    }
    let Some(round_id) = round_id else {
        return Holder::Elsewhere;
    };
    let designation = {
        let conn = db.conn();
        crate::share_tracking::round_immediate_share(&conn, round_id, scope.wallet_id())
    };
    let Ok(Some(key)) = designation else {
        return Holder::Elsewhere;
    };
    if proposals.iter().any(|proposal| {
        proposal.vote.bundle_index() == key.bundle_index
            && proposal.vote.proposal_id() == key.proposal_id
    }) {
        return Holder::Undeliverable;
    }
    Holder::Elsewhere
}

/// Names a gate outcome for observability.
///
/// An expired wait is `Pending`, not `Failed`: the round proceeded exactly as
/// specified, and what the record exists to say is that the delay was the gate
/// rather than delivery capacity.
fn gate_outcome(outcome: GateWait) -> crate::ObservationOutcome {
    match outcome {
        GateWait::AlreadyAccepted | GateWait::Accepted => crate::ObservationOutcome::Succeeded,
        GateWait::Expired => crate::ObservationOutcome::Pending,
        GateWait::Cancelled => crate::ObservationOutcome::Cancelled,
    }
}

/// Whether the round's designated share already has a definite acceptance.
///
/// The durable row is what a waiting delivery actually needs to know, and
/// reading it directly is what lets the wait end at the *first* acceptance
/// rather than at the end of the designated share's whole fan-out — a share
/// planned to several helpers finishes its workflow only once the slowest of
/// them answers or times out.
///
/// It also removes any need to carry state between calls: a later pass, or
/// anything after a restart, sees the acceptance and does not wait.
///
/// Any failure to read reports "not accepted". The wait is bounded either way,
/// so an unreadable row costs at most the budget and never blocks delivery.
fn designated_share_accepted(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: Option<&str>,
) -> bool {
    let Some(round_id) = round_id else {
        return false;
    };
    let designation = {
        let conn = db.conn();
        crate::share_tracking::round_immediate_share(&conn, round_id, scope.wallet_id())
    };
    let Ok(Some(key)) = designation else {
        return false;
    };
    matches!(
        crate::share::get_delegation_for_scope(
            db,
            scope,
            round_id,
            key.bundle_index,
            key.proposal_id,
            key.share_index,
        ),
        Ok(Some(share)) if !share.sent_to_urls.is_empty()
    )
}

/// Records one finished share against its proposal and finalizes the report
/// when that proposal has no work left.
fn record_completion(
    proposals: &mut [ProposalDelivery<'_>],
    completion: ShareJobCompletion,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    on_report: &mut (dyn FnMut(&CommittedVote, &ShareBatchDeliveryReport) + Send),
) {
    let proposal = &mut proposals[completion.proposal_position];
    proposal.record(completion.payload_position, completion.delivery);
    let vote = proposal.vote;
    if let Some(report) = proposal.finish(cancel()) {
        on_report(vote, report);
    }
}
