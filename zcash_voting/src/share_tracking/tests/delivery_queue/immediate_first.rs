//! The designated immediate share reaches a helper before the round's others.

use super::{fixtures::*, *};
use tokio::sync::Semaphore;

/// The round's designated share, as `(proposal_id, share_index)` on the wire.
///
/// Read from the durable designation rather than inferred from the plans: the
/// designation is the round's own record, written when the designated vote's
/// plan was first prepared, and it is what delivery orders against.
fn designated_wire_identity(fixture: &Fixture) -> (u32, u32) {
    let wallet = fixture.db.wallet_id();
    let conn = fixture.db.conn();
    let key = crate::share_tracking::round_immediate_share(&conn, ROUND_ID, &wallet)
        .unwrap()
        .expect("the fixture round designates an immediate share");
    (key.proposal_id, key.share_index)
}

/// Ordering inside one call. The designated share is naturally the first job of
/// its own commitment, so the votes are supplied in reverse to put its
/// commitment last and prove the queue moves it rather than finding it there.
#[tokio::test(start_paused = true)]
async fn the_immediate_share_is_posted_before_every_other_share() {
    let mut fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);
    fixture.votes.reverse();

    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let reports = fixture.deliver(transport.clone(), &uncancelled).await;
    assert_complete(reports, 3);

    let started = transport.started.lock().unwrap();
    assert_eq!(
        (started[0].proposal_id, started[0].share_index),
        designated,
        "the designated share is POSTed first even when its commitment is last"
    );
}

/// The point of the barrier: a concurrent delivery that does not hold the
/// designated share waits for it to reach a helper, then runs unrestricted.
#[tokio::test]
async fn other_bundles_wait_for_the_immediate_ack_then_deliver_without_limits() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);

    // Hold the designated POST open so the waiter cannot race past it.
    let release = Arc::new(Semaphore::new(0));
    let transport = ScriptedTransport::new({
        let release = release.clone();
        move |wire| ReplyPlan {
            gate: ((wire.proposal_id, wire.share_index) == designated).then(|| release.clone()),
            ..Default::default()
        }
    });

    let holder: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() == designated.0)
        .cloned()
        .collect();
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();

    let deliver = |votes: Vec<crate::vote::ConfirmedVote>| {
        let db = Arc::clone(&fixture.db);
        let configured = fixture.configured.clone();
        let transport = transport.clone();
        tokio::spawn(async move {
            crate::vote::submit_confirmed_vote_shares(
                &votes,
                &db,
                &HelperClient::new(transport, HelperHealth::default()),
                ShareDeliverySubmissionParams {
                    configured_server_urls: &configured,
                    now_seconds: SUBMIT_AT,
                },
                &uncancelled,
                &mut |_, _| {},
            )
            .await
            .into_iter()
            .map(|vote| vote.delivery)
            .collect::<Vec<_>>()
        })
    };

    // Both deliveries run concurrently, as two bundles of one round do.
    let held = deliver(holder);
    let waiting = deliver(others);

    // The holder's own commitment proceeds — a designated share does not bar its
    // siblings — but the concurrent delivery must send nothing until the gate
    // opens, despite having capacity for thirty-two shares.
    transport.wait_for(SHARE_COUNT).await;
    {
        let started = transport.started.lock().unwrap();
        assert_eq!(
            (started[0].proposal_id, started[0].share_index),
            designated,
            "the designated share is dispatched before anything else"
        );
        assert_eq!(
            started.len(),
            SHARE_COUNT,
            "only the holder's own commitment is in flight"
        );
        assert!(
            started.iter().all(|wire| wire.proposal_id == designated.0),
            "the waiting delivery must not POST before the ack"
        );
    }

    release.add_permits(1);
    assert_complete(held.await.unwrap(), 1);
    assert_complete(waiting.await.unwrap(), 2);
    assert_eq!(transport.count(), 3 * SHARE_COUNT);
}

/// The safety property. A round whose designated bundle has not confirmed leaves
/// no call holding the share, and the ready bundles must still deliver.
#[tokio::test(start_paused = true)]
async fn the_gate_expires_so_a_round_whose_designated_bundle_is_unconfirmed_still_delivers() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();

    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let reports = crate::vote::submit_confirmed_vote_shares(
        &others,
        &fixture.db,
        &HelperClient::new(transport.clone(), HelperHealth::default()),
        ShareDeliverySubmissionParams {
            configured_server_urls: &fixture.configured,
            now_seconds: SUBMIT_AT,
        },
        &uncancelled,
        &mut |_, _| {},
    )
    .await;

    assert_complete(reports.into_iter().map(|vote| vote.delivery).collect(), 2);
    assert_eq!(transport.count(), 2 * SHARE_COUNT);
}

/// Cancellation while blocked leaves the shares pending rather than recording
/// attempts nothing made.
#[tokio::test(start_paused = true)]
async fn cancellation_while_waiting_on_the_gate_leaves_shares_pending() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();

    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let cancelled = || true;
    let reports = crate::vote::submit_confirmed_vote_shares(
        &others,
        &fixture.db,
        &HelperClient::new(transport.clone(), HelperHealth::default()),
        ShareDeliverySubmissionParams {
            configured_server_urls: &fixture.configured,
            now_seconds: SUBMIT_AT,
        },
        &cancelled,
        &mut |_, _| {},
    )
    .await;

    assert_eq!(transport.count(), 0, "a cancelled wait POSTs nothing");
    for report in reports {
        assert!(report.delivery.unwrap().cancelled);
    }
    let persisted = share::list(&fixture.db, ROUND_ID).unwrap();
    assert!(persisted
        .iter()
        .all(|share| share.sent_to_urls.is_empty() && share.attempting_urls.is_empty()));
}

