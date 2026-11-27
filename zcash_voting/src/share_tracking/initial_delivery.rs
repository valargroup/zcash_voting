use std::{collections::HashSet, time::Duration};

use crate::recovery as vote_recovery;
use crate::{
    helper::{client::HelperClient, url::canonical_helper_url_list},
    round::VotingDb,
    share,
    share_policy::{
        effective_share_submission_target_count, is_planned_share_submission_target_count,
        SHARE_DELIVERY_MIN_ATTEMPT_BUDGET_MILLISECONDS,
        SHARE_INITIAL_DELIVERY_TIMEOUT_MILLISECONDS,
    },
    types::{ShareDelegationRecord, SharePayload, VotingError},
};

use super::{
    configured_fleet::ConfiguredHelperFleet, CommitmentRef, CommittedShareSubmissionRequest,
    InitialShareSubmissionParams, ShareKey, ShareSubmissionReport, TrackedShare,
};
use crate::storage::queries::ShareAttemptReservation;

/// Fans one freshly built share out to helpers until enough accept it.
///
/// Outstanding targets are posted together in waves capped at the definite
/// acceptances still needed. Ambiguous attempts do not count as acceptances,
/// so later waves may contact additional helpers.
///
/// Helpers are reserved in health order within the planned and fallback
/// groups. Outcomes from one wave affect the ordering of later waves. Each
/// helper is selected at most once by this fan-out; the client separately
/// retries definite transient failures under its bounded retry policy.
///
/// Every helper is first persisted as `attempting`; only then can its POST be
/// dispatched. Definite acknowledgements and unknown outcomes are promoted to
/// their final state, while definite pre-dispatch failures clear the marker.
/// A process death or failed outcome write therefore leaves an outcome-unknown
/// marker. Recovery retries that interrupted helper after untried helpers,
/// preserving the original schedule unless the share is genuinely overdue.
///
/// # Errors
///
/// Returns [`VotingError::InvalidInput`] when the share JSON is malformed or
/// when any candidate URL fails
/// [`crate::helper::url::canonicalize_helper_base_url`]. Validation happens
/// before any network I/O and is all-or-nothing: one invalid configured URL
/// fails the call so a misconfigured fleet is surfaced instead of silently
/// shrinking delivery. Pre-filter with that function to tolerate individual
/// invalid entries.
#[cfg(test)]
pub(crate) async fn submit_share_to_helpers(
    db: &VotingDb,
    client: &HelperClient,
    params: &InitialShareSubmissionParams<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ShareSubmissionReport, VotingError> {
    let scope = share::ShareOperationScope::capture(db);
    serde_json::from_str::<serde_json::Value>(params.share_wire_json).map_err(|e| {
        VotingError::InvalidInput {
            message: format!("invalid helper share JSON: {e}"),
        }
    })?;
    // The fleet is part of the request contract. Validate it before creating
    // or merging any durable row so invalid input is storage-atomic.
    let planned = canonical_helper_url_list(params.planned_servers)?;
    let fallback = canonical_helper_url_list(params.fallback_servers)?;
    if planned.len() != params.planned_servers.len()
        || fallback.len() != params.fallback_servers.len()
        || fallback.iter().any(|url| planned.contains(url))
    {
        return Err(VotingError::InvalidInput {
            message: "planned and fallback helper groups must contain distinct canonical helpers"
                .to_string(),
        });
    }
    submit_share_to_canonical_helpers(db, &scope, client, params, &planned, &fallback, cancel).await
}

/// Executes a fan-out after the caller has established canonical, disjoint
/// candidate groups.
#[cfg(test)]
async fn submit_share_to_canonical_helpers(
    db: &VotingDb,
    scope: &share::ShareOperationScope,
    client: &HelperClient,
    params: &InitialShareSubmissionParams<'_>,
    planned: &[String],
    fallback: &[String],
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ShareSubmissionReport, VotingError> {
    let (_, persisted_delivery) = prepare_share_delivery(db, scope, params)?;
    let journal = VoteShareJournal::for_delivery(db, scope, params, persisted_delivery.nullifier);
    dispatch_share_to_canonical_helpers(
        &journal,
        scope,
        client,
        ShareFanOut {
            round_id: params.round_id,
            share_wire_json: params.share_wire_json,
            target_count: params.target_count,
            now_seconds: params.now_seconds,
            planned,
            fallback,
        },
        cancel,
    )
    .await
}

/// Creates or merges the durable record before dispatch and returns the
/// write-once schedule together with the merged delivery state.
#[cfg(test)]
fn prepare_share_delivery(
    db: &VotingDb,
    scope: &share::ShareOperationScope,
    params: &InitialShareSubmissionParams<'_>,
) -> Result<(u64, ShareDelegationRecord), VotingError> {
    let initial_persisted_report = ShareSubmissionReport {
        target_count: params.target_count,
        ..ShareSubmissionReport::default()
    };
    let (durable_submit_at, expected_nullifier) = share::record_delivery_for_scope(
        db,
        scope,
        &share::ShareDeliveryRecordParams {
            round_id: params.round_id,
            bundle_index: params.bundle_index,
            proposal_id: params.proposal_id,
            share_index: params.share_index,
            submission: &initial_persisted_report,
            submit_at: params.submit_at,
        },
    )?;
    load_prepared_share_delivery(db, scope, params, durable_submit_at, &expected_nullifier)
}

fn prepare_committed_share_delivery(
    db: &VotingDb,
    scope: &share::ShareOperationScope,
    params: &InitialShareSubmissionParams<'_>,
    expected_commitment_bundle_json: &str,
    expected_nullifier: &[u8; 32],
) -> Result<(u64, ShareDelegationRecord), VotingError> {
    let initial_persisted_report = ShareSubmissionReport {
        target_count: params.target_count,
        ..ShareSubmissionReport::default()
    };
    let (durable_submit_at, expected_nullifier) = share::record_delivery_for_committed_vote(
        db,
        scope,
        &share::ShareDeliveryRecordParams {
            round_id: params.round_id,
            bundle_index: params.bundle_index,
            proposal_id: params.proposal_id,
            share_index: params.share_index,
            submission: &initial_persisted_report,
            submit_at: params.submit_at,
        },
        expected_commitment_bundle_json,
        expected_nullifier,
    )?;
    load_prepared_share_delivery(db, scope, params, durable_submit_at, &expected_nullifier)
}

fn load_prepared_share_delivery(
    db: &VotingDb,
    scope: &share::ShareOperationScope,
    params: &InitialShareSubmissionParams<'_>,
    durable_submit_at: u64,
    expected_nullifier: &[u8; 32],
) -> Result<(u64, ShareDelegationRecord), VotingError> {
    let persisted_delivery = share::get_delegation_for_scope(
        db,
        scope,
        params.round_id,
        params.bundle_index,
        params.proposal_id,
        params.share_index,
    )?
    .filter(|share| share.nullifier == expected_nullifier.as_slice())
    .ok_or_else(|| VotingError::Internal {
        message: "newly journaled helper share was not found".to_string(),
    })?;
    Ok((durable_submit_at, persisted_delivery))
}

/// Where one share's fan-out durably journals its helper attempts.
///
/// A journal is bound to the share and to the generation its caller
/// validated, so every read and write refuses a share replaced or deleted
/// meanwhile.
pub(crate) trait ShareAttemptJournal: Sync {
    /// The share, which keys its process-wide operation lock.
    fn share(&self) -> ShareKey;

    /// Durably reserves `server_url` before its POST. Under
    /// [`EnforcePlacementTarget`](share::ShareAttemptCapacityPolicy::EnforcePlacementTarget)
    /// it refuses once accepted and in-flight helpers among `candidates`
    /// reach `target_count`.
    fn begin_attempt(
        &self,
        server_url: &str,
        target_count: usize,
        candidates: &[String],
        capacity: share::ShareAttemptCapacityPolicy,
    ) -> Result<ShareAttemptReservation, VotingError>;

    /// Durably records how the POST to `server_url` ended; `false` once the
    /// generation was replaced. `reset_submit_at` records an overdue resend's
    /// `submit_at = 0` as the share's schedule where it can change: a vote
    /// share's can, a DC share's is fixed at planning (`DcShareJournal`).
    fn resolve_attempt(
        &self,
        server_url: &str,
        outcome: share::ShareDeliveryAttemptOutcome,
        reset_submit_at: bool,
    ) -> Result<bool, VotingError>;
}

/// One share's fan-out and the disjoint helper groups it tries in order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShareFanOut<'a> {
    /// The round the share belongs to.
    pub(crate) round_id: &'a str,
    /// The validated helper request body.
    pub(crate) share_wire_json: &'a str,
    /// Definite acceptances the fan-out aims for.
    pub(crate) target_count: usize,
    /// Wall time of the pass, for helper health ordering.
    pub(crate) now_seconds: u64,
    /// The share's planned helpers, tried first.
    pub(crate) planned: &'a [String],
    /// Every other eligible helper, tried once the planned ones are spent.
    pub(crate) fallback: &'a [String],
}

/// A vote share's attempts in `share_delegations`, bound to its nullifier.
pub(super) struct VoteShareJournal<'a> {
    pub(super) db: &'a VotingDb,
    pub(super) scope: &'a share::ShareOperationScope,
    pub(super) round_id: &'a str,
    pub(super) bundle_index: u32,
    pub(super) proposal_id: u32,
    pub(super) share_index: u32,
    /// The share's reveal nullifier, as loaded.
    pub(super) nullifier: Vec<u8>,
}

