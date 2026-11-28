//! The ZKP4 phase of a batch. The ZKP4 key is loaded for this phase only,
//! held in a [`ProxyDelegationProver`] that is dropped when the phase ends,
//! and proofs run on the shared proving runtime.

use rusqlite::Connection;
use voting_circuits::proxy_delegation::ProxyDelegationProver;

use super::{
    batch_store,
    dc_chain::{build_dc_chain, DcChainInputs},
    slot_plan::{decode_round_id, load_planned_slots},
    BatchAnchor, ProvedProxyBatch, ProvedProxyDc,
};
use crate::{
    backend::pasta_curves::group::ff::PrimeField,
    proving_runtime,
    proxy::blob_array,
    storage::{queries, VotingDb},
    types::{Network, VotingError},
    ObservationScope,
};

/// What one ZKP4 phase proves.
pub(crate) struct ProveBatchRequest<'a> {
    /// Canonical lowercase hex.
    pub(crate) round_id: &'a str,
    pub(crate) bundle_index: u32,
    /// The round hotkey's stored secret.
    pub(crate) hotkey_seed: &'a [u8],
    /// Must be the round's stored network.
    pub(crate) network: Network,
    pub(crate) anchor: &'a BatchAnchor,
    /// Most ZKP4 proofs in flight at once.
    pub(crate) max_proof_concurrency: usize,
}

/// Reads everything `bundle_index`'s DC chain is built from. Fails with
/// [`VotingError::InvalidInput`] when the round is stored for another
/// network, setup is missing, or batch planning has not run.
pub(crate) fn load_chain_inputs(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    network: Network,
) -> Result<DcChainInputs, VotingError> {
    let stored_network = queries::load_round_network(conn, round_id, wallet_id)?;
    if stored_network != network {
        return Err(VotingError::InvalidInput {
            message: format!("round {round_id} is stored for {stored_network:?}, not {network:?}"),
        });
    }
    let van = queries::load_van_state(conn, round_id, wallet_id, bundle_index)?;
    let missing = |what: &str| VotingError::InvalidInput {
        message: format!("bundle {bundle_index} has no stored {what}"),
    };
    let stores = |what: &str| VotingError::Internal {
        message: format!("bundle {bundle_index} stores {what}"),
    };
    Ok(DcChainInputs {
        round_id: decode_round_id(round_id)?,
        network,
        van_ballots: van.remaining_ballots()?,
        address_index: van.address_index.ok_or_else(|| missing("address index"))?,
        van_comm_rand: blob_array(
            "van_comm_rand",
            van.van_comm_rand.ok_or_else(|| missing("van_comm_rand"))?,
            stores,
        )?,
        ea_pk: blob_array("ea_pk", van.ea_pk, stores)?,
        registration_van: blob_array(
            "registration VAN",
            van.registration_van
                .ok_or_else(|| missing("registration VAN"))?,
            stores,
        )?,
        slots: load_planned_slots(conn, round_id, wallet_id, bundle_index)?,
    })
}

