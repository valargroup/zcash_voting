use std::collections::HashSet;

use crate::{
    helper::client::{HelperClient, HelperError},
    share,
    share_policy::{
        is_share_resubmission_window_open, resubmission_server_order,
        resubmission_server_order_random_bytes_required,
    },
    types::VotingError,
};

use super::{dedupe_preserving_order, tracked_share::TrackedShare, ShareTrackingParams};

pub(super) enum ResubmitOutcome {
    DefinitelyAcceptedByHelper(String),
    /// No helper in the recovery order definitely accepted the share this pass.
    NoDefiniteAcceptanceObserved,
    /// The loaded share was deleted or replaced while recovery was in flight.
    StaleGeneration,
    /// The vote-end recovery window closed during this tracking pass.
    CutoffReached,
    Cancelled,
}

pub(super) struct ResubmitReport {
    pub(super) outcome: ResubmitOutcome,
    pub(super) outcome_unknown_urls: Vec<String>,
}

/// A recovery walk that failed after possibly retaining ambiguous outcomes.
#[derive(Debug)]
pub(super) struct FailedResubmission {
    pub(super) error: VotingError,
    pub(super) outcome_unknown_urls: Vec<String>,
}

impl FailedResubmission {
    fn after_retained_outcomes(error: VotingError, outcome_unknown_urls: &[String]) -> Self {
        Self {
            error,
            outcome_unknown_urls: outcome_unknown_urls.to_vec(),
        }
    }
}

impl From<VotingError> for FailedResubmission {
    fn from(error: VotingError) -> Self {
        Self {
            error,
            outcome_unknown_urls: Vec::new(),
        }
    }
}

pub(super) struct ResubmitRequest<'a> {
    /// The share's helper request, rebuilt with the schedule's `submit_at`.
    pub(super) share_wire_json: &'a str,
    /// The configured helper fleet, already canonicalized by the caller.
    pub(super) configured_urls: &'a [String],
    pub(super) definite_acceptance_urls: &'a [String],
    /// Helpers whose completed POST has no known outcome.
    pub(super) ambiguous_urls: &'a [String],
    /// Helpers left in flight by an interrupted process.
    pub(super) interrupted_attempt_urls: &'a [String],
    pub(super) target_count: usize,
    pub(super) schedule: ResubmissionSchedule,
    pub(super) candidates: ResubmissionCandidates,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResubmissionCandidates {
    FullRecoveryOrder,
    InterruptedOnly,
}

#[derive(Clone, Copy)]
pub(super) enum ResubmissionSchedule {
    PreserveScheduledSubmitAt(u64),
    Immediate,
}

impl ResubmissionSchedule {
    pub(super) fn submit_at(self) -> u64 {
        match self {
            Self::PreserveScheduledSubmitAt(scheduled_submit_at) => scheduled_submit_at,
            Self::Immediate => 0,
        }
    }

    fn reset_submit_at(self) -> bool {
        matches!(self, Self::Immediate)
    }
}