impl<'a> VoteShareJournal<'a> {
    /// The journal of the share `params` delivers, as stored with `nullifier`.
    fn for_delivery(
        db: &'a VotingDb,
        scope: &'a share::ShareOperationScope,
        params: &InitialShareSubmissionParams<'a>,
        nullifier: Vec<u8>,
    ) -> Self {
        Self {
            db,
            scope,
            round_id: params.round_id,
            bundle_index: params.bundle_index,
            proposal_id: params.proposal_id,
            share_index: params.share_index,
            nullifier,
        }
    }

    /// The attempt params of a POST to `server_url` under `target_count`,
    /// with `submit_at` 0, which no attempt write reads.
    pub(super) fn attempt<'b>(
        &'b self,
        server_url: &'b str,
        target_count: usize,
    ) -> share::ShareDeliveryAttemptParams<'b> {
        share::ShareDeliveryAttemptParams {
            round_id: self.round_id,
            bundle_index: self.bundle_index,
            proposal_id: self.proposal_id,
            share_index: self.share_index,
            server_url,
            target_count,
            submit_at: 0,
        }
    }

    /// The share generation the journal is bound to.
    pub(super) fn generation(&self) -> share::ShareGeneration<'_> {
        share::ShareGeneration::new(self.scope, &self.nullifier)
    }
}

