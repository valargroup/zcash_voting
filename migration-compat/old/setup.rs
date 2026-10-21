//! Prepare the old wallet and all bundles before proving advances round phase.
use super::backend::{wallet_rng, zcash_client_sqlite};
use super::{wallet_sync, CaptureConfig};
use anyhow::{ensure, Result};
use zcash_voting::{delegate, round::VotingDb, session::Decision, Network, VotingHotkey};

pub(super) struct PreparedCapture {
    pub database: VotingDb,
    pub wallet: wallet_sync::SyncedWallet,
    pub wallet_database: zcash_client_sqlite::WalletDb<
        rusqlite::Connection,
        Network,
        zcash_client_sqlite::util::SystemClock,
        wallet_rng::rngs::OsRng,
    >,
    pub hotkey: VotingHotkey,
    pub bundles: Vec<delegate::PreparedDelegationBundle>,
}

pub(super) fn prepare(config: &CaptureConfig, seed: &[u8]) -> Result<PreparedCapture> {
    let runtime = tokio::runtime::Runtime::new()?;
    let wallet = runtime.block_on(wallet_sync::sync_wallet(
        &config.artifact_dir.join("wallet.sqlite"),
        &seed,
        &config.lightwalletd,
        Network::Testnet,
        config.scan_from,
        config.round_params.snapshot_height,
    ))?;
    ensure!(
        wallet.scanned_to == config.round_params.snapshot_height,
        "wallet did not reach snapshot"
    );
    let db = VotingDb::open_wallet_sidecar(&wallet.path, &wallet.account_uuid)?;
    let hotkey = zcash_voting::hotkey::generate_random_voting_hotkey(Network::Testnet)?;
    let round = &config.round_params.vote_round_id;
    let lwd = runtime.block_on(delegate::gather_delegation_lwd_inputs(
        delegate::ResolveDelegationLwdParams {
            lightwalletd_url: &config.lightwalletd,
            network: Network::Testnet,
            round_params: config.round_params.clone(),
            round_name: "migration-compatibility",
        },
    ))?;
    let wallet_db = zcash_client_sqlite::WalletDb::for_path(
        &wallet.path,
        Network::Testnet,
        zcash_client_sqlite::util::SystemClock,
        wallet_rng::rngs::OsRng,
    )?;
    // Preparation is completed for every bundle before proving advances phase.
    let first = delegate::prepare_delegation_bundle(
        &db,
        &wallet_db,
        delegate::PrepareDelegationBundleParams {
            lwd: lwd.clone(),
            session_json: None,
            account_uuid: &wallet.account_uuid,
            voting_hotkey: &hotkey,
            bundle_index: 0,
            bundle_policy: zcash_voting::BundlePolicy::default(),
        },
    )?;
    let count = db.get_bundle_count(round)?;
    ensure!(count >= 2, "realism gate requires multiple bundles");
    let mut bundles = vec![first];
    for bundle_index in 1..count {
        bundles.push(delegate::prepare_delegation_bundle(
            &db,
            &wallet_db,
            delegate::PrepareDelegationBundleParams {
                lwd: lwd.clone(),
                session_json: None,
                account_uuid: &wallet.account_uuid,
                voting_hotkey: &hotkey,
                bundle_index,
                bundle_policy: zcash_voting::BundlePolicy::default(),
            },
        )?);
    }
    db.set_ballot_intent(round, 1, Decision::Choice(0), 2)?;
    db.set_ballot_intent(round, 2, Decision::Choice(1), 3)?;
    db.set_ballot_intent(round, 3, Decision::Skipped, 4)?;
    Ok(PreparedCapture {
        database: db,
        wallet,
        wallet_database: wallet_db,
        hotkey,
        bundles,
    })
}
