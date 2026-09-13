//! Cross-release history oracle. This exact source is compiled by both SDKs.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::Path;
use zcash_voting::{round::VotingDb, session, wire::RoundPlanView};

/// Reads the wallet-facing history without executing recovery or making requests.
/// The wallet path must already have a sidecar; a missing input is never created.
pub fn read(wallet: &Path, wallet_id: &str, round_id: &str, proposals: &[u32]) -> Result<Value> {
    ensure!(
        VotingDb::wallet_sidecar_path(wallet).is_file(),
        "missing historical sidecar"
    );
    let database = VotingDb::open_wallet_sidecar(wallet, wallet_id)?;
    let rounds = database
        .list_rounds()?
        .into_iter()
        .map(|round| {
            json!({
                "round_id": round.round_id, "wallet_id": round.wallet_id,
                "phase": round.phase as i32, "network": format!("{:?}", round.network),
                "snapshot_height": round.snapshot_height, "created_at": round.created_at,
            })
        })
        .collect::<Vec<_>>();
    let state = database.get_round_state(round_id)?;
    let plan = session::resume_plan(&database, round_id, proposals)?;
    let display = plan.completed_vote_display.as_ref().map(|display| json!({
        "choices": display.choices.iter().map(|choice| json!({"proposal_id": choice.proposal_id, "choice": choice.choice})).collect::<Vec<_>>(),
        "voted_at": display.voted_at,
    }));
    let completed = plan.completed_for_display;
    let view = RoundPlanView::try_from(plan)?;
    let wire = serde_json::to_value(view)?;
    ensure!(
        wire["completed_vote_display"] == json!(display),
        "native/wire history disagreement"
    );
    let votes = serde_json::to_value(database.get_votes(round_id)?)?;
    let intents = database
        .ballot_intents(round_id)?
        .into_iter()
        .map(|(proposal, decision)| {
            let choice = match decision {
                session::Decision::Choice(choice) => Some(choice),
                session::Decision::Skipped => None,
            };
            json!({"proposal_id": proposal, "choice": choice})
        })
        .collect::<Vec<_>>();
    let other = VotingDb::open_wallet_sidecar(wallet, "migration-compat-unrelated-wallet")?;
    ensure!(
        other.list_rounds()?.is_empty(),
        "wallet scope leaked rounds"
    );
    ensure!(
        other.get_votes(round_id)?.is_empty(),
        "wallet scope leaked votes"
    );
    Ok(json!({
        "rounds": rounds,
        "state": {"round_id": state.round_id, "phase": state.phase as i32, "network": format!("{:?}",state.network),
            "snapshot_height": state.snapshot_height, "proof_generated": state.proof_generated,
            "hotkey_address": state.hotkey_address, "delegated_weight": state.delegated_weight},
        "votes": votes, "intents": intents, "completed_for_display": completed,
        "completed_vote_display": display,
    }))
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--fresh" {
        VotingDb::open_wallet_sidecar(Path::new(&args[2]), "migration-fresh")?;
        return Ok(());
    }
    if args.len() == 5 && args[1] == "--new-round" {
        let database = VotingDb::open_wallet_sidecar(Path::new(&args[2]), &args[3])?;
        let mut params: zcash_voting::VotingRoundParams =
            serde_json::from_slice(&std::fs::read(&args[4])?)?;
        params.vote_round_id = "0f".repeat(32);
        database.create_round(zcash_voting::Network::Testnet, &params, None)?;
        return Ok(());
    }
    ensure!(
        args.len() == 5,
        "usage: migration_history WALLET_PATH WALLET_ID ROUND_ID PROPOSALS_CSV"
    );
    let proposals = args[4]
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<u32>, _>>()?;
    let history = read(Path::new(&args[1]), &args[2], &args[3], &proposals)?;
    println!("{}", serde_json::to_string(&history)?);
    Ok(())
}
