use super::*;
use crate::helper::tests::ingress::{IngressRoute, Reply};

#[tokio::test(start_paused = true)]
async fn exhausted_helper_receipts_clear_fresh_reservation_without_acceptance() {
    let configured = helpers(1);
    let db = db_with_recoverable_vote();
    let route = Arc::new(IngressRoute::new([
        Reply::Receipt,
        Reply::Receipt,
        Reply::Receipt,
    ]));
    let client = HelperClient::new(
        Arc::new(crate::HyperTransport::with_shared_route(route.clone())),
        HelperHealth::default(),
    );
    let report = submit_share_to_helpers(
        &db,
        &client,
        &initial_submission(&configured),
        &never_cancel(),
    )
    .await
    .unwrap();
    assert!(report.accepted_urls.is_empty());
    assert!(report.ambiguous_urls.is_empty());
    assert_eq!(route.tokens.lock().unwrap().len(), 3);
    let stored = only_share(&db);
    assert!(stored.sent_to_urls.is_empty());
    assert!(stored.ambiguous_urls.is_empty());
    assert!(stored.attempting_urls.is_empty());
}

#[tokio::test(start_paused = true)]
async fn helper_receipt_preserves_prior_ambiguity_after_reopen() {
    let path =
        std::env::temp_dir().join(format!("helper-receipt-{}.sqlite", rand::random::<u64>()));
    let path = path.to_str().unwrap();
    let db = VotingDb::open(path).unwrap();
    db.set_wallet_id(WALLET_ID);
    seed_recoverable_vote_for_wallet(&db, WALLET_ID);
    let submission = ShareSubmissionReport {
        accepted_urls: vec![],
        ambiguous_urls: vec![helper(1)],
        target_count: 1,
    };
    share::record_delivery(
        &db,
        &share::ShareDeliveryRecordParams {
            round_id: ROUND_ID,
            bundle_index: 0,
            proposal_id: 1,
            share_index: 0,
            submission: &submission,
            submit_at: SUBMIT_AT,
        },
    )
    .unwrap();
    drop(db);

    let db = VotingDb::open(path).unwrap();
    db.set_wallet_id(WALLET_ID);
    let configured = helpers(1);
    let route = Arc::new(IngressRoute::new([Reply::Receipt]));
    let client = HelperClient::new(
        Arc::new(crate::HyperTransport::with_shared_route(route.clone())),
        HelperHealth::default(),
    );
    let report = track_pending_shares(
        &db,
        &params(&configured, overdue(), &zero_bytes),
        &client,
        &never_cancel(),
    )
    .await
    .unwrap();
    assert!(report.resubmitted.is_empty());
    assert_eq!(route.tokens.lock().unwrap().len(), 1);
    assert_eq!(only_share(&db).ambiguous_urls, vec![helper(1)]);
    assert!(only_share(&db).sent_to_urls.is_empty());
    drop(db);

    let db = VotingDb::open(path).unwrap();
    db.set_wallet_id(WALLET_ID);
    assert_eq!(only_share(&db).ambiguous_urls, vec![helper(1)]);
    assert!(only_share(&db).sent_to_urls.is_empty());
    drop(db);
    std::fs::remove_file(path).unwrap();
}
