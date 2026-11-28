//! The `ProxyDelegate` step. It plans and proves a bundle's batch (see
//! `proxy::batch`) and stops at a stored, unsigned batch. It never signs or
//! broadcasts.

use std::sync::Arc;

use crate::{
    phases::DelegationPhase,
    proxy::batch::{plan_batch_slots, prove_batch, BatchAnchor, ProveBatchRequest},
    ChainTransport,
};

use super::{
    round_lock::HeldRoundLock, step_ledger::StepLedger, step_scope::StepScope, RoundExecutor,
    RoundStepDisposition, RoundStepFailure, RoundStepFailureKind, RoundStepOutcome,
    RoundStepProgress, RoundStepProgressReporter,
};

impl<T: ChainTransport> RoundExecutor<T> {
    /// Plans and proves `bundle_index`'s proxy batch.
    ///
    /// Refuses a new batch after the vote end with `VoteEnded`, as a new cast
    /// is. The bound hotkey must be the bundle's delegation target. An
    /// unregistered bundle proves ZKP1 through the host's delegation driver
    /// first, and a confirmed one syncs its VAN witness instead. Interruption
    /// is observed between phases, and durable effects stay.
    pub(super) async fn run_proxy_delegate(
        &self,
        scope: &StepScope<'_>,
        bundle_index: u32,
        lock: &HeldRoundLock,
        progress: &dyn RoundStepProgressReporter,
    ) -> Result<RoundStepOutcome, RoundStepFailure> {
        let ledger = StepLedger::default();
        let host = scope.host;
        self.refuse_after_vote_end(scope, &ledger, "a new proxy batch cannot be proved")?;
        let hotkey_secret = self.bound_hotkey_secret(
            scope,
            bundle_index,
            &ledger,
            "proving a proxy batch requires the round voting hotkey",
        )?;
        let network = scope.network;

        {
            let db = Arc::clone(&self.database);
            let round_id = scope.round_id.clone();
            let secret = hotkey_secret.clone();
            self.blocking(scope, "proxy batch planning", move || {
                plan_batch_slots(&db, &round_id, bundle_index, &secret, network)
            })
            .await?;
        }
        if scope.interrupted() {
            return self.step_cancelled(scope, ledger);
        }

        let phase = self
            .database
            .delegation_phase(&scope.round_id, bundle_index)
            .map_err(|error| self.step_voting_failure(error, Some(&scope.step), &ledger))?;
        let anchor = match phase {
            DelegationPhase::Prepared | DelegationPhase::PcztBuilt => {
                if self
                    .prepare_delegation_proof(scope, bundle_index, lock, progress)
                    .await?
                    .is_none()
                {
                    return self.step_cancelled(scope, ledger);
                }
                BatchAnchor::Registration
            }
            DelegationPhase::Proved => BatchAnchor::Registration,
            DelegationPhase::Confirmed => {
                let Some(witness) = self
                    .sync_van_witness(scope, bundle_index, lock, progress)
                    .await?
                else {
                    return self.step_cancelled(scope, ledger);
                };
                BatchAnchor::Chain(witness)
            }
            in_flight => {
                return Err(self.step_failure(
                    RoundStepFailureKind::InvalidInput,
                    Some(&scope.step),
                    None,
                    &ledger,
                    format!(
                        "bundle {bundle_index}'s delegation is {in_flight:?}; its proxy batch \
                         waits for the delegation to resolve"
                    ),
                ));
            }
        };
        if scope.interrupted() {
            return self.step_cancelled(scope, ledger);
        }

        let batch = {
            let db = Arc::clone(&self.database);
            let round_id = scope.round_id.clone();
            let max_proof_concurrency = host.max_proof_concurrency.max(1);
            // The prover keeps the bundle lock until the batch is persisted,
            // even if this future is dropped.
            let held_lock = Arc::clone(lock);
            let observations = scope.observations.clone();
            let operation = crate::proving_runtime::Operation::controlled(
                format!(
                    "{}:{}:{}:{}",
                    self.database.sidecar_id(),
                    scope.wallet_id,
                    scope.round_id,
                    bundle_index
                ),
                scope.chain().clone(),
                scope.entry_epoch(),
            );
            let _operation_owner = operation.owner();
            self.blocking(scope, "proxy batch proving", move || {
                operation.enter(|| {
                    let _held_lock = held_lock;
                    prove_batch(
                        &db,
                        ProveBatchRequest {
                            round_id: &round_id,
                            bundle_index,
                            hotkey_seed: hotkey_secret.as_slice(),
                            network,
                            anchor: &anchor,
                            max_proof_concurrency,
                        },
                        &observations,
                    )
                })
            })
            .await
        };
        if scope.interrupted() {
            return self.step_cancelled(scope, ledger);
        }
        batch?;
        progress.report(RoundStepProgress::ProxyBatchProved { bundle_index });
        self.outcome(scope, RoundStepDisposition::Advanced, ledger)
    }
}
