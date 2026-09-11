//! Attempt-bound ingress receipts and preservation of earlier dispatch evidence.
use super::*;

struct TimeoutIngress {
    calls: AtomicUsize,
    lose_first_response: bool,
    tokens: Mutex<Vec<String>>,
}

impl ChainTransport for TimeoutIngress {
    fn chain_get<'a>(&'a self, _: ChainHttpRequest) -> ChainTransportFuture<'a> {
        Box::pin(async { panic!("a known non-broadcast attempt needs no lookup") })
    }
    fn chain_post_json<'a>(
        &'a self,
        request: ChainHttpRequest,
        _: Vec<u8>,
    ) -> ChainTransportFuture<'a> {
        Box::pin(async move {
            let token = request
                .headers()
                .iter()
                .find(|(name, _)| name == "x-vote-ingress-attempt-v1")
                .unwrap()
                .1
                .clone();
            self.tokens.lock().unwrap().push(token.clone());
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 && self.lose_first_response {
                return Err(ChainTransportError::possibly_dispatched(
                    "response lost after broadcast",
                ));
            }
            Ok(ChainHttpResponse::json(408, serde_json::to_vec(&serde_json::json!({"error":{
                "version":1,"code":"request_body_timeout","dispatch":"not_started","attempt":token
            }})).unwrap()))
        })
    }
}

#[tokio::test]
async fn ingress_timeouts_respect_budget_and_preserve_previous_ambiguity() {
    for lose_first_response in [false, true] {
        let identity = identity(1, 0);
        let store = Arc::new(InMemoryChainSubmissionStore::default());
        store.seed_derivation(derived(identity.clone(), 1));
        let transport = Arc::new(TimeoutIngress {
            calls: AtomicUsize::new(0),
            lose_first_response,
            tokens: Mutex::new(vec![]),
        });
        let protocol = ChainProtocolClient::new(
            transport.clone(),
            Network::Testnet,
            &["https://chain.example".into()],
        )
        .unwrap();
        let coordinator = ChainSubmissionCoordinator::new(
            protocol,
            store.clone(),
            ManualClock::new(100),
            CoordinatorPolicy::new(
                Duration::from_secs(10),
                3,
                vec![Duration::from_millis(20); 2],
            )
            .unwrap(),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let failure = coordinator
            .advance(
                StoreAdvancementRequest::vote(identity.clone()),
                &ManualControl::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.kind(), ChainSubmissionFailureKind::Transport);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
        assert!(started.elapsed() >= Duration::from_millis(40));
        let tokens = transport.tokens.lock().unwrap();
        assert!(tokens.iter().all(|token| token.len() == 64));
        assert!(tokens.windows(2).all(|pair| pair[0] != pair[1]));
        if lose_first_response {
            assert_eq!(
                store.record(&identity).unwrap().durable_state(),
                ChainSubmissionState::Recovering
            );
        } else {
            assert!(store.record(&identity).is_none());
        }
    }
}

#[tokio::test]
async fn cancellation_during_ingress_timeout_backoff_does_not_retry() {
    let identity = identity(1, 0);
    let store = Arc::new(InMemoryChainSubmissionStore::default());
    store.seed_derivation(derived(identity.clone(), 1));
    let transport = Arc::new(TimeoutIngress {
        calls: AtomicUsize::new(0),
        lose_first_response: false,
        tokens: Mutex::new(vec![]),
    });
    let protocol = ChainProtocolClient::new(
        transport.clone(),
        Network::Testnet,
        &["https://chain.example".into()],
    )
    .unwrap();
    let coordinator = ChainSubmissionCoordinator::new(
        protocol,
        store.clone(),
        ManualClock::new(100),
        CoordinatorPolicy::new(Duration::from_secs(10), 2, vec![Duration::from_secs(2)]).unwrap(),
    )
    .unwrap();
    let control = ManualControl::default();
    let advance = coordinator.advance(StoreAdvancementRequest::vote(identity.clone()), &control);
    let cancel = async {
        while transport.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        control.cancelled.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(advance, cancel);
    assert_eq!(result.unwrap(), ChainSubmissionResult::Cancelled);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(store.record(&identity).is_none());
}
