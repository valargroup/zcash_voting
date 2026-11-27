use super::*;

#[tokio::test]
async fn first_block_accepts_omitted_zero_start_index_and_recovers_layout() {
    let (outcome, urls) = scan(&[[8; 32], [3; 32], [4; 32], [7; 32]], None)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        RecoveryScanOutcome::Match {
            final_van_position: 1,
            ref vote_commitment_positions
        } if vote_commitment_positions == &[2]
    ));
    assert_eq!(urls.len(), 2);
    assert!(urls[0].ends_with(&format!("/{}/latest", hex::encode([1; 32]))));
}

#[tokio::test]
async fn complete_no_match_authorizes_only_the_captured_generation_and_candidate() {
    let candidate = CandidateTransactionHash::from_bytes([10; 32]);
    let (outcome, _) = scan(&[[3; 32], [8; 32], [4; 32]], Some(candidate))
        .await
        .unwrap();
    let RecoveryScanOutcome::NoMatch(authorization) = outcome else {
        panic!("nonadjacent leaves must not match")
    };
    assert_eq!(authorization.operation().host_operation_epoch(), 11);
    assert_eq!(authorization.generation_digest().as_bytes(), &[9; 32]);
    assert_eq!(authorization.candidate(), Some(candidate));
}

#[tokio::test]
async fn empty_snapshot_accepts_omitted_zero_metadata_without_a_leaves_request() {
    let (outcome, urls) = scan(&[], None).await.unwrap();
    assert!(matches!(outcome, RecoveryScanOutcome::NoMatch(_)));
    assert_eq!(urls.len(), 1);
}

#[tokio::test]
async fn duplicate_complete_layout_is_ambiguous_not_confirmation() {
    let failure = scan(&[[3; 32], [4; 32], [8; 32], [3; 32], [4; 32]], None)
        .await
        .err()
        .expect("duplicate layout is invalid");
    assert!(matches!(failure, RecoveryScanFailure::AmbiguousLayout(_)));
}

/// A pass without a required height asks for none and ignores the
/// snapshot's `block_height` (contracts-v1 §8), even a malformed one.
#[tokio::test]
async fn a_pass_without_a_required_height_ignores_the_snapshot_block_height() {
    let mut responses = tree_responses(&[[8; 32], [3; 32], [4; 32], [7; 32]]);
    let mut latest: serde_json::Value = serde_json::from_slice(responses[0].body()).unwrap();
    latest["block_height"] = "soon".into();
    responses[0] = ChainHttpResponse::json(200, serde_json::to_vec(&latest).unwrap());

    let (outcome, urls) = scan_responses(responses, None).await.unwrap();

    assert!(matches!(outcome, RecoveryScanOutcome::Match { .. }));
    assert_eq!(urls.len(), 2, "the snapshot and its one page: {urls:?}");
}