/// Proves `request.bundle_index`'s planned batch against `request.anchor` and
/// persists it, returning it. Inputs are read in one
/// transaction and DCs proved outside it, all with one ZKP4 prover loaded
/// and prepared as an admitted CPU job and dropped, freeing its keys, before
/// the batch is persisted. The write transaction first runs
/// [`reverify_before_persisting`]. Observes cancellation and epoch changes
/// between phases.
pub(crate) fn prove_batch(
    db: &VotingDb,
    request: ProveBatchRequest<'_>,
    observations: &ObservationScope,
) -> Result<ProvedProxyBatch, VotingError> {
    let wallet_id = db.wallet_id();
    let inputs = db.read_transaction("read proxy batch inputs", |tx| {
        load_chain_inputs(
            tx,
            request.round_id,
            &wallet_id,
            request.bundle_index,
            request.network,
        )
    })?;
    if inputs.slots.is_empty() {
        return Err(VotingError::InvalidInput {
            message: format!(
                "bundle {} has no committed proxy slots to prove",
                request.bundle_index
            ),
        });
    }
    let built = build_dc_chain(&inputs, request.anchor, request.hotkey_seed)?;
    proving_runtime::check_interruption()?;
    let prover = observations.measure_result("zkp4.load_keys", || {
        proving_runtime::execute(observations, || {
            let prover =
                ProxyDelegationProver::load().map_err(|error| VotingError::ProofFailed {
                    message: format!("proxy delegation keys failed to load: {error}"),
                })?;
            prover.prepare();
            Ok(prover)
        })
    })?;
    let proofs = proving_runtime::execute_many(
        built.len(),
        request.max_proof_concurrency.max(1),
        observations,
        |index| {
            let dc = &built[index];
            observations
                .measure_result("zkp4.prove", || {
                    prover.create_proof(dc.bundle.circuit.clone(), &dc.bundle.instance)
                })
                .map_err(|error| VotingError::ProofFailed {
                    message: format!(
                        "proxy delegation proof for slot {} failed: {error}",
                        dc.slot.dc_slot
                    ),
                })
        },
    );
    drop(prover);
    let proofs = proofs?;
    proving_runtime::check_interruption()?;

    let batch = ProvedProxyBatch {
        bundle_index: request.bundle_index,
        anchor_height: request.anchor.anchor_height(),
        layout: inputs
            .slots
            .first()
            .map(|slot| slot.layout)
            .expect("checked non-empty above"),
        dcs: built
            .iter()
            .zip(proofs)
            .map(|(dc, proof)| {
                let instance = &dc.bundle.instance;
                ProvedProxyDc {
                    dc_slot: dc.slot.dc_slot,
                    delegate_index: dc.slot.delegate_index,
                    weight_ballots: dc.slot.weight_ballots,
                    van_nullifier: instance.van_nullifier.to_repr(),
                    r_vpk: dc.bundle.r_vpk_bytes,
                    vote_authority_note_new: instance.vote_authority_note_new.to_repr(),
                    delegation_commitment: instance.delegation_commitment.to_repr(),
                    dc_slot_nullifier: instance.dc_slot_nullifier.to_repr(),
                    alpha_v: dc.slot.alpha_v,
                    proof,
                }
            })
            .collect(),
    };
    db.write_transaction("persist proved proxy batch", |tx| {
        reverify_before_persisting(tx, &wallet_id, &request, &inputs)?;
        batch_store::insert_proved_batch(
            tx,
            request.round_id,
            &wallet_id,
            &batch,
            crate::proxy::unix_now(),
        )
    })?;
    Ok(batch)
}

/// Re-verifies, on the persisting transaction's snapshot, that:
///
/// - no batch is proved already;
/// - the inputs are unchanged, so the proofs still describe the slots;
/// - nothing has spent the VAN: no vote reached the chain and no vote or proxy
///   lifecycle row other than a definite rejection exists;
/// - a batch carrying the registration has a proved, never-submitted ZKP1,
///   and a chain-anchored batch spends the bundle's confirmed VAN position.
fn reverify_before_persisting(
    tx: &Connection,
    wallet_id: &str,
    request: &ProveBatchRequest<'_>,
    captured: &DcChainInputs,
) -> Result<(), VotingError> {
    let (round_id, bundle_index) = (request.round_id, request.bundle_index);
    if batch_store::has_proved_batch(tx, round_id, wallet_id, bundle_index)? {
        return Err(VotingError::InvalidInput {
            message: format!("bundle {bundle_index}'s proxy batch is already proved"),
        });
    }
    if load_chain_inputs(tx, round_id, wallet_id, bundle_index, request.network)? != *captured {
        return Err(VotingError::InvalidInput {
            message: format!(
                "bundle {bundle_index}'s delegation setup or proxy slots changed while its batch \
                 was proving"
            ),
        });
    }
    let van = queries::load_van_state(tx, round_id, wallet_id, bundle_index)?;
    if van.registration_van_spent
        || van.proposal_authority != voting_circuits::MAX_PROPOSAL_AUTHORITY
    {
        return Err(VotingError::InvalidInput {
            message: format!(
                "a vote may already have spent bundle {bundle_index}'s VAN, so its proxy batch \
                 cannot spend it"
            ),
        });
    }
    let registration = batch_store::registration_evidence(tx, round_id, wallet_id, bundle_index)?;
    match request.anchor {
        BatchAnchor::Registration if registration.may_be_on_chain => {
            Err(VotingError::InvalidInput {
                message: format!(
                    "bundle {bundle_index}'s registration may already be on chain, so its proxy \
                     batch cannot carry it"
                ),
            })
        }
        BatchAnchor::Registration if !registration.proved => Err(VotingError::InvalidInput {
            message: format!(
                "bundle {bundle_index}'s proxy batch carries a registration with no stored ZKP1 \
                 proof"
            ),
        }),
        BatchAnchor::Registration => Ok(()),
        BatchAnchor::Chain(witness) if van.van_position != Some(u64::from(witness.position)) => {
            Err(VotingError::InvalidInput {
                message: format!(
                    "bundle {bundle_index}'s confirmed VAN is not at the witnessed position {}",
                    witness.position
                ),
            })
        }
        BatchAnchor::Chain(_) => Ok(()),
    }
}
