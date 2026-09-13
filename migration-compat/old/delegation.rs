//! Prove and submit old-release delegations; only real chain events confirm them.
use super::{setup::PreparedCapture, transport, verification, CaptureConfig};
use anyhow::{ensure, Context, Result};
use pasta_curves::{group::ff::PrimeField, pallas};
use serde_json::{json, Value};
use zcash_voting::{delegate, round::VotingDb, Network, NoopProgressReporter};

pub(super) fn submit(
    config: &CaptureConfig,
    prepared: &PreparedCapture,
    seed: &[u8],
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let db = &prepared.database;
    let wallet_db = &prepared.wallet_database;
    let wallet = &prepared.wallet;
    let round = &config.round_params.vote_round_id;
    for bundle in &prepared.bundles {
        eprintln!("capture: proving delegation bundle {}", bundle.bundle_index);
        let pir = zcash_voting::connect_pir_blocking(
            config.pir_layout,
            &config.pir_url,
            std::sync::Arc::new(zcash_voting::HyperTransport::new()),
        )?;
        bundle.precompute(&db, &wallet_db, &pir)?;
        let setup = bundle.setup(&db, &NoopProgressReporter)?;
        let request = bundle.signing_request(&db)?;
        let account = zip32::AccountId::try_from(request.account_index)
            .map_err(|_| anyhow::anyhow!("account index"))?;
        let keys =
            zcash_keys::keys::UnifiedSpendingKey::from_seed(&Network::Testnet, &seed, account)
                .map_err(|_| anyhow::anyhow!("derive account keys"))?;
        ensure!(
            zip32::fingerprint::SeedFingerprint::from_seed(&seed)
                .context("seed fingerprint")?
                .to_bytes()
                == request.seed_fingerprint,
            "wallet and seed differ"
        );
        let alpha = Option::<pallas::Scalar>::from(pallas::Scalar::from_repr(request.alpha))
            .context("invalid alpha")?;
        let signature = orchard::keys::SpendAuthorizingKey::from(keys.orchard())
            .randomize(&alpha)
            .sign(rand::rngs::OsRng, &request.sighash);
        bundle.prove(&db, &pir, &NoopProgressReporter)?;
        verification::verify(&VotingDb::wallet_sidecar_path(&wallet.path))?;
        let signed = bundle.signed_bundle(
            &db,
            setup.pczt_bytes,
            delegate::PreparedSigner::signature((&signature).into(), request.sighash),
        )?;
        let hash = transport::submit(
            &config.vote_server,
            "delegate-vote",
            &signed.submission.to_wire_json()?,
        )?;
        let (receipt, events) = transport::confirm(&config.chain_rpc, &hash)?;
        zcash_voting::confirmation::confirm_delegation_submission(
            &db,
            round,
            bundle.bundle_index,
            &hash,
            &events,
        )?;
        evidence.push(
            json!({"kind":"delegation", "bundle_index":bundle.bundle_index, "receipt":receipt}),
        );
    }
    Ok(())
}
