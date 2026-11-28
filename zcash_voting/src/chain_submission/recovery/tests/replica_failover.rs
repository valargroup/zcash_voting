use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn endpoints() -> Vec<String> {
    vec![
        "https://chain-a.example".to_string(),
        "https://chain-b.example".to_string(),
    ]
}

#[tokio::test]
async fn transport_failure_before_snapshot_fails_over_to_next_replica() {
    let mut responses = vec![Err(ChainTransportError::definitely_unsent(
        "replica unavailable",
    ))];
    responses.extend(tree_responses(&[[3; 32], [4; 32]]).into_iter().map(Ok));

    let (outcome, urls) = scan_results(endpoints(), responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::Match { .. }));
    assert_eq!(urls.len(), 3);
    assert!(urls[0].starts_with("https://chain-a.example/"));
    assert!(urls[1].starts_with("https://chain-b.example/"));
    assert!(urls[1].ends_with("/latest"));
    assert!(urls[2].starts_with("https://chain-b.example/"));
}

#[tokio::test]
async fn mid_scan_failure_restarts_snapshot_and_cursor_on_next_replica() {
    let first_replica_leaves = [[8; 32], [9; 32]];
    let mut complete_frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
    for leaf in &first_replica_leaves {
        assert!(complete_frontier.append(MerkleHashVote::from_bytes(leaf).unwrap()));
    }
    let mut partial_frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
    assert!(partial_frontier.append(MerkleHashVote::from_bytes(&first_replica_leaves[0]).unwrap()));
    let responses = vec![
        Ok(ChainHttpResponse::json(
            200,
            serde_json::to_vec(&serde_json::json!({
                "tree": {
                    "next_index": 2,
                    "root": BASE64_STANDARD.encode(complete_frontier.root().to_bytes()),
                    "height": 2
                }
            }))
            .unwrap(),
        )),
        Ok(ChainHttpResponse::json(
            200,
            serde_json::to_vec(&serde_json::json!({
                "blocks": [{
                    "height": 1,
                    "start_index": 0,
                    "leaves": [BASE64_STANDARD.encode(first_replica_leaves[0])],
                    "root": BASE64_STANDARD.encode(partial_frontier.root().to_bytes())
                }],
                "next_from_height": 1
            }))
            .unwrap(),
        )),
        Err(ChainTransportError::possibly_dispatched(
            "replica interrupted",
        )),
    ]
    .into_iter()
    .chain(tree_responses(&[[3; 32], [4; 32]]).into_iter().map(Ok))
    .collect();

    let (outcome, urls) = scan_results(endpoints(), responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::Match { .. }));
    assert_eq!(urls.len(), 5);
    assert!(urls[1].contains("from_height=0&to_height=2"));
    assert!(urls[2].contains("from_height=1&to_height=2"));
    assert!(urls[3].starts_with("https://chain-b.example/"));
    assert!(urls[3].ends_with("/latest"));
    assert!(urls[4].starts_with("https://chain-b.example/"));
    assert!(urls[4].contains("from_height=0&to_height=1"));
}

#[tokio::test]
async fn malformed_replica_is_skipped_without_mixing_its_snapshot() {
    let responses = vec![Ok(ChainHttpResponse::json(200, b"{".to_vec()))]
        .into_iter()
        .chain(tree_responses(&[[8; 32], [9; 32]]).into_iter().map(Ok))
        .collect();

    let (outcome, urls) = scan_results(endpoints(), responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::NoMatch(_)));
    assert_eq!(urls.len(), 3);
    assert!(urls[0].starts_with("https://chain-a.example/"));
    assert!(urls[1].starts_with("https://chain-b.example/"));
    assert!(urls[1].ends_with("/latest"));
}

