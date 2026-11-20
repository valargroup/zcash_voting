//! Cast real singleton votes and deliver the old SDK's complete share plans.
use super::{setup::PreparedCapture, transport, CaptureConfig};
use anyhow::Result;
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
            super::delivery::submit(config, db, &recovered, bundle_index, proposal_id, evidence)?;
        }
    }
    Ok(())
}
