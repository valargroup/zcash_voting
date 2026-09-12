//! A normalized crash reservation still owes its first recovery check.

use super::*;

fn abandoned_store() -> (Arc<InMemoryChainSubmissionStore>, StoreAdvancementRequest) {
    let identity = identity(1, 0);
    let store = Arc::new(InMemoryChainSubmissionStore::default());
    store.seed_derivation(derived(identity.clone(), 1));
    let request = StoreAdvancementRequest::vote(identity);
    assert!(matches!(
        store.admit(&request, true, 1, 90).unwrap(),
        StoreAdmission::Ready {
            fresh_reservation: true,
            ..
        }
    ));
    (store, request)
}

struct InterruptAfterNormalization {
    store: Arc<InMemoryChainSubmissionStore>,
    identity: ChainSubmissionIdentity,
    change_epoch: bool,
}

impl InterruptAfterNormalization {
    fn normalized(&self) -> bool {
        self.store.record(&self.identity).unwrap().durable_state()
            == ChainSubmissionState::Recovering
    }
}

impl SubmissionControl for InterruptAfterNormalization {
    fn is_cancelled(&self) -> bool {
        !self.change_epoch && self.normalized()
    }

    fn operation_epoch(&self) -> u64 {
        u64::from(self.change_epoch && self.normalized())
    }
}

#[tokio::test]
async fn interruption_after_normalization_prevents_readmission() {
    for change_epoch in [false, true] {
        let (store, request) = abandoned_store();
        let control = InterruptAfterNormalization {
            store: Arc::clone(&store),
            identity: request.identity().clone(),
            change_epoch,
        };
        let transport = Arc::new(ScriptedTransport::default());
        let result = coordinator(
            Arc::clone(&transport),
            Arc::clone(&store),
            ManualClock::new(100),
            10,
        )
        .advance_with_recovery(request, ChainRecoveryMode::ExactTree, &control)
        .await
        .unwrap();
        assert!(matches!(
            result,
            ChainSubmissionResult::Pending(ChainSubmissionPending::Recovering { .. })
        ));
        assert!(transport.methods().is_empty());
        assert_eq!(
            store
                .record(&control.identity)
                .unwrap()
                .committed_post_reservations(),
            1
        );
    }
}

#[tokio::test]
async fn an_abandoned_reservation_scans_before_retransmitting_in_the_same_exact_pass() {
    let (store, request) = abandoned_store();
    let identity = request.identity().clone();
    let transport = Arc::new(ScriptedTransport::default());
    for response in tree_responses(&[[8; 32]]) {
        transport.queue(Ok(response));
    }
    transport.queue(Ok(accepted()));

    let result = coordinator(
        Arc::clone(&transport),
        Arc::clone(&store),
        ManualClock::new(100),
        10,
    )
    .advance_with_recovery(
        request,
        ChainRecoveryMode::ExactTree,
        &ManualControl::default(),
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        ChainSubmissionResult::Pending(ChainSubmissionPending::Tracking { .. })
    ));
    assert_eq!(transport.methods(), vec!["GET", "GET", "POST"]);
    assert_eq!(
        store
            .record(&identity)
            .unwrap()
            .committed_post_reservations(),
        2
    );
}

#[tokio::test]
async fn an_abandoned_reservation_with_an_incomplete_scan_never_retransmits() {
    let (store, request) = abandoned_store();
    let identity = request.identity().clone();
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Err(ChainTransportError::definitely_unsent(
        "tree unavailable",
    )));

    let failure = coordinator(
        Arc::clone(&transport),
        Arc::clone(&store),
        ManualClock::new(100),
        10,
    )
    .advance_with_recovery(
        request,
        ChainRecoveryMode::ExactTree,
        &ManualControl::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        failure.strongest_state().unwrap().state(),
        ChainSubmissionState::Recovering
    );
    assert!(transport.methods().iter().all(|method| *method == "GET"));
    assert_eq!(
        store
            .record(&identity)
            .unwrap()
            .committed_post_reservations(),
        1
    );
}

#[tokio::test]
async fn an_abandoned_reservation_does_not_reconcile_when_cancelled_or_status_only() {
    for (mode, cancelled) in [
        (ChainRecoveryMode::ExactTree, true),
        (ChainRecoveryMode::StatusOnly, false),
    ] {
        let (store, request) = abandoned_store();
        let identity = request.identity().clone();
        let transport = Arc::new(ScriptedTransport::default());
        let control = ManualControl::default();
        control.cancelled.store(cancelled, Ordering::SeqCst);
        let result = coordinator(
            Arc::clone(&transport),
            Arc::clone(&store),
            ManualClock::new(100),
            10,
        )
        .advance_with_recovery(request, mode, &control)
        .await
        .unwrap();

        assert!(matches!(
            result,
            ChainSubmissionResult::Pending(ChainSubmissionPending::Recovering { .. })
        ));
        assert_eq!(
            store
                .record(&identity)
                .unwrap()
                .committed_post_reservations(),
            1
        );
        assert!(transport.methods().is_empty());
    }
}
