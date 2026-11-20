//! Provision a migration capture on an explicitly identified disposable chain.
//! Shared Zcash testnet sources supply the snapshot; voting writes stay isolated.
use anyhow::{ensure, Context, Result};
use recovery_conformance::provisioning::{self, RoundDescription, VoteManagerKeyring};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

fn fetch(url: &str) -> Result<Value> {
    let response = Command::new("curl")
        .args(["-fsS", "--max-time", "30", url])
        .output()?;
    ensure!(response.status.success(), "isolated endpoint failed");
    Ok(serde_json::from_slice(&response.stdout)?)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: migration_provision_isolated DEPLOYMENT ARTIFACT_DIR CAPTURE_CONFIG"
    );
    let deployment: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let field = |name: &str| -> Result<&str> {
        deployment[name]
            .as_str()
            .with_context(|| format!("missing {name}"))
    };
    let chain_id = field("chain_id")?;
    ensure!(
        chain_id.starts_with("migration-v3-") && chain_id.len() <= 50,
        "not a disposable migration chain"
    );
    let rpc = field("chain_rpc")?;
    ensure!(
        fetch(&format!("{rpc}/status"))?["result"]["node_info"]["network"] == chain_id,
        "RPC chain identity mismatch"
    );
    let artifact = PathBuf::from(&args[2]);
    ensure!(
        !artifact.exists() && !PathBuf::from(&args[3]).exists(),
        "capture output already exists"
    );
    let mnemonic = std::env::var("MIGRATION_V3_VM_MNEMONIC").context("missing isolated manager")?;
    let keyring = VoteManagerKeyring::import(&mnemonic)?;
    ensure!(
        keyring.address() == field("manager_address")?,
        "isolated manager address mismatch"
    );
    let anchor = provisioning::resolve_anchor(
        field("pir_url")?,
        field("lightwalletd")?,
        zcash_voting::Network::Testnet,
    )
    .await?;
    let expiry = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 7200;
    let ballot = provisioning::suite_ballot();
    let description = RoundDescription::new(&anchor, expiry as i64, ballot.clone());
    let description_path = PathBuf::from(&args[3]).with_extension("round.json");
    std::fs::write(&description_path, description.to_json()?)?;
    // This adapter owns an isolated target. The ordinary conformance provisioner
    // deliberately permits only staging, and its guard remains intact.
    let transaction = Command::new("svoted")
        .args(["tx", "vote", "create-voting-session"])
        .arg(&description_path)
        .args([
            "--node",
            rpc,
            "--chain-id",
            chain_id,
            "--output",
            "json",
            "--gas",
            "auto",
            "--gas-adjustment",
            "1.4",
            "--fees",
            "0usvote",
            "--yes",
        ])
        .args(keyring.signing_flags())
        .output()?;
    ensure!(
        transaction.status.success(),
        "isolated round submission failed: {}",
        String::from_utf8_lossy(&transaction.stderr)
    );
    let receipt: Value = serde_json::from_slice(&transaction.stdout)?;
    std::fs::write(
        PathBuf::from(&args[3]).with_extension("submission.json"),
        &transaction.stdout,
    )?;
    ensure!(
        receipt["code"].as_u64() == Some(0),
        "isolated round rejected; receipt retained"
    );
    let hash = receipt["txhash"].as_str().context("missing tx hash")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid transaction hash"
    );
    let mut round_id = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let confirmed = fetch(&format!("{rpc}/tx?hash=0x{hash}"))?;
        if let Some(events) = confirmed["result"]["tx_result"]["events"].as_array() {
            ensure!(
                confirmed["result"]["tx_result"]["code"].as_u64() == Some(0),
                "round failed in block"
            );
            for event in events {
                if event["type"] == "create_voting_session" {
                    for attribute in event["attributes"].as_array().context("event attributes")? {
                        if attribute["key"] == "vote_round_id" {
                            round_id = attribute["value"].as_str().map(str::to_owned);
                        }
                    }
                }
            }
            if round_id.is_some() {
                break;
            }
        }
    }
    let round_id = round_id.context("round creation never confirmed; do not repeat submission")?;
    for _ in 0..60 {
        let round = provisioning::fetch_round(rpc, &round_id)?;
        if round.is_active() {
            let mut config = deployment.clone();
            config["artifact_dir"] = serde_json::json!(artifact);
            config["round_params"] = serde_json::to_value(round.params)?;
            config["historical_roster"] = serde_json::to_value(ballot)?;
            config["vote_end_time"] = serde_json::json!(expiry);
            config["scan_from"] = serde_json::json!(u32::from(
                zcash_voting::Network::Testnet
                    .activation_height(NetworkUpgrade::Nu6_3)
                    .context("activation")?
            ));
            std::fs::write(&args[3], serde_json::to_vec_pretty(&config)?)?;
            println!("Isolated round active: {round_id}");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    anyhow::bail!("isolated ceremony did not activate")
}
