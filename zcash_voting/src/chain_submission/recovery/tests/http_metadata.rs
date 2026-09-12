use super::*;

struct DelayedTreeTransport {
    delay: Duration,
}

impl ChainTransport for DelayedTreeTransport {
    fn chain_get<'a>(&'a self, _request: ChainHttpRequest) -> ChainTransportFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Ok(ChainHttpResponse::json(
                200,
                br#"{"tree":{"next_index":0,"height":0}}"#.to_vec(),
            ))
        })
    }

    fn chain_post_json<'a>(
        &'a self,
        _request: ChainHttpRequest,
        _json: Vec<u8>,
    ) -> ChainTransportFuture<'a> {
        Box::pin(async { panic!("scanner must not POST") })
    }
}

#[tokio::test]
async fn json_media_type_accepts_case_variants_and_delimiter_whitespace() {
    let responses = tree_responses(&[[3; 32], [4; 32]])
        .into_iter()
        .map(|response| {
            ChainHttpResponse::new(
                response.status(),
                response.body().to_vec(),
                Some("Application/JSON ; charset=utf-8".to_string()),
                response.headers().to_vec(),
            )
        })
        .collect();

    let (outcome, _) = scan_responses(responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::Match { .. }));
}

#[tokio::test(start_paused = true)]
async fn pass_deadline_cancels_an_in_flight_request() {
    let transport = DelayedTreeTransport {
        delay: Duration::from_secs(10),
    };
    let recovery_deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    let failure = get_json_with_size::<_, LatestResponse>(
        &transport,
        "https://chain.example/latest".to_string(),
        recovery_deadline,
        &|| false,
        &crate::ObservationScope::disabled(),
    )
    .await
    .err()
    .expect("the pass deadline must bound a request already in flight");

    assert!(matches!(failure, RecoveryScanFailure::Invalid(_)));
}