#[tokio::test]
async fn contradictory_replica_is_skipped_before_authorization() {
    let leaves = [[8; 32], [9; 32]];
    let mut frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
    for leaf in &leaves {
        assert!(frontier.append(MerkleHashVote::from_bytes(leaf).unwrap()));
    }
    let root = BASE64_STANDARD.encode(frontier.root().to_bytes());
    let responses = vec![
        Ok(ChainHttpResponse::json(
            200,
            serde_json::to_vec(&serde_json::json!({
                "tree": { "next_index": 2, "root": root, "height": 2 }
            }))
            .unwrap(),
        )),
        Ok(ChainHttpResponse::json(
            200,
            serde_json::to_vec(&serde_json::json!({
                "blocks": [
                    {
                        "height": 1,
                        "start_index": 0,
                        "leaves": leaves.map(|leaf| BASE64_STANDARD.encode(leaf)),
                        "root": root
                    },
                    {
                        "height": 2,
                        "leaves": [],
                        "root": BASE64_STANDARD.encode([7; 32])
                    }
                ],
                "next_from_height": 0
            }))
            .unwrap(),
        )),
        Ok(ChainHttpResponse::json(200, br#"{"tree":{}}"#.to_vec())),
    ];

    let (outcome, urls) = scan_results(endpoints(), responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::NoMatch(_)));
    assert_eq!(urls.len(), 3);
    assert!(urls[2].starts_with("https://chain-b.example/"));
    assert!(urls[2].ends_with("/latest"));
}

#[tokio::test]
async fn duplicate_layout_does_not_fail_over_to_stale_no_match() {
    let mut responses = tree_responses(&[[3; 32], [4; 32], [8; 32], [3; 32], [4; 32]])
        .into_iter()
        .map(Ok)
        .collect::<Vec<_>>();
    responses.push(Ok(ChainHttpResponse::json(200, br#"{"tree":{}}"#.to_vec())));

    let failure = scan_results(
        endpoints(),
        responses,
        Some(CandidateTransactionHash::from_bytes([10; 32])),
    )
    .await
    .err()
    .expect("duplicate ledger evidence must stop replica failover");

    assert!(matches!(failure, RecoveryScanFailure::AmbiguousLayout(_)));
}

#[tokio::test]
async fn exhausting_all_replicas_produces_no_authorization() {
    let responses = vec![
        Err(ChainTransportError::definitely_unsent(
            "first replica unavailable",
        )),
        Err(ChainTransportError::definitely_unsent(
            "second replica unavailable",
        )),
    ];

    let failure = scan_results(
        endpoints(),
        responses,
        Some(CandidateTransactionHash::from_bytes([10; 32])),
    )
    .await
    .err()
    .expect("replica exhaustion must not authorize a retry");

    assert!(matches!(failure, RecoveryScanFailure::Transport(_)));
}

#[tokio::test]
async fn complete_no_match_stops_before_later_replicas() {
    let (outcome, urls) = scan_results(
        endpoints(),
        vec![Ok(ChainHttpResponse::json(200, br#"{"tree":{}}"#.to_vec()))],
        None,
    )
    .await
    .unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::NoMatch(_)));
    assert_eq!(urls.len(), 1);
    assert!(urls[0].starts_with("https://chain-a.example/"));
}

#[tokio::test]
async fn cancellation_between_replicas_stops_failover() {
    let interruption_checks = AtomicUsize::new(0);
    let failure = scan_results_with_interruption(
        endpoints(),
        vec![Err(ChainTransportError::definitely_unsent(
            "first replica unavailable",
        ))],
        None,
        || interruption_checks.fetch_add(1, Ordering::Relaxed) >= 2,
    )
    .await
    .err()
    .expect("cancellation must stop before the second replica");

    assert!(matches!(failure, RecoveryScanFailure::Interrupted));
    assert_eq!(interruption_checks.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn response_byte_budget_is_shared_across_replicas() {
    let malformed = b"{";
    let second_response = br#"{"tree":{}}"#;
    let transport = ScriptedTreeTransport::new(vec![
        ChainHttpResponse::json(200, malformed.to_vec()),
        ChainHttpResponse::json(200, second_response.to_vec()),
    ]);
    let mut budget = RecoveryPassBudget {
        deadline: tokio::time::Instant::now() + RECOVERY_PASS_TIMEOUT,
        leaf_request_count: 0,
        response_bytes: MAX_RECOVERY_TOTAL_BYTES - malformed.len() as u64,
    };

    let first_failure = get_json::<_, LatestResponse>(
        &transport,
        "https://chain-a.example/latest".to_string(),
        0,
        &mut budget,
        &|| false,
        &crate::ObservationScope::disabled(),
    )
    .await
    .err()
    .expect("the first replica response is malformed");
    assert!(matches!(first_failure, RecoveryScanFailure::Invalid(_)));

    let second_failure = get_json::<_, LatestResponse>(
        &transport,
        "https://chain-b.example/latest".to_string(),
        1,
        &mut budget,
        &|| false,
        &crate::ObservationScope::disabled(),
    )
    .await
    .err()
    .expect("the second replica must inherit the spent byte budget");
    assert!(matches!(second_failure, RecoveryScanFailure::Invalid(_)));
    assert!(budget.response_bytes > MAX_RECOVERY_TOTAL_BYTES);
}

#[test]
fn leaf_request_budget_is_shared_across_replicas() {
    let mut budget = RecoveryPassBudget {
        deadline: tokio::time::Instant::now() + RECOVERY_PASS_TIMEOUT,
        leaf_request_count: MAX_RECOVERY_LEAF_REQUESTS - 1,
        response_bytes: 0,
    };

    budget
        .begin_leaf_request()
        .expect("the final request belongs to the first replica");
    let failure = budget
        .begin_leaf_request()
        .expect_err("the next replica must not receive a fresh request budget");

    assert!(matches!(failure, RecoveryScanFailure::Invalid(_)));
}

/// A JSON answer giving a replica's latest block height, as both the height
/// route and the validator-set route carry it.
fn replica_height(height: u64) -> ChainHttpResponse {
    ChainHttpResponse::json(
        200,
        format!(r#"{{"height":"{height}","block_height":"{height}"}}"#).into_bytes(),
    )
}

/// One replica's pass with a required height: its height read before the
/// snapshot, then the snapshot of `leaves`.
fn replica_pass(height: u64, leaves: &[[u8; 32]]) -> Vec<ChainHttpResponse> {
    std::iter::once(replica_height(height))
        .chain(tree_responses(leaves))
        .collect()
}

#[tokio::test]
async fn no_replica_at_the_required_height_ends_the_pass_lagging() {
    // The second replica fails outright, its height read and its snapshot
    // alike; the first one's lag still decides.
    let unavailable = || {
        Err(ChainTransportError::definitely_unsent(
            "replica unavailable",
        ))
    };
    let responses = [replica_height(99), replica_height(100)]
        .into_iter()
        .chain(replica_pass(99, &[[8; 32]]))
        .map(Ok)
        .chain([unavailable(), unavailable()])
        .collect();

    let (outcome, _) = scan_results_requiring(
        endpoints(),
        responses,
        None,
        RequiredReplicaHeight::HighestReported,
        || false,
    )
    .await
    .unwrap();

    assert!(matches!(
        outcome,
        RecoveryScanOutcome::Lagging {
            required_height: 100
        }
    ));
}

#[tokio::test]
async fn a_match_counts_from_a_replica_below_the_required_height() {
    let responses = [replica_height(1), replica_height(100)]
        .into_iter()
        .chain(replica_pass(1, &[[3; 32], [4; 32]]))
        .map(Ok)
        .collect();

    let (outcome, _) = scan_results_requiring(
        endpoints(),
        responses,
        None,
        RequiredReplicaHeight::HighestReported,
        || false,
    )
    .await
    .unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::Match { .. }));
}

/// A pass with a required height judges a snapshot by its own
/// `block_height` (contracts-v1 §8) over the height read before it, and
/// refuses one that is not a decimal height at or above the tree's.
#[tokio::test]
async fn a_required_pass_judges_the_snapshot_by_its_block_height() {
    let pass = |read_before: u64, block_height: &str| {
        let mut responses = replica_pass(read_before, &[[8; 32]]);
        let mut latest: serde_json::Value = serde_json::from_slice(responses[1].body()).unwrap();
        latest["block_height"] = block_height.into();
        responses[1] = ChainHttpResponse::json(200, serde_json::to_vec(&latest).unwrap());
        responses
    };
    let scan = |first: Vec<ChainHttpResponse>, second: Vec<ChainHttpResponse>| {
        let responses = [replica_height(100), replica_height(100)]
            .into_iter()
            .chain(first)
            .chain(second)
            .map(Ok)
            .collect();
        scan_results_requiring(
            endpoints(),
            responses,
            None,
            RequiredReplicaHeight::HighestReported,
            || false,
        )
    };

    let (outcome, urls) = scan(pass(150, "99"), pass(50, "100")).await.unwrap();
    assert!(matches!(outcome, RecoveryScanOutcome::NoMatch(_)));
    assert!(urls.last().unwrap().starts_with("https://chain-b.example/"));

    for malformed in ["soon", "0"] {
        let failure = scan(pass(150, malformed), pass(150, malformed))
            .await
            .err()
            .expect("a malformed snapshot height is refused");
        assert!(
            matches!(failure, RecoveryScanFailure::Invalid(_)),
            "{malformed}"
        );
    }
}

#[tokio::test]
async fn the_highest_reported_height_is_read_from_every_endpoint_first() {
    let responses = [replica_height(120), replica_height(150)]
        .into_iter()
        .chain(replica_pass(120, &[[8; 32]]))
        .chain(replica_pass(150, &[[8; 32]]))
        .map(Ok)
        .collect();

    let (outcome, urls) = scan_results_requiring(
        endpoints(),
        responses,
        None,
        RequiredReplicaHeight::HighestReported,
        || false,
    )
    .await
    .unwrap();

    assert!(
        matches!(outcome, RecoveryScanOutcome::NoMatch(_)),
        "the replica at the highest height counts"
    );
    assert!(urls[0].ends_with("/shielded-vote/v1/height"));
    assert!(urls[1].ends_with("/shielded-vote/v1/height"));
    assert!(urls[5].starts_with("https://chain-b.example/"));
}
