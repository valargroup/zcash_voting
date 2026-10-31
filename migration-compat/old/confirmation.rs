//! Confirm the accepted shares using the real helper's recorded responses.
use super::{hex, transport, CaptureConfig};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use zcash_voting::round::VotingDb;

pub(super) fn track(
    config: &CaptureConfig,
    db: &VotingDb,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let round = &config.round_params.vote_round_id;
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        let pending = db.get_unconfirmed_delegations(round)?;
        if pending.is_empty() {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "helper confirmation deadline exceeded"
        );
        for share in pending {
            let nullifier = hex(&share.nullifier);
            for helper in &share.sent_to_urls {
                let response = transport::request(
                    &format!(
                        "{}/shielded-vote/v1/share-status/{round}/{nullifier}",
                        helper.trim_end_matches('/')
                    ),
                    None,
                )?;
                if response["status"] == "confirmed" {
                    zcash_voting::share::confirm(
                        &db,
                        round,
                        share.bundle_index,
                        share.proposal_id,
                        share.share_index,
                    )?;
                    evidence.push(json!({"kind":"share_confirmation", "bundle_index":share.bundle_index,
                        "proposal_id":share.proposal_id, "share_index":share.share_index, "receipt":response}));
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Ok(())
}
