//! Provision a staging round for the isolated v3.0.0 capture worker.
use anyhow::{ensure, Context, Result};
use recovery_conformance::{
    environment::{Environment, STAGING_CHAIN_ID},
    provisioning, stage_config,
};
use std::{
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "usage: migration_provision ARTIFACT_DIR CONFIG_PATH"
    );
    let artifact = PathBuf::from(&args[1]);
    ensure!(!artifact.exists(), "capture artifact already exists");
    let response = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "30",
            stage_config::STAGE_DYNAMIC_CONFIG_URL,
        ])
        .output()?;
    ensure!(
        response.status.success(),
        "could not load staging configuration"
    );
    let deployment = stage_config::StageDeployment::from_json(&response.stdout)?;
    ensure!(
        deployment.supported_versions.vote_protocol == "v0"
            && deployment.supported_versions.vote_server == "v1"
            && deployment.supported_versions.pir.iter().any(|v| v == "v0"),
        "staging does not advertise v3.0.0 protocols"
    );
    let environment = Environment::from_env(deployment.clone())?;
    let endpoints = recovery_conformance::round_run::endpoints_from(&deployment);
    let response = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "20",
            &format!("{}/status", endpoints.chain_rpc),
        ])
        .output()?;
    ensure!(
        response.status.success(),
        "could not check staging identity"
    );
    let status: serde_json::Value = serde_json::from_slice(&response.stdout)?;
    ensure!(
        status["result"]["node_info"]["network"] == STAGING_CHAIN_ID,
        "RPC is not staging"
    );
    let keyring = provisioning::VoteManagerKeyring::import(environment.vote_manager_mnemonic())?;
    let expiry = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 7200;
    let round_id = provisioning::provision_active_round(
        &keyring,
        &endpoints.pir_urls[0],
        &endpoints.lightwalletd,
        zcash_voting::Network::Testnet,
        &provisioning::ChainTarget {
            rpc_url: &endpoints.chain_rpc,
            chain_id: STAGING_CHAIN_ID,
        },
        expiry as i64,
    )
    .await?;
    let round = provisioning::fetch_round(&endpoints.chain_rpc, &round_id)?;
    let activation = zcash_voting::Network::Testnet
        .activation_height(NetworkUpgrade::Nu6_3)
        .context("missing NU6.3 activation")?;
    let config = serde_json::json!({
        "artifact_dir": artifact, "round_params": round.params,
        "chain_id":STAGING_CHAIN_ID, "chain_rpc":endpoints.chain_rpc, "vote_server":endpoints.vote_servers[0],
        "pir_url":endpoints.pir_urls[0], "helper_urls":endpoints.helper_urls,
        "lightwalletd":endpoints.lightwalletd, "scan_from":u32::from(activation),
        "pir_layout":deployment.pir_layout, "vote_end_time":expiry,
        "historical_roster":provisioning::suite_ballot(),
    });
    std::fs::write(&args[2], serde_json::to_vec_pretty(&config)?)?;
    println!("Provisioned migration capture round {round_id}");
    Ok(())
}
