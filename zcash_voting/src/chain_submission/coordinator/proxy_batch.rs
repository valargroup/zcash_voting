//! How a pass settles a 0x09 proxy batch's definite answers. The rules live
//! in `docs/chain_submission_invariants.md`, "Proxy delegation batches"
//! (Classification).

use super::{ChainSubmissionClock, ChainSubmissionCoordinator, RecoveryScan, SubmissionControl};
use crate::chain_submission::{
    coordination::{CapturedSubmissionOperation, SubmissionOperationLease},
    generation::DerivedChainSubmission,
    protocol::{proxy_batch_rejection_kind, ChainRejectionKind, CommittedTransaction},
    recovery::RequiredReplicaHeight,
    state::SubmissionObservation,
    store::{
        ChainSubmissionStore, ProxyRetirement, StoreAdvancementRequest, StoredChainSubmission,
    },
    ChainSubmissionDiagnostic, ChainSubmissionDiagnosticKind, ChainSubmissionFailure,
    ChainSubmissionPending, ChainSubmissionResult, ChainSubmissionState, ChainTransport,
};

impl<T, S, C> ChainSubmissionCoordinator<T, S, C>
where
    T: ChainTransport,
    S: ChainSubmissionStore,
    C: ChainSubmissionClock,
{
    /// Settles a definite answer to a 0x09 POST.
    ///
    /// An expiry retires the generation and spent nullifiers run one tree
    /// pass. Any other refusal leaves the row `Recovering` and pending,
    /// keeping any dispatch ambiguity it records, so a later step sends it
    /// again.
    #[allow(
        clippy::too_many_arguments,
        reason = "the settlement keeps each captured authority explicit"
    )]
    pub(super) async fn settle_proxy_rejection(
        &self,
        request: &StoreAdvancementRequest,
        operation: &CapturedSubmissionOperation,
        lease: &SubmissionOperationLease,
        derived: &DerivedChainSubmission,
        kind: ChainRejectionKind,
        diagnostic: ChainSubmissionDiagnostic,
        control: &dyn SubmissionControl,
    ) -> Result<ChainSubmissionResult, ChainSubmissionFailure> {
        match kind {
            ChainRejectionKind::ProxyBatchExpired => self
                .retire_proxy_after_post(request, derived, ProxyRetirement::Expired(diagnostic))?
                .public_result(),
            ChainRejectionKind::ProxyBatchPaused | ChainRejectionKind::Other => {
                let refusal = refusal_diagnostic(kind, diagnostic);
                self.classify_dispatched_post(
                    derived.generation(),
                    SubmissionObservation::DefiniteRejection(refusal.clone()),
                )?;
                Ok(ChainSubmissionResult::Pending(
                    ChainSubmissionPending::Recovering {
                        candidate_transaction_hash: None,
                        diagnostic: refusal,
                    },
                ))
            }
            ChainRejectionKind::NullifierAlreadySpent => {
                let current = self.classify_dispatched_post(
                    derived.generation(),
                    SubmissionObservation::DefiniteRejection(diagnostic.clone()),
                )?;
                self.settle_proxy_nullifier_spent(
                    request, operation, lease, derived, current, control,
                )
                .await
            }
        }
    }

    /// Settles a 0x09 candidate that committed unsuccessfully, classified on
    /// the codespace its lookup reported.
    #[allow(
        clippy::too_many_arguments,
        reason = "the settlement keeps each captured authority explicit"
    )]
    pub(super) async fn settle_proxy_committed_failure(
        &self,
        request: &StoreAdvancementRequest,
        operation: &CapturedSubmissionOperation,
        lease: &SubmissionOperationLease,
        derived: &DerivedChainSubmission,
        committed: &CommittedTransaction,
        durable_state: ChainSubmissionState,
        control: &dyn SubmissionControl,
    ) -> Result<ChainSubmissionResult, ChainSubmissionFailure> {
        let codespace = committed.codespace.as_deref().unwrap_or_default();
        let diagnostic = ChainSubmissionDiagnostic::from_redacted_message(
            ChainSubmissionDiagnosticKind::ChainRejected,
            format!(
                "proxy batch transaction committed unsuccessfully with {codespace} code {}",
                committed.code
            ),
        );
        match proxy_batch_rejection_kind(codespace, committed.code) {
            ChainRejectionKind::ProxyBatchExpired => self
                .retire_proxy(
                    request,
                    derived,
                    ProxyRetirement::Expired(diagnostic),
                    durable_state,
                )?
                .public_result(),
            kind @ (ChainRejectionKind::ProxyBatchPaused | ChainRejectionKind::Other) => {
                let refusal = refusal_diagnostic(kind, diagnostic);
                self.reconcile_with_durable_state(
                    derived.generation(),
                    SubmissionObservation::CandidateCommittedFailure(refusal.clone()),
                    Some(refusal),
                    durable_state,
                )?
                .public_result()
            }
            // Spent nullifiers under a committed candidate prove something
            // spent them first: an earlier attempt or a lifted registration.
            ChainRejectionKind::NullifierAlreadySpent => {
                let current = self.reconcile_with_durable_state(
                    derived.generation(),
                    SubmissionObservation::CandidateCommittedFailure(diagnostic.clone()),
                    Some(diagnostic.clone()),
                    durable_state,
                )?;
                self.settle_proxy_nullifier_spent(
                    request, operation, lease, derived, current, control,
                )
                .await
            }
        }
    }

    /// Resolves "nullifier already spent" for a hashless `Recovering` 0x09
    /// row with one tree pass at the highest reported height.
    ///
    /// The layout confirms the batch, and the registration alone splits it
    /// off. Otherwise the row stays `Recovering` and pending, and its retry
    /// authorization is discarded, so a later step sends the batch again.
    async fn settle_proxy_nullifier_spent(
        &self,
        request: &StoreAdvancementRequest,
        operation: &CapturedSubmissionOperation,
        lease: &SubmissionOperationLease,
        derived: &DerivedChainSubmission,
        current: StoredChainSubmission,
        control: &dyn SubmissionControl,
    ) -> Result<ChainSubmissionResult, ChainSubmissionFailure> {
        match self
            .scan_tree(
                derived,
                None,
                RequiredReplicaHeight::HighestReported,
                operation,
                lease,
                control,
            )
            .await?
        {
            RecoveryScan::Match {
                final_van_position,
                vote_commitment_positions,
            } => self.confirm_tree(
                request,
                operation,
                derived,
                final_van_position,
                vote_commitment_positions,
                control,
            ),
            RecoveryScan::RegistrationLanded { position } => self
                .retire_proxy(
                    request,
                    derived,
                    ProxyRetirement::RegistrationLanded {
                        registration_position: position,
                    },
                    ChainSubmissionState::Recovering,
                )?
                .public_result(),
            RecoveryScan::NoMatch(_discarded_authorization) => current.public_result(),
            RecoveryScan::Interrupted => current.public_result(),
            RecoveryScan::Lagging(record) => record.public_result(),
        }
    }

    /// Retires a 0x09 generation. A failure reports `durable_state`, since the
    /// row is unchanged.
    pub(super) fn retire_proxy(
        &self,
        request: &StoreAdvancementRequest,
        derived: &DerivedChainSubmission,
        retirement: ProxyRetirement,
        durable_state: ChainSubmissionState,
    ) -> Result<StoredChainSubmission, ChainSubmissionFailure> {
        let attach_state = |error: ChainSubmissionFailure| error.or_durable_state(durable_state);
        let now = self.clock.now_seconds().map_err(attach_state)?;
        self.store
            .retire_proxy_generation(request, derived.generation(), retirement, now)
            .map_err(attach_state)
    }

    /// Retires after a POST answer. A failure reports possible dispatch,
    /// since the reserved row is unchanged.
    fn retire_proxy_after_post(
        &self,
        request: &StoreAdvancementRequest,
        derived: &DerivedChainSubmission,
        retirement: ProxyRetirement,
    ) -> Result<StoredChainSubmission, ChainSubmissionFailure> {
        let now = self.clock.now_seconds().map_err(|error| {
            ChainSubmissionFailure::with_known_possible_dispatch(error.kind(), error.message())
        })?;
        self.store
            .retire_proxy_generation(request, derived.generation(), retirement, now)
            .map_err(|error| {
                ChainSubmissionFailure::with_known_possible_dispatch(error.kind(), error.message())
            })
    }
}

/// The diagnostic a refused 0x09 row records: `ProxyBatchPaused` for code 51,
/// otherwise the chain's own rejection.
fn refusal_diagnostic(
    kind: ChainRejectionKind,
    diagnostic: ChainSubmissionDiagnostic,
) -> ChainSubmissionDiagnostic {
    match kind {
        ChainRejectionKind::ProxyBatchPaused => ChainSubmissionDiagnostic::from_redacted_message(
            ChainSubmissionDiagnosticKind::ProxyBatchPaused,
            diagnostic.message(),
        ),
        _ => diagnostic,
    }
}
