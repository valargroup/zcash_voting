//! The `AdvanceProxyBatch` step. It signs and submits a proved batch through
//! the chain-submission lifecycle and re-signs expired attempts.

use std::sync::Arc;

use crate::{
    proxy::batch::{
        load_submission_state, sign_batch_attempt, AttemptSigningRequest, SubmissionState,
    },
    AdvanceProxyDelegationBatch, ChainAdvanceOutcome, ChainAdvanceRequest,
    ChainSubmissionDiagnosticKind, ChainTransport,
};

use super::{
    round_lock::HeldRoundLock, step_ledger::StepLedger, step_scope::StepScope,
    steps::persisted_policy, RoundExecutor, RoundStepDisposition, RoundStepFailure,
    RoundStepFailureKind, RoundStepOutcome, RoundStepProgress, RoundStepProgressReporter,
};

/// Re-signs per step after expired answers before reporting pending. Each
/// re-sign moves the expiry past the height just read, so a second expiry
/// means a lagging or stalled chain that a later step retries.
const MAX_RESIGNS_PER_STEP: usize = 2;

impl<T: ChainTransport> RoundExecutor<T> {
    /// Advances `bundle_index`'s proved proxy batch.
    ///
    /// A batch with nothing on the wire is signed first (see
    /// `sign_attempt`). An attempt answered expired is re-signed up to
    /// [`MAX_RESIGNS_PER_STEP`] times. Landing and a split-off registration
    /// landing (the batch is proved again) advance. Any other refusal leaves
    /// the batch pending, and a later step sends it again until it lands or
    /// voting ends.
    #[allow(
        clippy::result_large_err,
        reason = "a step fails with the executor's RoundStepFailure, as every step does"
    )]
    pub(super) async fn run_advance_proxy_batch(
        &self,
        scope: &StepScope<'_>,
        bundle_index: u32,
        lock: &HeldRoundLock,
        progress: &dyn RoundStepProgressReporter,
    ) -> Result<RoundStepOutcome, RoundStepFailure> {
        let ledger = StepLedger::default();
        let state = load_submission_state(&self.database, &scope.round_id, bundle_index)
            .map_err(|error| self.step_voting_failure(error, Some(&scope.step), &ledger))?;
        let batch_digest = match (&state.attempt, state.lifecycle_state.is_some()) {
            (Some(attempt), true) => attempt.batch_digest,
            (None, true) => {
                return Err(self.step_failure(
                    RoundStepFailureKind::InvariantViolation,
                    Some(&scope.step),
                    None,
                    &ledger,
                    "a proxy batch on the wire has no signed attempt",
                ))
            }
            (_, false) => match self
                .sign_attempt(scope, bundle_index, &state, false, lock, progress)
                .await?
            {
                Some(batch_digest) => batch_digest,
                None => return self.step_cancelled(scope, ledger),
            },
        };

        let mut resigns = 0;
        let outcome = loop {
            let outcome = self
                .chain_client
                .advance_until_terminal_in_epoch(
                    ChainAdvanceRequest::ProxyDelegationBatch(AdvanceProxyDelegationBatch {
                        vote_round_id: scope.round_id_bytes,
                        bundle_index,
                        batch_digest,
                    }),
                    &persisted_policy(scope.host),
                    scope.chain(),
                    scope.entry_epoch(),
                )
                .await
                .map_err(|failure| self.step_chain_failure(failure, Some(&scope.step), &ledger))?;
            let expired = matches!(
                &outcome,
                ChainAdvanceOutcome::Rejected(diagnostic)
                    if diagnostic.kind() == ChainSubmissionDiagnosticKind::ProxyBatchExpired
            );
            if !expired || resigns == MAX_RESIGNS_PER_STEP {
                break outcome;
            }
            resigns += 1;
            if scope.interrupted() {
                return self.step_cancelled(scope, ledger);
            }
            let state = load_submission_state(&self.database, &scope.round_id, bundle_index)
                .map_err(|error| self.step_voting_failure(error, Some(&scope.step), &ledger))?;
            if self
                .sign_attempt(scope, bundle_index, &state, true, lock, progress)
                .await?
                .is_none()
            {
                return self.step_cancelled(scope, ledger);
            }
        };

        let disposition = match &outcome {
            ChainAdvanceOutcome::Confirmed(_) => RoundStepDisposition::Advanced,
            ChainAdvanceOutcome::Rejected(diagnostic) => match diagnostic.kind() {
                // Split off. The batch is proved again against the chain.
                ChainSubmissionDiagnosticKind::ProxyRegistrationLanded => {
                    RoundStepDisposition::Advanced
                }
                // Expired again within this step. A later step re-signs.
                ChainSubmissionDiagnosticKind::ProxyBatchExpired => RoundStepDisposition::Pending,
                _ => RoundStepDisposition::ChainTerminal,
            },
            ChainAdvanceOutcome::SubmittedWithoutHash(_) => RoundStepDisposition::ChainTerminal,
            ChainAdvanceOutcome::StillPending(_) => RoundStepDisposition::Pending,
            ChainAdvanceOutcome::Cancelled => RoundStepDisposition::Cancelled,
        };
        let mut ledger = ledger;
        let result = outcome.into_result();
        progress.report(RoundStepProgress::ChainOutcome(result.clone()));
        ledger.record_chain_outcome(result);
        self.outcome(scope, disposition, ledger)
    }

    /// Signs and persists an attempt at the highest chain height any endpoint
    /// reports, under [`sign_batch_attempt`]. Refused with `VoteEnded` after
    /// the vote end. `expired` says the chain reported the stored attempt
    /// expired. Returns the batch digest, or `None` with nothing signed if the
    /// step was interrupted. A first attempt carrying the registration gets
    /// its SpendAuth signature from the delegation driver, which re-signs the
    /// bundle's locked delegation generation.
    #[allow(
        clippy::result_large_err,
        reason = "signing fails with the step's RoundStepFailure"
    )]
    async fn sign_attempt(
        &self,
        scope: &StepScope<'_>,
        bundle_index: u32,
        state: &SubmissionState,
        expired: bool,
        lock: &HeldRoundLock,
        progress: &dyn RoundStepProgressReporter,
    ) -> Result<Option<[u8; 32]>, RoundStepFailure> {
        let ledger = StepLedger::default();
        self.refuse_after_vote_end(scope, &ledger, "a new proxy batch attempt cannot be signed")?;
        let hotkey_secret = self.bound_hotkey_secret(
            scope,
            bundle_index,
            &ledger,
            "signing a proxy batch requires the round voting hotkey",
        )?;
        // The registration is signed once, for the batch's first attempt.
        let registration_signature = if state.carries_registration && state.attempt.is_none() {
            Some(
                self.resign_delegation(scope, bundle_index, lock, &ledger, "registration")
                    .await?,
            )
        } else {
            None
        };
        if scope.interrupted() {
            return Ok(None);
        }
        let chain_height = self
            .chain_client
            .latest_block_height(scope.chain())
            .await
            .map_err(|failure| self.step_chain_failure(failure, Some(&scope.step), &ledger))?;
        let db = Arc::clone(&self.database);
        let round_id = scope.round_id.clone();
        let network = scope.network;
        let chain_id = self.chain_client.vote_chain_id().to_string();
        let signing = self
            .blocking(scope, "proxy batch signing", move || {
                sign_batch_attempt(
                    &db,
                    AttemptSigningRequest {
                        round_id: &round_id,
                        bundle_index,
                        hotkey_seed: hotkey_secret.as_slice(),
                        network,
                        chain_id: &chain_id,
                        registration_signature,
                        chain_height,
                        stored_attempt_expired: expired,
                    },
                )
            })
            .await?;
        if signing.signed_now {
            progress.report(RoundStepProgress::ProxyBatchSigned { bundle_index });
        }
        Ok(Some(signing.attempt.batch_digest))
    }
}
