use super::{fixtures::*, *};
use crate::{ObservabilityOptions, ObservationOutcome as Outcome};
use tokio::sync::Semaphore;

#[tokio::test]
async fn round_audit_spans_clear_caller_identity_including_failures() {
    for corrupt_sibling in [false, true] {
        for parent_identity in [
            crate::ObservationAttribution::default(),
            crate::ObservationAttribution {
                bundle_index: Some(99),
                proposal_id: Some(1),
                share_index: Some(7),
            },
        ] {
            let fixture = Fixture::new(3);
            if corrupt_sibling {
                fixture.db.conn().execute(
                    "UPDATE helper_share_plans SET share_plans_json = json_set(share_plans_json, '$[0].immediate', json('true')) WHERE proposal_id = 3",
                    [],
                ).unwrap();
            }
            let invocation =
                crate::ObservationScope::new(Some(ObservabilityOptions::default())).invocation();
            invocation.scope().bind_round_id(ROUND_ID);
            let parent = invocation.scope().attributed(parent_identity);
            let transport = ScriptedTransport::new(|_| ReplyPlan::default());
            let client =
                HelperClient::new(transport.clone(), HelperHealth::default()).observing(&parent);
            // Only proposal 1 is submitted; the round audit must still detect
            // corruption in proposal 3 without blaming the triggering caller.
            let reports = crate::vote::submit_confirmed_vote_shares(
                &fixture.votes[..1],
                &fixture.db,
                &client,
                ShareDeliverySubmissionParams {
                    configured_server_urls: &fixture.configured,
                    now_seconds: SUBMIT_AT,
                },
                &uncancelled,
                &mut |_, _| {},
            )
            .await;
            assert_eq!(reports.len(), 1);
            if corrupt_sibling {
                assert!(reports
                    .into_iter()
                    .next()
                    .unwrap()
                    .delivery
                    .unwrap_err()
                    .error
                    .to_string()
                    .contains("more than one round-immediate share"));
                assert_eq!(transport.count(), 0);
            } else {
                assert_complete(
                    reports.into_iter().map(|report| report.delivery).collect(),
                    1,
                );
                assert_eq!(transport.count(), SHARE_COUNT);
            }
            let diagnostics = invocation
                .complete("delivery", Outcome::Succeeded, ())
                .observability
                .unwrap();
            assert_eq!(diagnostics.round_id.as_deref(), Some(ROUND_ID));
            let audits = diagnostics
                .records
                .iter()
                .filter(|record| record.stage.as_ref() == "helper::audit_delivery_round")
                .collect::<Vec<_>>();
            assert_eq!(audits.len(), 1);
            assert_eq!(
                audits[0].attribution,
                crate::ObservationAttribution::default()
            );
            assert_eq!(
                audits[0].outcome,
                if corrupt_sibling {
                    Outcome::Failed
                } else {
                    Outcome::Succeeded
                }
            );
            let queue = diagnostics
                .records
                .iter()
                .find(|record| record.stage.as_ref() == "helper::prepare_delivery_queue")
                .unwrap();
            assert_eq!(queue.attribution, parent_identity);
        }
    }
}

#[tokio::test]
async fn payload_validation_spans_identify_each_vote_including_failures() {
    for proposals in [1, 3] {
        for parent_bundle in [0, 99] {
            let fixture = Fixture::new(proposals);
            if proposals > 1 {
                // The plan remains structurally valid, but its second payload
                // cannot encode a schedule outside the JSON-safe integer range.
                fixture.db.conn().execute(
                    "UPDATE helper_share_plans SET share_plans_json = json_set(share_plans_json, '$[1].submit_at', 9007199254740992) WHERE proposal_id = 2",
                    [],
                ).unwrap();
            }
            let invocation =
                crate::ObservationScope::new(Some(ObservabilityOptions::default())).invocation();
            let parent_identity = crate::ObservationAttribution {
                bundle_index: Some(parent_bundle),
                proposal_id: Some(1),
                share_index: Some(7),
            };
            let parent = invocation.scope().attributed(parent_identity);
            let transport = ScriptedTransport::new(|_| ReplyPlan::default());
            let client =
                HelperClient::new(transport.clone(), HelperHealth::default()).observing(&parent);
            let reports = crate::vote::submit_confirmed_vote_shares(
                &fixture.votes,
                &fixture.db,
                &client,
                ShareDeliverySubmissionParams {
                    configured_server_urls: &fixture.configured,
                    now_seconds: SUBMIT_AT,
                },
                &uncancelled,
                &mut |_, _| {},
            )
            .await;
            for report in reports {
                if report.vote.proposal_id() == 2 {
                    assert!(report
                        .delivery
                        .unwrap_err()
                        .error
                        .to_string()
                        .contains("submit_at"));
                } else {
                    assert_complete(vec![report.delivery], 1);
                }
            }
            let diagnostics = invocation
                .complete("delivery", Outcome::Succeeded, ())
                .observability
                .unwrap();
            let validations = diagnostics
                .records
                .iter()
                .filter(|record| record.stage.as_ref() == "helper::validate_delivery_payloads")
                .collect::<Vec<_>>();
            assert_eq!(validations.len(), proposals as usize);
            for (proposal, record) in (1..=proposals).zip(validations) {
                assert_eq!(
                    record.attribution,
                    crate::ObservationAttribution {
                        bundle_index: Some(0),
                        proposal_id: Some(proposal),
                        share_index: None,
                    }
                );
                assert_eq!(
                    record.outcome,
                    if proposal == 2 {
                        Outcome::Failed
                    } else {
                        Outcome::Succeeded
                    }
                );
            }
            let queue = diagnostics
                .records
                .iter()
                .find(|record| record.stage.as_ref() == "helper::prepare_delivery_queue")
                .unwrap();
            assert_eq!(
                queue.attribution, parent_identity,
                "attributing members must not mutate the parent"
            );
            let valid_proposals = if proposals == 1 { 1 } else { proposals - 1 };
            assert_eq!(transport.count(), valid_proposals as usize * SHARE_COUNT);
            assert!(transport
                .started
                .lock()
                .unwrap()
                .iter()
                .all(|wire| wire.proposal_id != 2));
        }
    }
}

