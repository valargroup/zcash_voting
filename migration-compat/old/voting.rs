//! Cast real singleton votes and deliver the old SDK's complete share plans.
use super::{setup::PreparedCapture, transport, CaptureConfig};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use zcash_voting::{
    vote::{CommittedVote, DraftVote, VoteSigner},
    NoopProgressReporter,
};

pub(super) fn cast_and_deliver(
    config: &CaptureConfig,
    prepared: &PreparedCapture,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let db = &prepared.database;
    let hotkey = &prepared.hotkey;
    let round = &config.round_params.vote_round_id;
    let count = u32::try_from(prepared.bundles.len())?;
    for bundle_index in 0..count {
        for (proposal_id, choice, num_options) in [(1, 0, 2), (2, 1, 3)] {
            eprintln!("capture: proving vote bundle {bundle_index}, proposal {proposal_id}");
            let height = zcash_voting::precompute::sync_vote_tree(&db, round, &config.vote_server)?;
            let witness = zcash_voting::precompute::van_witness(&db, round, bundle_index, height)?;
            let committed = CommittedVote::commit(
                &db,
                round,
                bundle_index,
                &DraftVote {
                    proposal_id,
                    choice,
                    num_options,
                    vc_tree_position: 0,
                    single_share: false,
                },
                &witness,
                VoteSigner::hotkey(&hotkey),
                &NoopProgressReporter,
            )?;
            let hash = transport::submit(
                &config.vote_server,
                "cast-vote",
                &committed.signed_commitment(&db)?.to_wire_json()?,
            )?;
            let (receipt, events) = transport::confirm(&config.chain_rpc, &hash)?;
            zcash_voting::confirmation::confirm_vote_submission(
                &db,
                round,
                bundle_index,
                proposal_id,
                &hash,
                &events,
            )?;
            evidence.push(json!({"kind":"vote", "bundle_index":bundle_index, "proposal_id":proposal_id, "receipt":receipt}));
            let recovered = CommittedVote::recover(&db, round, bundle_index, proposal_id)?;
            // Use the release's complete-commitment planner. This capture exercises
            // its supported expedited (last-moment) policy, with one real helper.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let required = zcash_voting::share_policy::share_submission_random_bytes_required(
                recovered.share_payloads().len(),
                config.helper_urls.len(),
                now,
                config.vote_end_time,
                Some(7200),
                false,
            );
            let mut timing_entropy = vec![0; required.submit_at_random_bytes];
            let mut server_entropy = vec![0; required.server_random_bytes];
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut timing_entropy);
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut server_entropy);
            let plans = zcash_voting::share_policy::plan_share_submissions(
                recovered.share_payloads().len(),
                &config.helper_urls,
                now,
                config.vote_end_time,
                Some(7200),
                false,
                &timing_entropy,
                &server_entropy,
            )?;
            for (payload, plan) in recovered.share_payloads().iter().zip(plans) {
                let index = payload.enc_share.share_index;
                let helper = &plan.target_servers[0];
                let response = transport::request(
                    &format!("{}/shielded-vote/v1/shares", helper.trim_end_matches('/')),
                    Some(&payload.to_wire_json(None, plan.submit_at)?),
                )?;
                ensure!(
                    matches!(response["status"].as_str(), Some("queued" | "duplicate")),
                    "helper did not accept share"
                );
                zcash_voting::share::record(
                    &db,
                    round,
                    bundle_index,
                    proposal_id,
                    index,
                    std::slice::from_ref(helper),
                    plan.submit_at,
                )?;
                evidence.push(
                    json!({"kind":"share", "bundle_index":bundle_index, "proposal_id":proposal_id,
                    "share_index":index, "helper":helper, "receipt":response}),
                );
            }
        }
    }
    Ok(())
}