impl ShareAttemptJournal for VoteShareJournal<'_> {
    fn share(&self) -> ShareKey {
        ShareKey::vote(self.bundle_index, self.proposal_id, self.share_index)
    }

    fn begin_attempt(
        &self,
        server_url: &str,
        target_count: usize,
        candidates: &[String],
        capacity: share::ShareAttemptCapacityPolicy,
    ) -> Result<ShareAttemptReservation, VotingError> {
        share::begin_existing_delivery_attempt_for_generation(
            self.db,
            &self.attempt(server_url, target_count),
            self.generation(),
            candidates,
            capacity,
        )
    }

    fn resolve_attempt(
        &self,
        server_url: &str,
        outcome: share::ShareDeliveryAttemptOutcome,
        reset_submit_at: bool,
    ) -> Result<bool, VotingError> {
        share::resolve_delivery_attempt_for_generation(
            self.db,
            // A resolution reads no target.
            &self.attempt(server_url, 0),
            self.generation(),
            outcome,
            reset_submit_at,
        )
    }
}

/// Continues fan-out from an already-prepared durable delivery record.
/// See [`submit_share_to_helpers`] for the waves, ordering and durability.
pub(crate) async fn dispatch_share_to_canonical_helpers<J: TrackedShare>(
    journal: &J,
    scope: &share::ShareOperationScope,
    client: &HelperClient,
    fan_out: ShareFanOut<'_>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ShareSubmissionReport, VotingError> {
    let ShareFanOut {
        round_id,
        share_wire_json,
        target_count,
        now_seconds,
        planned,
        fallback,
    } = fan_out;
    let share = journal.share();
    let current_state = |candidates: &[String]| {
        journal
            .delivery_state(candidates)?
            .ok_or_else(|| stale_delivery_error(round_id, share))
    };
    let candidates = planned.iter().chain(fallback).cloned().collect::<Vec<_>>();
    let Some(_operation_guard) = super::lock_share_operation_or_cancel(
        scope,
        round_id,
        share,
        cancel,
        &client.observation_scope(),
    )
    .await?
    else {
        let delivery_state = current_state(&candidates)?;
        return Ok(delivery_report(&delivery_state, target_count));
    };
    let planned_set: HashSet<&str> = planned.iter().map(String::as_str).collect();
    let mut delivery_state = current_state(&candidates)?;
    let mut remaining = candidates.clone();
    remaining.retain(|url| {
        !delivery_state.accepted_urls().contains(url)
            && !delivery_state.outcome_unknown_urls().contains(url)
            && !delivery_state.in_flight_urls().contains(url)
    });
    let definite_acceptance_target = target_count.min(
        delivery_state
            .accepted_urls()
            .len()
            .saturating_add(remaining.len()),
    );
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(SHARE_INITIAL_DELIVERY_TIMEOUT_MILLISECONDS);

    while !remaining.is_empty() && delivery_state.accepted_urls().len() < definite_acceptance_target
    {
        if cancel() {
            break;
        }
        // Reserve only the outstanding acceptance need. Sequential reservations
        // preserve planned-before-fallback ordering and let the durable capacity
        // check observe every reservation already made in this wave.
        let need = definite_acceptance_target.saturating_sub(delivery_state.accepted_urls().len());
        let mut wave: Vec<String> = Vec::new();
        let mut stop_after_wave = false;
        while wave.len() < need && !remaining.is_empty() {
            if cancel() {
                stop_after_wave = true;
                break;
            }
            let active_group = if remaining
                .iter()
                .any(|url| planned_set.contains(url.as_str()))
            {
                remaining
                    .iter()
                    .filter(|url| planned_set.contains(url.as_str()))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                remaining.clone()
            };
            let ordered = client
                .health()
                .candidate_servers(&active_group, now_seconds);
            let Some(server_url) = ordered.into_iter().next() else {
                break;
            };
            let remaining_time = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining_time
                < Duration::from_millis(SHARE_DELIVERY_MIN_ATTEMPT_BUDGET_MILLISECONDS)
            {
                // An attempt started with almost no budget is guaranteed to end
                // as an unknown outcome; leave the helper untouched instead.
                stop_after_wave = true;
                break;
            }

            match journal.begin_attempt(
                &server_url,
                definite_acceptance_target,
                &candidates,
                share::ShareAttemptCapacityPolicy::EnforcePlacementTarget,
            )? {
                ShareAttemptReservation::Started => {
                    remaining.retain(|url| url != &server_url);
                    wave.push(server_url);
                }
                ShareAttemptReservation::AlreadyRecorded => {
                    remaining.retain(|url| url != &server_url);
                    continue;
                }
                ShareAttemptReservation::PlacementCapacityReached => {
                    // Resolving this wave can free capacity. Keep this helper
                    // eligible and reassess after those outcomes are durable.
                    break;
                }
                ShareAttemptReservation::StaleGeneration => {
                    return Err(stale_delivery_error(round_id, share));
                }
            }
        }

        if wave.is_empty() {
            break;
        }

        let placement_fleet = candidates.as_slice();
        let posts = wave.iter().map(|server_url| async move {
            // Waiting consumes the fan-out budget, but an unsent reservation
            // must remain a definite failure on cancellation or expiry.
            let observations = client.observation_scope().for_helper(server_url);
            let wait = observations.stage("helper::post_capacity_wait");
            let admission = super::post_capacity::acquire(deadline, cancel).await;
            wait.finish(
                match &admission {
                    Ok(_) => crate::ObservationOutcome::Succeeded,
                    Err(crate::helper::client::HelperError::Cancelled) => {
                        crate::ObservationOutcome::Cancelled
                    }
                    Err(_) => crate::ObservationOutcome::Failed,
                },
                admission
                    .as_ref()
                    .err()
                    .map(crate::observability::helper_error_kind),
            );
            let _permit = match admission {
                Ok(permit) => permit,
                Err(error) => return Ok(Ok(Err(error))),
            };
            // A permit wait can outlive the durable generation or its attempt.
            // Validate the starting wallet's journal again before any POST.
            let current_delivery = current_state(placement_fleet)?;
            if !current_delivery.in_flight_urls().contains(server_url) {
                return Err(stale_delivery_error(round_id, share));
            }
            let remaining_time = deadline.saturating_duration_since(tokio::time::Instant::now());
            Ok(tokio::time::timeout_at(
                deadline,
                client.submit_share_with_timeout(
                    server_url,
                    share_wire_json,
                    now_seconds,
                    cancel,
                    remaining_time,
                    Some(deadline),
                ),
            )
            .await)
        });
        let helper_post_outcomes = futures_util::future::join_all(posts).await;

        // Outcomes are applied in wave order, one at a time: each writes the
        // durable attempt row, and a stale generation must abort before a
        // later write can mask it.
        let mut deadline_elapsed = false;
        for (server_url, helper_post_outcome) in wave.iter().zip(helper_post_outcomes) {
            let helper_post_outcome = helper_post_outcome?;
            let resolve = |outcome| {
                if journal.resolve_attempt(server_url, outcome, false)? {
                    Ok(())
                } else {
                    Err(stale_delivery_error(round_id, share))
                }
            };
            match helper_post_outcome {
                Err(_) => {
                    // The client returns a held definite error rather than
                    // sleeping a retry backoff into this deadline, so an elapse
                    // here can only cancel an in-flight HTTP attempt, whose
                    // transport outcome is genuinely unknown; retain it for
                    // polling.
                    resolve(share::ShareDeliveryAttemptOutcome::Ambiguous)?;
                    delivery_state.mark_outcome_unknown(server_url)?;
                    deadline_elapsed = true;
                }
                Ok(Ok(_)) => {
                    let observations = client.observation_scope().for_helper(server_url);
                    observations.measure("helper::persist_acceptance", || {
                        resolve(share::ShareDeliveryAttemptOutcome::Accepted)
                    })?;
                    delivery_state.mark_accepted(server_url)?;
                }
                Ok(Err(error)) if error.is_ambiguous() => {
                    resolve(share::ShareDeliveryAttemptOutcome::Ambiguous)?;
                    delivery_state.mark_outcome_unknown(server_url)?;
                }
                Ok(Err(_)) => {
                    resolve(share::ShareDeliveryAttemptOutcome::DefiniteFailure)?;
                }
            }
        }

        if deadline_elapsed || stop_after_wave {
            break;
        }
    }
    delivery_state = current_state(&candidates)?;
    Ok(delivery_report(&delivery_state, target_count))
}