/// Walks the randomized resubmission order until one helper accepts.
///
/// Untried helpers come first. An interrupted attempt follows them even during
/// early recovery so a crash marker cannot strand a share when vote-end timing
/// is unavailable. Completed ambiguous attempts and accepted fallbacks remain
/// overdue-only. Helper-side duplicate detection makes every replay converge
/// instead of double-counting.
///
/// Randomization is preserved within the untried and previously attempted
/// groups; interrupted and ambiguous retry groups are deterministic last
/// resorts ranked only by health, since their membership is already persisted.
/// Degraded helpers move behind healthy peers in their group. Every POST is
/// journaled before dispatch, and every accepted or ambiguous outcome is
/// persisted before returning or advancing to another helper.
pub(super) async fn resubmit_to_next_helper<S: TrackedShare>(
    tracked: &S,
    params: &ShareTrackingParams<'_>,
    client: &HelperClient,
    request: &ResubmitRequest<'_>,
    attempted_urls: &mut Vec<String>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    elapsed_seconds: &(dyn Fn() -> u64 + Send + Sync),
) -> Result<ResubmitReport, FailedResubmission> {
    let ambiguous_helpers: HashSet<&str> =
        request.ambiguous_urls.iter().map(String::as_str).collect();
    let interrupted_helpers: HashSet<&str> = request
        .interrupted_attempt_urls
        .iter()
        .map(String::as_str)
        .collect();
    let definitely_accepted_helpers: HashSet<&str> = request
        .definite_acceptance_urls
        .iter()
        .map(String::as_str)
        .collect();
    let eligible_servers: Vec<String> =
        if request.candidates == ResubmissionCandidates::FullRecoveryOrder {
            let attempted: HashSet<&str> = attempted_urls.iter().map(String::as_str).collect();
            dedupe_preserving_order(
                request
                    .configured_urls
                    .iter()
                    .filter(|url| !ambiguous_helpers.contains(url.as_str()))
                    .filter(|url| !interrupted_helpers.contains(url.as_str()))
                    .filter(|url| !attempted.contains(url.as_str()))
                    .filter(|url| {
                        request.schedule.reset_submit_at()
                            || !definitely_accepted_helpers.contains(url.as_str())
                    })
                    .cloned(),
            )
        } else {
            Vec::new()
        };
    let needed = resubmission_server_order_random_bytes_required(
        &eligible_servers,
        request.definite_acceptance_urls,
    );
    #[cfg(test)]
    let random_bytes = (params.random_bytes)(needed);
    #[cfg(not(test))]
    let random_bytes = super::os_random_bytes(needed);
    let randomized = resubmission_server_order(
        &eligible_servers,
        request.definite_acceptance_urls,
        &random_bytes,
    )?;
    let untried_count = randomized
        .iter()
        .take_while(|url| !definitely_accepted_helpers.contains(url.as_str()))
        .count();
    let (untried, previously_attempted) = randomized.split_at(untried_count);
    let ordering_time = params.now_seconds.saturating_add(elapsed_seconds());
    let mut order = client.health().candidate_servers(untried, ordering_time);
    let interrupted_retry_urls = request
        .interrupted_attempt_urls
        .iter()
        .filter(|url| !attempted_urls.contains(url))
        .filter(|url| !definitely_accepted_helpers.contains(url.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    order.extend(
        client
            .health()
            .candidate_servers(&interrupted_retry_urls, ordering_time),
    );
    if request.schedule.reset_submit_at()
        && request.candidates == ResubmissionCandidates::FullRecoveryOrder
    {
        let ambiguous_retry_urls: Vec<String> = request
            .ambiguous_urls
            .iter()
            .filter(|url| !attempted_urls.contains(url))
            .filter(|url| !definitely_accepted_helpers.contains(url.as_str()))
            .cloned()
            .collect();
        order.extend(
            client
                .health()
                .candidate_servers(&ambiguous_retry_urls, ordering_time),
        );
        order.extend(
            client
                .health()
                .candidate_servers(previously_attempted, ordering_time),
        );
    }
    let mut newly_outcome_unknown_urls = Vec::new();
    let mut fresh_placement_capacity_reached = false;
    for server_url in order {
        if cancel() {
            return Ok(ResubmitReport {
                outcome: ResubmitOutcome::Cancelled,
                outcome_unknown_urls: newly_outcome_unknown_urls,
            });
        }
        let current_time = params.now_seconds.saturating_add(elapsed_seconds());
        if params.vote_end_time_seconds.is_some_and(|vote_end| {
            !is_share_resubmission_window_open(current_time, vote_end, params.policy)
        }) {
            return Ok(ResubmitReport {
                outcome: ResubmitOutcome::CutoffReached,
                outcome_unknown_urls: newly_outcome_unknown_urls,
            });
        }
        attempted_urls.push(server_url.clone());
        let retries_ambiguous_helper = ambiguous_helpers.contains(server_url.as_str());
        let retries_interrupted_helper = interrupted_helpers.contains(server_url.as_str());
        let retries_outcome_unknown_helper = retries_ambiguous_helper || retries_interrupted_helper;
        let retries_definitely_accepted_helper =
            definitely_accepted_helpers.contains(server_url.as_str());
        let journals_fresh_attempt =
            !retries_outcome_unknown_helper && !retries_definitely_accepted_helper;
        let capacity_policy =
            if request.schedule.reset_submit_at() || !request.interrupted_attempt_urls.is_empty() {
                share::ShareAttemptCapacityPolicy::AllowRecoveryBeyondPlacementTarget
            } else {
                share::ShareAttemptCapacityPolicy::EnforcePlacementTarget
            };
        // Outcome-unknown and previously accepted helpers are already durably
        // journaled, so the journal-before-dispatch invariant holds without a
        // new attempting marker (which the guard would refuse). Re-read
        // confirmation immediately before either last-resort re-POST.
        let may_dispatch = if journals_fresh_attempt {
            if fresh_placement_capacity_reached {
                continue;
            }
            match tracked
                .begin_attempt(
                    &server_url,
                    request.target_count,
                    request.configured_urls,
                    capacity_policy,
                )
                .map_err(|error| {
                    FailedResubmission::after_retained_outcomes(error, &newly_outcome_unknown_urls)
                })? {
                crate::storage::queries::ShareAttemptReservation::Started => true,
                crate::storage::queries::ShareAttemptReservation::AlreadyRecorded => false,
                crate::storage::queries::ShareAttemptReservation::PlacementCapacityReached => {
                    fresh_placement_capacity_reached = true;
                    false
                }
                crate::storage::queries::ShareAttemptReservation::StaleGeneration => {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
            }
        } else {
            match tracked.stored_confirmation().map_err(|error| {
                FailedResubmission::after_retained_outcomes(error, &newly_outcome_unknown_urls)
            })? {
                Some(confirmed) => !confirmed,
                None => {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
            }
        };
        if !may_dispatch {
            continue;
        }
        match client
            .resubmit_share(&server_url, request.share_wire_json, current_time, cancel)
            .await
        {
            Ok(_) => {
                if !tracked
                    .resolve_attempt(
                        &server_url,
                        share::ShareDeliveryAttemptOutcome::Accepted,
                        request.schedule.reset_submit_at(),
                    )
                    .map_err(|error| {
                        FailedResubmission::after_retained_outcomes(
                            error,
                            &newly_outcome_unknown_urls,
                        )
                    })?
                {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
                return Ok(ResubmitReport {
                    outcome: ResubmitOutcome::DefinitelyAcceptedByHelper(server_url),
                    outcome_unknown_urls: newly_outcome_unknown_urls,
                });
            }
            Err(HelperError::Cancelled) => {
                if journals_fresh_attempt
                    && !tracked
                        .resolve_attempt(
                            &server_url,
                            share::ShareDeliveryAttemptOutcome::DefiniteFailure,
                            request.schedule.reset_submit_at(),
                        )
                        .map_err(|error| {
                            FailedResubmission::after_retained_outcomes(
                                error,
                                &newly_outcome_unknown_urls,
                            )
                        })?
                {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
                return Ok(ResubmitReport {
                    outcome: ResubmitOutcome::Cancelled,
                    outcome_unknown_urls: newly_outcome_unknown_urls,
                });
            }
            // A weaker outcome from a recovery re-POST cannot downgrade the
            // durable acceptance established by the original request.
            Err(error) if error.is_ambiguous() && retries_definitely_accepted_helper => {}
            Err(error) if error.is_ambiguous() => {
                if !tracked
                    .resolve_attempt(
                        &server_url,
                        share::ShareDeliveryAttemptOutcome::Ambiguous,
                        request.schedule.reset_submit_at(),
                    )
                    .map_err(|error| {
                        FailedResubmission::after_retained_outcomes(
                            error,
                            &newly_outcome_unknown_urls,
                        )
                    })?
                {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
                if !retries_ambiguous_helper {
                    newly_outcome_unknown_urls.push(server_url);
                }
            }
            Err(_) if retries_interrupted_helper => {
                if !tracked
                    .resolve_attempt(
                        &server_url,
                        share::ShareDeliveryAttemptOutcome::Ambiguous,
                        request.schedule.reset_submit_at(),
                    )
                    .map_err(|error| {
                        FailedResubmission::after_retained_outcomes(
                            error,
                            &newly_outcome_unknown_urls,
                        )
                    })?
                {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
                newly_outcome_unknown_urls.push(server_url);
            }
            // A definite failure of a re-POST says nothing about the original
            // outcome-unknown POST, so that persisted state is kept.
            Err(_) if retries_outcome_unknown_helper => {}
            Err(_) => {
                if !tracked
                    .resolve_attempt(
                        &server_url,
                        share::ShareDeliveryAttemptOutcome::DefiniteFailure,
                        request.schedule.reset_submit_at(),
                    )
                    .map_err(|error| {
                        FailedResubmission::after_retained_outcomes(
                            error,
                            &newly_outcome_unknown_urls,
                        )
                    })?
                {
                    return Ok(ResubmitReport {
                        outcome: ResubmitOutcome::StaleGeneration,
                        outcome_unknown_urls: newly_outcome_unknown_urls,
                    });
                }
            }
        }
    }
    Ok(ResubmitReport {
        outcome: ResubmitOutcome::NoDefiniteAcceptanceObserved,
        outcome_unknown_urls: newly_outcome_unknown_urls,
    })
}
