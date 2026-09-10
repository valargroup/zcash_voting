//! Validates a whole commitment before any of its shares can enter delivery.

use crate::{
    round::VotingDb,
    share::ShareOperationScope,
    share_tracking::{ShareDeliveryPlan, ShareDeliverySubmissionParams},
    vote::CommittedVote,
    VotingError,
};

/// Immutable commitment-wide validation retained by all jobs for this proposal.
pub(super) struct PreparedVoteDelivery<'a> {
    pub(super) vote: &'a CommittedVote,
    pub(super) plan: ShareDeliveryPlan,
    pub(super) generation: String,
}

/// Audit a unit on a blocking worker so sibling upload futures remain pollable.
/// The worker owns its inputs and performs only reads; dropping the awaiting
/// caller cannot dispatch or mutate anything. Admission rechecks cancellation
/// and each share still revalidates its generation at the write boundary.
pub(super) async fn prepare_all<'a>(
    votes: &[&'a CommittedVote],
    db: &VotingDb,
    scope: &ShareOperationScope,
    params: &ShareDeliverySubmissionParams<'_>,
    observations: &crate::ObservationScope,
) -> Vec<Result<PreparedVoteDelivery<'a>, VotingError>> {
    if votes.is_empty() {
        return Vec::new();
    }
    let prepared = async {
        let db = db.scoped(scope.wallet_id())?;
        let scope = scope.clone();
        let owned_votes = votes.iter().map(|vote| (*vote).clone()).collect::<Vec<_>>();
        let fleet = params.configured_server_urls.to_vec();
        let observations = observations.clone();
        tokio::task::spawn_blocking(move || {
            let snapshot_stage = observations.stage("helper::load_delivery_snapshot");
            let requests = owned_votes
                .iter()
                .map(|vote| crate::share_tracking::DeliveryPlanRequest {
                    round_id: &vote.round_id,
                    bundle_index: vote.bundle_index,
                    proposal_id: vote.commit.proposal_id,
                    generation: &vote.commitment_bundle_json,
                    payloads: &vote.commit.share_payloads,
                })
                .collect::<Vec<_>>();
            let plans = crate::share_tracking::load_delivery_plans(
                &db,
                &scope,
                &requests,
                &fleet,
                &observations,
            );
            snapshot_stage.finish(
                if plans.iter().all(Result::is_ok) {
                    crate::ObservationOutcome::Succeeded
                } else {
                    crate::ObservationOutcome::Failed
                },
                None,
            );
            // No connection hold spans the expensive point/wire validation.
            owned_votes
                .iter()
                .zip(plans)
                .map(|(vote, plan)| {
                    // Validation covers this whole proposal, not the share or
                    // batch anchor that happened to trigger its parent step.
                    let proposal_observations = observations
                        .for_bundle(vote.bundle_index)
                        .attributed(crate::ObservationAttribution {
                            proposal_id: Some(vote.commit.proposal_id),
                            ..Default::default()
                        });
                    let stage = proposal_observations.stage("helper::validate_delivery_payloads");
                    let validated = plan.and_then(|(plan, generation)| {
                        validate_payloads(vote, &db, &scope, &plan, &generation)?;
                        Ok((plan, generation))
                    });
                    stage.finish(
                        if validated.is_ok() {
                            crate::ObservationOutcome::Succeeded
                        } else {
                            crate::ObservationOutcome::Failed
                        },
                        validated
                            .as_ref()
                            .err()
                            .map(crate::observability::voting_error_kind),
                    );
                    validated
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| VotingError::Internal {
            message: format!("delivery preparation worker failed: {error}"),
        })
    }
    .await;
    match prepared {
        Ok(plans) => votes
            .iter()
            .zip(plans)
            .map(|(&vote, plan)| {
                plan.map(|(plan, generation)| PreparedVoteDelivery {
                    vote,
                    plan,
                    generation,
                })
            })
            .collect(),
        Err(error) => votes
            .iter()
            .map(|_| {
                Err(VotingError::Internal {
                    message: format!("could not prepare delivery: {error}"),
                })
            })
            .collect(),
    }
}

/// Recheck confirmation and validate every payload after releasing the snapshot.
/// A concurrent replacement is rejected here or by generation-bound dispatch.
fn validate_payloads(
    vote: &CommittedVote,
    db: &VotingDb,
    scope: &ShareOperationScope,
    plan: &ShareDeliveryPlan,
    plan_generation: &str,
) -> Result<(), VotingError> {
    let recovery = crate::recovery::helper_recovery_material_for_wallet(
        db,
        scope.wallet_id(),
        &vote.round_id,
        vote.bundle_index,
        vote.commit.proposal_id,
    )?;
    let vc_tree_position = match recovery {
        crate::recovery::HelperRecoveryMaterial::Ready(bundle)
            if bundle.commitment_bundle_json == plan_generation =>
        {
            bundle.vc_tree_position
        }
        crate::recovery::HelperRecoveryMaterial::Ready(_) => {
            return Err(VotingError::InvalidInput {
                message: "committed vote changed after loading its helper-share delivery plan"
                    .to_string(),
            })
        }
        crate::recovery::HelperRecoveryMaterial::AwaitingVcPosition => {
            return Err(VotingError::InvalidInput {
                message: "committed vote must be confirmed before submitting helper shares"
                    .to_string(),
            })
        }
        crate::recovery::HelperRecoveryMaterial::Missing => {
            return Err(VotingError::Internal {
                message: "committed vote is missing durable helper recovery material".to_string(),
            })
        }
    };
    for (payload, share_plan) in vote.commit.share_payloads.iter().zip(&plan.share_plans) {
        payload.to_wire_json(Some(vc_tree_position), share_plan.submit_at)?;
    }

    Ok(())
}