#[tokio::test]
async fn queue_residence_precedes_admission_and_active_delivery_includes_journaling() {
    let fixture = Fixture::new(5);
    let gate = Arc::new(Semaphore::new(0));
    let transport = ScriptedTransport::new({
        let gate = gate.clone();
        move |_| ReplyPlan {
            gate: Some(gate.clone()),
            ..Default::default()
        }
    });
    let invocation =
        crate::ObservationScope::new(Some(ObservabilityOptions::default())).invocation();
    let client =
        HelperClient::new(transport.clone(), HelperHealth::default()).observing(invocation.scope());
    let release = async {
        transport.wait_for(50).await;
        gate.add_permits(5 * SHARE_COUNT);
    };
    let mut on_report = |_: &crate::vote::CommittedVote, _: &ShareBatchDeliveryReport| {};
    let (reports, ()) = tokio::join!(
        crate::vote::submit_confirmed_vote_shares(
            &fixture.votes,
            &fixture.db,
            &client,
            ShareDeliverySubmissionParams {
                configured_server_urls: &fixture.configured,
                now_seconds: SUBMIT_AT
            },
            &uncancelled,
            &mut on_report,
        ),
        release,
    );
    assert_complete(reports.into_iter().map(|vote| vote.delivery).collect(), 5);
    let diagnostics = invocation
        .complete("delivery", Outcome::Succeeded, ())
        .observability
        .unwrap();
    assert_eq!(diagnostics.records_dropped, 0);
    assert_eq!(diagnostics.active_stages_dropped, 0);
    let preparation = diagnostics
        .records
        .iter()
        .find(|record| record.stage.as_ref() == "helper::prepare_delivery_queue")
        .expect("complete-plan validation has its own timing boundary");
    assert_eq!(preparation.outcome, Outcome::Succeeded);
    assert_eq!(
        diagnostics
            .records
            .iter()
            .filter(|record| record.stage.as_ref() == "helper::audit_delivery_round")
            .count(),
        1,
        "all five proposals share one authoritative round audit",
    );
    assert_eq!(
        diagnostics
            .records
            .iter()
            .filter(|record| record.stage.as_ref() == "helper::validate_delivery_payloads")
            .count(),
        5,
        "every proposal still validates its own complete payload set",
    );
    for proposal in 1..=5 {
        for share_index in 0..SHARE_COUNT as u32 {
            let records = diagnostics
                .records
                .iter()
                .filter(|record| {
                    record.attribution.proposal_id == Some(proposal)
                        && record.attribution.share_index == Some(share_index)
                })
                .collect::<Vec<_>>();
            let record = |stage: &str| {
                *records
                    .iter()
                    .find(|record| record.stage.as_ref() == stage)
                    .unwrap()
            };
            let queued = record("helper::delivery_queue_wait");
            assert!(
                preparation.started_after_us + preparation.elapsed_us <= queued.started_after_us
            );
            let active = record("helper::active_delivery");
            let post = record("helper::post_share");
            let persisted = record("helper::persist_acceptance");
            assert!(queued.started_after_us + queued.elapsed_us <= active.started_after_us);
            assert!(post.started_after_us + post.elapsed_us <= persisted.started_after_us);
            assert!(
                persisted.started_after_us + persisted.elapsed_us
                    <= active.started_after_us + active.elapsed_us
            );
            assert_eq!(post.outcome, Outcome::Pending);
            assert_eq!(persisted.outcome, Outcome::Succeeded);
            assert_eq!(post.endpoint_index, Some(0));
            assert_eq!(record("helper.http.post_json").endpoint_index, Some(0));
            assert_eq!(record("helper::post_capacity_wait").endpoint_index, Some(0));
        }
    }
    let first_finished = diagnostics
        .records
        .iter()
        .filter(|record| record.stage.as_ref() == "helper::active_delivery")
        .map(|record| record.started_after_us + record.elapsed_us)
        .min()
        .unwrap();
    let last_proposal = diagnostics.records.iter().filter(|record| {
        record.stage.as_ref() == "helper::delivery_queue_wait"
            && record.attribution.proposal_id == Some(5)
    });
    for queued in last_proposal {
        assert!(queued.started_after_us < first_finished);
        assert!(queued.started_after_us + queued.elapsed_us >= first_finished);
    }
}
