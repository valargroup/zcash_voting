//! Cache initialization is single-flighted outside proof admission.

use crate::{ObservationScope, VotingError};
use std::sync::OnceLock;
use voting_crypto_deps::halo2_proofs::{pasta::EqAffine, poly::commitment::Params};

#[derive(Clone, Copy)]
pub(crate) enum CacheKind {
    Delegation,
    Vote,
}
static DELEGATION: OnceLock<Result<(), String>> = OnceLock::new();
static VOTE: OnceLock<Result<(), String>> = OnceLock::new();

/// Prepares the retained commitment tables used by Zakura's Halo2 prover.
///
/// A `false` result keeps the prover's standard unprepared path, so no error
/// needs to be surfaced to callers.
#[cfg(feature = "zakura")]
fn prepare_proving_params(params: &Params<EqAffine>) {
    let _ = params.prepare_commitments();
}

/// LRZ has no equivalent proving preparation.
#[cfg(feature = "lrz")]
fn prepare_proving_params(_: &Params<EqAffine>) {}

/// Initializes one circuit key cache before releasing waiting proof callers.
fn initialize_cache(kind: CacheKind) -> Result<(), VotingError> {
    let params = match kind {
        CacheKind::Delegation => {
            let (params, _, _) = voting_circuits::delegation::delegation_cached_keys()
                .map_err(|error| super::internal(error.to_string()))?;
            params
        }
        CacheKind::Vote => {
            let (params, _, _) = voting_circuits::vote_proof::vote_proof_cached_keys()
                .map_err(|error| super::internal(error.to_string()))?;
            params
        }
    };
    prepare_proving_params(params);
    Ok(())
}

/// Waiters never occupy pool workers or heavy-job permits while keys are cold.
pub(crate) fn ensure_cache(
    kind: CacheKind,
    observations: &ObservationScope,
) -> Result<(), VotingError> {
    let runtime = super::runtime().map_err(super::internal)?;
    if runtime.pool.current_thread_index().is_some() {
        return Err(super::internal(
            "cache initialization requested on a CPU worker",
        ));
    }
    let cache = match kind {
        CacheKind::Delegation => &DELEGATION,
        CacheKind::Vote => &VOTE,
    };
    observations.measure_result("proving::cache_ready", || {
        cache
            .get_or_init(|| {
                // Shared cache work outlives any one requesting operation.
                let cache_operation = super::Operation::controlled(
                    format!(
                        "cache:{}",
                        match kind {
                            CacheKind::Delegation => "delegation",
                            CacheKind::Vote => "vote",
                        }
                    ),
                    crate::ChainSubmissionControl::new(0),
                    0,
                );
                cache_operation
                    .enter(|| super::execute(observations, || initialize_cache(kind)))
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map(|_| ())
            .map_err(|message| VotingError::ProofFailed {
                message: message.clone(),
            })
    })
}