/// A later delivery pass must not wait for a share that is already placed.
///
/// The round's gate does not survive its last delivery, so a pass that finds no
/// holder would otherwise spend the whole budget waiting for an acknowledgement
/// that was recorded long ago.
#[tokio::test(start_paused = true)]
async fn a_pass_after_the_immediate_share_is_accepted_does_not_wait() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);
    let transport = ScriptedTransport::new(|_| ReplyPlan::default());

    // First pass: everything delivers, including the designated share.
    let reports = fixture.deliver(transport.clone(), &uncancelled).await;
    assert_complete(reports, 3);
    let persisted = share::list(&fixture.db, ROUND_ID).unwrap();
    assert!(persisted
        .iter()
        .any(|share| share.proposal_id == designated.0
            && share.share_index == designated.1
            && !share.sent_to_urls.is_empty()));

    // A later pass over the other proposals holds no designated share. It must
    // proceed on the durable acceptance rather than waiting out the budget.
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();
    let before = tokio::time::Instant::now();
    let reports = crate::vote::submit_confirmed_vote_shares(
        &others,
        &fixture.db,
        &HelperClient::new(transport.clone(), HelperHealth::default()),
        ShareDeliverySubmissionParams {
            configured_server_urls: &fixture.configured,
            now_seconds: SUBMIT_AT,
        },
        &uncancelled,
        &mut |_, _| {},
    )
    .await;
    assert_eq!(reports.len(), 2);
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "a pass whose designated share is already accepted must not wait: {:?}",
        before.elapsed()
    );
}

/// A refused designated share must release the round for good.
///
/// Refusal releases the waiters that exist, but a definite failure records no
/// acceptance, so an acceptance-only test would send every later pass back into
/// the full budget waiting for a share no helper is going to take.
#[tokio::test(start_paused = true)]
async fn a_later_pass_does_not_wait_again_after_the_designated_share_was_refused() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);

    // Every helper refuses the designated share; the rest succeed.
    let transport = ScriptedTransport::new(move |wire| ReplyPlan {
        status: if (wire.proposal_id, wire.share_index) == designated {
            500
        } else {
            200
        },
        ..Default::default()
    });
    let _ = fixture.deliver(transport.clone(), &uncancelled).await;

    // Nothing durable records an acceptance for it.
    let persisted = share::list(&fixture.db, ROUND_ID).unwrap();
    assert!(persisted
        .iter()
        .any(|share| share.proposal_id == designated.0
            && share.share_index == designated.1
            && share.sent_to_urls.is_empty()));

    // A later pass over the other proposals must not wait out the budget.
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();
    let before = tokio::time::Instant::now();
    let reports = crate::vote::submit_confirmed_vote_shares(
        &others,
        &fixture.db,
        &HelperClient::new(transport, HelperHealth::default()),
        ShareDeliverySubmissionParams {
            configured_server_urls: &fixture.configured,
            now_seconds: SUBMIT_AT,
        },
        &uncancelled,
        &mut |_, _| {},
    )
    .await;
    assert_eq!(reports.len(), 2);
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "a refused designation must not make later passes wait: {:?}",
        before.elapsed()
    );
}

/// A designated share this call cannot dispatch must release the round now.
///
/// Preparation failure leaves the designated proposal with no job, so a holder
/// identified from the job list alone would read as "someone else has it" and
/// leave this call's own siblings waiting out the budget for a share that is
/// already resolved and sitting in front of them.
#[tokio::test(start_paused = true)]
async fn a_designated_share_that_cannot_be_prepared_releases_the_round() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);

    // Remove the designated proposal's durable plan so its preparation fails.
    fixture
        .db
        .conn()
        .execute(
            "DELETE FROM helper_share_plans WHERE proposal_id = ?1",
            [designated.0],
        )
        .unwrap();

    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let before = tokio::time::Instant::now();
    let reports = fixture.deliver(transport.clone(), &uncancelled).await;

    assert!(
        before.elapsed() < Duration::from_secs(1),
        "siblings must not wait for a designated share this call cannot send: {:?}",
        before.elapsed()
    );
    // The designated proposal fails; the other two still deliver in full.
    let delivered = reports
        .iter()
        .filter(|report| {
            report
                .as_ref()
                .is_ok_and(|r| r.deliveries.len() == SHARE_COUNT)
        })
        .count();
    assert_eq!(delivered, 2);
}

/// A gate wait is recorded, so its cost is not read as delivery contention.
///
/// Every share's `helper::delivery_queue_wait` stage is already open when the
/// gate wait begins, so without its own record an expired wait would be charged
/// to generic queue time and misattributed in a bottleneck table.
#[tokio::test(start_paused = true)]
async fn an_expired_gate_wait_is_recorded_separately_from_queue_time() {
    let fixture = Fixture::new(3);
    let designated = designated_wire_identity(&fixture);
    let others: Vec<_> = fixture
        .votes
        .iter()
        .filter(|vote| vote.vote().proposal_id() != designated.0)
        .cloned()
        .collect();

    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let invocation =
        crate::ObservationScope::new(Some(crate::ObservabilityOptions::default())).invocation();
    let client =
        HelperClient::new(transport, HelperHealth::default()).observing(invocation.scope());
    let _ = crate::vote::submit_confirmed_vote_shares(
        &others,
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
    let diagnostics = invocation
        .complete("delivery", crate::ObservationOutcome::Succeeded, ())
        .observability
        .expect("observability was requested");
    let gate = diagnostics
        .records
        .iter()
        .find(|record| &*record.stage == "helper::immediate_gate_wait")
        .expect("the gate wait is recorded");
    assert_eq!(
        gate.outcome,
        crate::ObservationOutcome::Pending,
        "an expired wait is distinguishable from one that was released"
    );
}