fn delivery_report(
    delivery_state: &share::ShareDeliveryState,
    target_count: usize,
) -> ShareSubmissionReport {
    ShareSubmissionReport {
        accepted_urls: delivery_state.accepted_urls().to_vec(),
        ambiguous_urls: delivery_state
            .outcome_unknown_urls()
            .iter()
            .chain(delivery_state.in_flight_urls())
            .cloned()
            .collect(),
        target_count,
    }
}

fn stale_delivery_error(round_id: &str, share: ShareKey) -> VotingError {
    let commitment = match share.commitment {
        CommitmentRef::Vote { proposal_id } => format!("proposal={proposal_id}"),
        CommitmentRef::Proxy { dc_slot } => format!("dc_slot={dc_slot}"),
    };
    VotingError::InvalidInput {
        message: format!(
            "committed share changed while helper delivery was in flight for \
             round={round_id}, bundle={}, {commitment}, share={}",
            share.bundle_index, share.share_index
        ),
    }
}

/// Validates and submits one payload owned by a committed vote.
///
/// The complete configured fleet, plan membership and target, committed share
/// identity, and recovery material are validated before delivery creates
/// durable state or dispatches network requests.
pub(crate) async fn submit_committed_share_to_helpers(
    db: &VotingDb,
    client: &HelperClient,
    round_id: &str,
    bundle_index: u32,
    proposal_id: u32,
    expected_vote_commitment: &[u8; 32],
    payloads: &[SharePayload],
    request: CommittedShareSubmissionRequest<'_>,
    expected_commitment_bundle_json: &str,
    scope: &share::ShareOperationScope,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ShareSubmissionReport, VotingError> {
    // Batch steps name an anchor proposal, but each concurrent share must
    // override that inherited identity before any of its transport work.
    let observations = client.observation_scope();
    observations.bind_round_id(round_id);
    let observations = observations.attributed(crate::ObservationAttribution {
        bundle_index: Some(bundle_index),
        proposal_id: Some(proposal_id),
        share_index: Some(request.share_index),
    });
    let observed_client = client.observing(&observations);
    let client = &observed_client;
    let configured = ConfiguredHelperFleet::new(request.configured_server_urls)?;
    let observed_client = client.observing(
        &client
            .observation_scope()
            .with_helper_fleet(configured.urls()),
    );
    let client = &observed_client;
    let planning_fleet = ConfiguredHelperFleet::new(request.planning_server_urls)?;
    let planned = canonical_helper_url_list(&request.plan.target_servers)?;
    if planned.len() != request.plan.target_servers.len() {
        return Err(VotingError::InvalidInput {
            message: "plan target_servers must contain distinct canonical helpers".to_string(),
        });
    }
    let planned_target = usize::try_from(request.plan.target_count).unwrap_or(usize::MAX);
    if !is_planned_share_submission_target_count(planned_target, planning_fleet.len())
        || planned.len() != planned_target
    {
        return Err(VotingError::InvalidInput {
            message: format!(
                "plan target_count {planned_target} and target_servers must match a planned target for its {}-helper planning fleet",
                planning_fleet.len()
            ),
        });
    }
    if let Some(server_url) = planned.iter().find(|url| !planning_fleet.contains(url)) {
        return Err(VotingError::InvalidInput {
            message: format!("planned helper is not in the persisted planning fleet: {server_url}"),
        });
    }
    let payload = payloads
        .iter()
        .find(|payload| payload.enc_share.share_index == request.share_index)
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!(
                "share_index {} is not present in the committed vote",
                request.share_index
            ),
        })?;
    if payload.vote_round_id != round_id || payload.proposal_id != proposal_id {
        return Err(VotingError::Internal {
            message: "committed share payload identity does not match its vote handle".to_string(),
        });
    }
    let (vc_tree_position, expected_nullifier) =
        match vote_recovery::helper_recovery_material_for_wallet(
            db,
            scope.wallet_id(),
            round_id,
            bundle_index,
            proposal_id,
        )? {
            vote_recovery::HelperRecoveryMaterial::Ready(bundle) => {
                if bundle.commitment_bundle_json != expected_commitment_bundle_json {
                    return Err(VotingError::InvalidInput {
                        message: format!(
                            "committed vote changed before helper share submission for \
                             round={round_id}, bundle={bundle_index}, proposal={proposal_id}; \
                             recover the current committed vote"
                        ),
                    });
                }
                let recovery = crate::vote::parse_recovery(&bundle.commitment_bundle_json)?;
                if recovery.vote_commitment != *expected_vote_commitment {
                    return Err(VotingError::InvalidInput {
                        message: format!(
                            "committed vote changed before helper share submission for \
                             round={round_id}, bundle={bundle_index}, proposal={proposal_id}; \
                             recover the current committed vote"
                        ),
                    });
                }
                let expected_nullifier = share::nullifier_from_recovery_json(
                    &bundle.commitment_bundle_json,
                    proposal_id,
                    request.share_index,
                )?;
                (bundle.vc_tree_position, expected_nullifier)
            }
            vote_recovery::HelperRecoveryMaterial::AwaitingVcPosition => {
                return Err(VotingError::InvalidInput {
                    message: "committed vote must be confirmed before submitting helper shares"
                        .to_string(),
                });
            }
            vote_recovery::HelperRecoveryMaterial::Missing => {
                return Err(VotingError::Internal {
                    message: "committed vote is missing durable helper recovery material"
                        .to_string(),
                });
            }
        };
    // Validate the complete typed payload before the durable row is created.
    // A continuation may replace this requested schedule with the write-once
    // value returned by persistence.
    let requested_share_wire_json =
        payload.to_wire_json(Some(vc_tree_position), request.plan.submit_at)?;
    let mut candidates = planned
        .into_iter()
        .filter(|url| configured.contains(url))
        .collect::<Vec<_>>();
    let eligible_planned_count = candidates.len();
    let fallback = configured
        .urls()
        .iter()
        .filter(|url| !candidates.contains(url))
        .cloned()
        .collect::<Vec<_>>();
    candidates.extend(fallback);
    let requested_params = InitialShareSubmissionParams {
        round_id,
        bundle_index,
        proposal_id,
        share_index: request.share_index,
        share_wire_json: &requested_share_wire_json,
        #[cfg(test)]
        planned_servers: &candidates[..eligible_planned_count],
        #[cfg(test)]
        fallback_servers: &candidates[eligible_planned_count..],
        // A plan stored under the previous cap keeps its validated target list
        // but never drives placement above the current cap.
        target_count: effective_share_submission_target_count(
            request.plan.target_count,
            planning_fleet.len(),
        ),
        submit_at: request.plan.submit_at,
        now_seconds: request.now_seconds,
    };
    let (durable_submit_at, persisted_delivery) = prepare_committed_share_delivery(
        db,
        scope,
        &requested_params,
        expected_commitment_bundle_json,
        &expected_nullifier,
    )?;
    let share_wire_json = payload.to_wire_json(Some(vc_tree_position), durable_submit_at)?;
    let durable_params = InitialShareSubmissionParams {
        share_wire_json: &share_wire_json,
        submit_at: durable_submit_at,
        ..requested_params
    };
    let journal =
        VoteShareJournal::for_delivery(db, scope, &durable_params, persisted_delivery.nullifier);
    dispatch_share_to_canonical_helpers(
        &journal,
        scope,
        client,
        ShareFanOut {
            round_id: durable_params.round_id,
            share_wire_json: durable_params.share_wire_json,
            target_count: durable_params.target_count,
            now_seconds: durable_params.now_seconds,
            planned: &candidates[..eligible_planned_count],
            fallback: &candidates[eligible_planned_count..],
        },
        cancel,
    )
    .await
}
