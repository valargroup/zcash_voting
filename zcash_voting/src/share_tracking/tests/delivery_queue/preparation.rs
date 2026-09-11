//! Shared preparation remains operation-local and leaves sibling futures live.

use super::{fixtures::*, *};
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn cancellation_during_preparation_leaves_every_share_pending() {
    let fixture = Fixture::new(3);
    let cancelled = AtomicBool::new(false);
    let cancel = || cancelled.load(Ordering::SeqCst);
    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let (reports, ()) = tokio::join!(
        biased;
        fixture.deliver(transport.clone(), &cancel),
        async { cancelled.store(true, Ordering::SeqCst); },
    );
    for report in reports {
        let report = report.unwrap();
        assert!(report.cancelled);
        assert!(report.deliveries.is_empty());
        assert_eq!(report.pending_share_indices.len(), SHARE_COUNT);
    }
    assert_eq!(transport.count(), 0);
    assert!(share::list(&fixture.db, ROUND_ID).unwrap().is_empty());
}

#[tokio::test]
async fn preparation_connection_wait_does_not_block_sibling_futures() {
    let fixture = Fixture::new(3);
    let (held, holding) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let database = Arc::clone(&fixture.db);
    let holder = std::thread::spawn(move || {
        let _connection = database.conn();
        held.send(()).unwrap();
        // A synchronous preparation on the runtime thread cannot poll the
        // sibling that releases us. Bound that regression rather than hang.
        released.recv_timeout(Duration::from_secs(5)).is_ok()
    });
    holding.await.unwrap();
    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let (reports, ()) = tokio::join!(
        biased;
        fixture.deliver(transport.clone(), &uncancelled),
        async { release.send(()).unwrap(); },
    );
    assert!(
        holder.join().unwrap(),
        "preparation blocked sibling polling"
    );
    assert_complete(reports, 3);
}

#[tokio::test]
async fn a_later_delivery_reaudits_round_immediate_plans() {
    let fixture = Fixture::new(3);
    let first = ScriptedTransport::new(|_| ReplyPlan::default());
    assert_complete(fixture.deliver(first, &uncancelled).await, 3);
    fixture.db.conn().execute(
        "UPDATE helper_share_plans SET share_plans_json = json_set(share_plans_json, '$[0].immediate', json('true')) WHERE proposal_id = 3",
        [],
    ).unwrap();
    let resumed = ScriptedTransport::new(|_| ReplyPlan::default());
    let reports = fixture.deliver(resumed.clone(), &uncancelled).await;
    assert_eq!(reports.len(), 3);
    for report in reports {
        assert!(report
            .unwrap_err()
            .error
            .to_string()
            .contains("more than one round-immediate share"));
    }
    assert_eq!(resumed.count(), 0);
}

#[tokio::test]
async fn wallet_switch_during_preparation_does_not_redirect_the_snapshot() {
    let fixture = Fixture::new(3);
    let wallet = fixture.db.wallet_id();
    let transport = ScriptedTransport::new(|_| ReplyPlan::default());
    let (reports, ()) = tokio::join!(
        biased;
        fixture.deliver(transport.clone(), &uncancelled),
        async { fixture.db.set_wallet_id("replacement-wallet"); },
    );
    assert_complete(reports, 3);
    assert!(share::list(&fixture.db, ROUND_ID).unwrap().is_empty());
    fixture.db.set_wallet_id(&wallet);
    assert_eq!(
        share::list(&fixture.db, ROUND_ID).unwrap().len(),
        3 * SHARE_COUNT
    );
}
