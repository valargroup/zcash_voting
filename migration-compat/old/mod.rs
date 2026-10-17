//! Host adapter compiled only against the selected unmodified SDK release.
mod backend;
mod confirmation;
mod delegation;
mod delivery;
mod setup;
mod transport;
mod verification;
mod voting;
mod wallet_sync;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use zcash_voting::round::VotingDb;

#[derive(Deserialize)]
struct CaptureConfig {
    artifact_dir: PathBuf,
    round_params: zcash_voting::VotingRoundParams,
    chain_rpc: String,
    chain_id: String,
    vote_server: String,
    pir_url: String,
    helper_urls: Vec<String>,
    lightwalletd: String,
    scan_from: u64,
    pir_layout: zcash_voting::config::PirLayout,
    vote_end_time: u64,
}

pub fn run() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: migration_capture CONFIG")?;
    if path == "verify-delegation" {
        return verification::verify(std::path::Path::new(
            &std::env::args().nth(2).context("missing sidecar")?,
        ));
    }
    if path == "preflight" {
        let deployment = transport::request("https://raw.githubusercontent.com/valargroup/token-holder-voting-config/main/stage/dynamic-voting-config.json", None)?;
        let layout = serde_json::from_value(deployment["pir_layout"].clone())?;
        let endpoint = deployment["pir_endpoints"][0]["url"]
            .as_str()
            .context("no staging PIR")?;
        let _pir = zcash_voting::connect_pir_blocking(
            layout,
            endpoint,
            std::sync::Arc::new(zcash_voting::HyperTransport::new()),
        )?;
        println!("Released SDK PIR handshake passed");
        return Ok(());
    }
    let config: CaptureConfig = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        !config.artifact_dir.exists(),
        "capture directory already exists; original evidence is immutable"
    );
    ensure!(
        config.helper_urls.len() == 1,
        "capture profile requires one helper"
    );
    let status = transport::request(
        &format!("{}/status", config.chain_rpc.trim_end_matches('/')),
        None,
    )?;
    ensure!(
        config.chain_id != "zvote-1" && status["result"]["node_info"]["network"] == config.chain_id,
        "capture requires the configured test chain; production is forbidden"
    );
    let seed_hex = zeroize::Zeroizing::new(
        std::env::var("MIGRATION_VOTER_SEED").context("missing runtime voter seed")?,
    );
    let seed = zeroize::Zeroizing::new(decode_hex(&seed_hex)?);
    ensure!(seed.len() == 64, "invalid voter seed length");
    std::fs::create_dir_all(&config.artifact_dir)?;
    transport::record_responses(config.artifact_dir.join("http-evidence"))?;
    let prepared = setup::prepare(&config, &seed)?;
    let mut evidence = Vec::<Value>::new();
    delegation::submit(&config, &prepared, &seed, &mut evidence)?;
    voting::cast_and_deliver(&config, &prepared, &mut evidence)?;
    let db = &prepared.database;
    let wallet = &prepared.wallet;
    let round = &config.round_params.vote_round_id;
    ensure!(
        zcash_voting::session::resume_plan(&db, round, &[1, 2, 3])?.completed_for_display,
        "old SDK does not display completed vote"
    );
    capture(&config, &wallet.path, "accepted.sqlite")?;
    confirmation::track(&config, db, &mut evidence)?;
    capture(&config, &wallet.path, "confirmed.sqlite")?;
    std::fs::write(
        config.artifact_dir.join("evidence.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    std::fs::write(
        config.artifact_dir.join("capture.json"),
        serde_json::to_vec_pretty(&json!({
            "wallet_id":wallet.account_uuid, "round_id":round, "proposals":[1,2,3],
            "captures":[{"database":"accepted.sqlite"},{"database":"confirmed.sqlite"}],
            "round_params": config.round_params,
        }))?,
    )?;
    Ok(())
}

fn capture(config: &CaptureConfig, wallet: &std::path::Path, name: &str) -> Result<()> {
    let source = rusqlite::Connection::open(VotingDb::wallet_sidecar_path(wallet))?;
    source.execute(
        "VACUUM INTO ?1",
        [config
            .artifact_dir
            .join(name)
            .to_str()
            .context("artifact path")?],
    )?;
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    ensure!(value.len() % 2 == 0, "odd hex length");
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
