use super::*;

/// With 15 helpers the race waits for 8 ready helpers, the readiness target,
/// not just the 5 that each share is sent to.
#[tokio::test(start_paused = true)]
async fn preflight_fleet_waits_for_the_readiness_target_not_the_share_target() {
    let transport = Arc::new(MockTransport::default());
    let urls: Vec<String> = (0..15)
        .map(|index| format!("https://helper-{index}.example"))
        .collect();
    for (index, url) in urls.iter().enumerate() {
        let delay = match index {
            0..5 => Duration::ZERO,
            5..8 => Duration::from_secs(4),
            _ => Duration::from_secs(20),
        };
        transport.queue_get_after(
            &format!("{url}/shielded-vote/v1/status"),
            delay,
            json_status("ok"),
        );
    }
    let client = HelperClient::with_config(
        transport,
        HelperHealth::default(),
        HelperClientConfig::default(),
    );
    let started = tokio::time::Instant::now();

    let fleet = client.preflight_fleet(&urls).await.unwrap();

    assert_eq!(fleet.ready_server_count(), 8);
    assert_eq!(started.elapsed(), Duration::from_secs(4));
    assert_eq!(&fleet.ranked_server_urls()[8..], &urls[8..]);
}
