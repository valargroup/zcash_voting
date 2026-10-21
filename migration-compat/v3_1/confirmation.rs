//! v3.1.0 confirms exact durable generations through its public polling API.
use super::{delivery, CaptureConfig};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zcash_voting::{
    round::VotingDb,
    share_tracking::{confirm_pending_share, ShareConfirmationParams, ShareKey},
};
pub(super) fn track(
    config: &CaptureConfig,
    db: &VotingDb,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let client = delivery::client();
        let round = &config.round_params.vote_round_id;
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            let pending = db.get_unconfirmed_delegations(round)?;
            if pending.is_empty() { return Ok(()); }
            ensure!(Instant::now() < deadline, "helper confirmation deadline exceeded");
            for share in pending {
                let key = ShareKey { bundle_index: share.bundle_index, proposal_id: share.proposal_id, share_index: share.share_index };
                let report = confirm_pending_share(db, &ShareConfirmationParams { round_id: round, share:key,
                    configured_server_urls: &config.helper_urls, now_seconds:SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() }, &client, &|| false).await?;
                if report.confirmed { evidence.push(json!({"kind":"share_confirmation", "bundle_index":key.bundle_index, "proposal_id":key.proposal_id, "share_index":key.share_index})); }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    })
}
