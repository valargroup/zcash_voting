use super::*;
use crate::helper::tests::ingress::{IngressRoute, Reply};

fn receipt_client(replies: impl IntoIterator<Item = Reply>) -> (HelperClient, Arc<IngressRoute>) {
    let route = Arc::new(IngressRoute::new(replies));
    // Exercise the Arc forwarding implementation as well as HyperTransport.
    let transport = Arc::new(Arc::new(crate::HyperTransport::with_shared_route(
        route.clone(),
    )));
    (HelperClient::new(transport, HelperHealth::default()), route)
}

#[tokio::test(start_paused = true)]
async fn helper_receipts_retry_with_fresh_tokens_and_existing_backoff() {
    let (client, route) = receipt_client([Reply::Receipt, Reply::Receipt, Reply::Accepted]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        client
            .submit_share(helper(), &valid_share_json(), 10, &never_cancel())
            .await
            .unwrap(),
        ShareSubmissionStatus::Queued
    );
    assert_eq!(started.elapsed(), Duration::from_millis(800));
    let tokens = route.tokens.lock().unwrap();
    assert_eq!(tokens.len(), 3);
    assert!(tokens
        .iter()
        .all(|token| hex::decode(token).unwrap().len() == 32));
    assert!(tokens.windows(2).all(|pair| pair[0] != pair[1]));
}

#[tokio::test(start_paused = true)]
async fn helper_receipts_obey_initial_budget_and_recovery_single_attempt() {
    let (client, route) = receipt_client([Reply::Receipt, Reply::Receipt, Reply::Receipt]);
    let failure = client
        .submit_share(helper(), &valid_share_json(), 10, &never_cancel())
        .await
        .unwrap_err();
    assert!(matches!(failure, HelperError::NotEnqueuedByServer));
    assert!(!failure.is_ambiguous());
    assert_eq!(route.tokens.lock().unwrap().len(), 3);

    let (client, route) = receipt_client([Reply::Receipt]);
    assert!(matches!(
        client
            .resubmit_share(helper(), &valid_share_json(), 10, &never_cancel())
            .await,
        Err(HelperError::NotEnqueuedByServer)
    ));
    assert_eq!(route.tokens.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn invalid_helper_receipts_never_authorize_retry() {
    for reply in [
        Reply::Mismatched,
        Reply::UnknownVersion,
        Reply::ExtraField,
        Reply::MissingToken,
        Reply::WrongContentType,
        Reply::Oversized,
        Reply::Truncated,
        Reply::GenericTimeout,
    ] {
        let (client, route) = receipt_client([reply]);
        let failure = client
            .submit_share(helper(), &valid_share_json(), 10, &never_cancel())
            .await
            .unwrap_err();
        assert!(!matches!(failure, HelperError::NotEnqueuedByServer));
        assert_eq!(route.tokens.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn lost_helper_receipt_stays_ambiguous_and_stops_retries() {
    let (client, route) = receipt_client([Reply::Receipt, Reply::Lost, Reply::Receipt]);
    let failure = client
        .submit_share(helper(), &valid_share_json(), 10, &never_cancel())
        .await
        .unwrap_err();
    assert!(failure.is_ambiguous());
    assert_eq!(route.tokens.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn helper_receipt_deadline_returns_definite_failure_without_waiting() {
    let (client, route) = receipt_client([Reply::Receipt]);
    let started = tokio::time::Instant::now();
    let failure = client
        .submit_share_with_timeout(
            helper(),
            &valid_share_json(),
            10,
            &never_cancel(),
            Duration::from_secs(1),
            Some(started + Duration::from_millis(100)),
        )
        .await
        .unwrap_err();
    assert!(matches!(failure, HelperError::NotEnqueuedByServer));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(route.tokens.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn helper_receipt_cancellation_stops_before_retry() {
    let (client, route) = receipt_client([Reply::Receipt]);
    let cancel = || !route.tokens.lock().unwrap().is_empty();
    assert!(matches!(
        client
            .submit_share(helper(), &valid_share_json(), 10, &cancel)
            .await,
        Err(HelperError::Cancelled)
    ));
    assert_eq!(route.tokens.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn legacy_helper_transport_keeps_generic_timeout_handling() {
    let transport = Arc::new(MockTransport::default());
    transport.queue_post(&post_url(), http_status(408));
    let client = client_with(transport.clone());
    assert!(matches!(
        client
            .submit_share(helper(), &valid_share_json(), 10, &never_cancel())
            .await,
        Err(HelperError::Status { status: 408 })
    ));
    assert_eq!(transport.call_count(&post_url()), 1);
}
