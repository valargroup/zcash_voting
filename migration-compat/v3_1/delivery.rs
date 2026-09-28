//! v3.1.0 owns complete share planning, transport journaling, and acceptance.
#[path = "helper_transport.rs"]
mod helper_transport;
use super::CaptureConfig;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use zcash_voting::helper::{
    client::{HelperClient, HelperClientConfig},
    health::HelperHealth,
};
use zcash_voting::{round::VotingDb, vote::CommittedVote};

pub(super) fn client() -> HelperClient {
    HelperClient::with_config(
        std::sync::Arc::new(helper_transport::RecordingTransport::new()),
        HelperHealth::default(),
        HelperClientConfig::default().without_retries(),
    )
}

pub(super) fn submit(
    config: &CaptureConfig,
    db: &VotingDb,
    recovered: &CommittedVote,
    bundle_index: u32,
    proposal_id: u32,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let client = client();
        let fleet = client.preflight_fleet(&config.helper_urls).await?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        let plan = recovered.prepare_share_delivery(db, zcash_voting::share_tracking::ShareDeliveryPlanningParams {
            fleet: &fleet, now_seconds: now, vote_end_time_seconds: config.vote_end_time,
            last_moment_buffer_seconds: Some(7200), proposal_ids: &[1,2,3],
        })?;
        ensure!(plan.share_plans.len() == 16, "capture requires all 16 shares");
        let report = recovered.submit_prepared_shares(db, &client, zcash_voting::share_tracking::ShareDeliverySubmissionParams {
            configured_server_urls: &config.helper_urls, now_seconds: now,
        }, &|| false).await?;
        ensure!(!report.cancelled && report.pending_share_indices.is_empty(), "share delivery incomplete");
        ensure!(report.deliveries.len() == 16 && report.deliveries.iter().all(|d| d.submission.target_count > 0 && d.submission.accepted_urls.len() >= d.submission.target_count && d.submission.ambiguous_urls.is_empty()), "not every share was definitively accepted");
        evidence.push(json!({"kind":"share_delivery", "bundle_index":bundle_index, "proposal_id":proposal_id, "delivered_shares":report.deliveries.len(), "pending_share_indices":report.pending_share_indices, "cancelled":report.cancelled}));
        Ok(())
    })
}
