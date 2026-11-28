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

/// A definite 0x09 failure a pass settles.
pub(super) enum ProxyFailure<'a> {
    /// The chain rejected a POST.
    Rejected(ChainRejectionKind, ChainSubmissionDiagnostic),
    /// The candidate committed unsuccessfully, read from a row in the given
    /// state and classified on the codespace its lookup reported.
    Committed(&'a CommittedTransaction, ChainSubmissionState),
}

impl<T, S, C> ChainSubmissionCoordinator<T, S, C>
where
    T: ChainTransport,
    S: ChainSubmissionStore,
    C: ChainSubmissionClock,
{
    /// Settles a definite 0x09 failure.
    ///
    /// An expiry retires the generation and spent nullifiers run one tree
    /// pass. Any other refusal leaves the row `Recovering` and pending,
    /// keeping any dispatch ambiguity it records, so a later step sends it
    /// again.
    pub(super) async fn settle_proxy_failure(
        &self,
        request: &StoreAdvancementRequest,
        operation: &CapturedSubmissionOperation,
        lease: &SubmissionOperationLease,
        derived: &DerivedChainSubmission,
        failure: ProxyFailure<'_>,
        control: &dyn SubmissionControl,
    ) -> Result<ChainSubmissionResult, ChainSubmissionFailure> {
        // The committed failure's row state, or `None` for a POST answer.
        let (kind, diagnostic, durable_state) = match failure {
            ProxyFailure::Rejected(kind, diagnostic) => (kind, diagnostic, None),
            ProxyFailure::Committed(committed, durable_state) => {
                let codespace = committed.codespace.as_deref().unwrap_or_default();
                let message = format!(
                    "proxy batch transaction committed unsuccessfully with {codespace} code {}",
                    committed.code
                );
                (
                    proxy_batch_rejection_kind(codespace, committed.code),
                    ChainSubmissionDiagnostic::from_redacted_message(
                        ChainSubmissionDiagnosticKind::ChainRejected,
                        message,
                    ),
                    Some(durable_state),
                )
            }
        };
        let record = |diagnostic: &ChainSubmissionDiagnostic| match durable_state {
            None => self.classify_dispatched_post(
                derived.generation(),
                SubmissionObservation::DefiniteRejection(diagnostic.clone()),
            ),
            Some(durable_state) => self.reconcile_with_durable_state(
                derived.generation(),
                SubmissionObservation::CandidateCommittedFailure(diagnostic.clone()),
                Some(diagnostic.clone()),
                durable_state,
            ),
        };
        match kind {
            ChainRejectionKind::ProxyBatchExpired => self
                .retire_proxy(
                    request,
                    derived,
                    ProxyRetirement::Expired(diagnostic),
                    durable_state,
                )?
                .public_result(),
            ChainRejectionKind::ProxyBatchPaused | ChainRejectionKind::Other => {
                let refusal = refusal_diagnostic(kind, diagnostic);
                let recorded = record(&refusal)?;
                if durable_state.is_some() {
                    return recorded.public_result();
                }
                Ok(ChainSubmissionResult::Pending(
                    ChainSubmissionPending::Recovering {
                        candidate_transaction_hash: None,
                        diagnostic: refusal,
                    },
                ))
            }
            // Spent nullifiers under a committed candidate prove something
            // spent them first: an earlier attempt or a lifted registration.
            ChainRejectionKind::NullifierAlreadySpent => {
                let current = record(&diagnostic)?;
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
                    Some(ChainSubmissionState::Recovering),
                )?
                .public_result(),
            RecoveryScan::NoMatch(_discarded_authorization) => current.public_result(),
            RecoveryScan::Interrupted => current.public_result(),
            RecoveryScan::Lagging(record) => record.public_result(),
        }
    }

    /// Retires a 0x09 generation. A failure leaves the row unchanged, so it
    /// reports `durable_state`, or possible dispatch when that is `None`
    /// after a POST answer.
    pub(super) fn retire_proxy(
        &self,
        request: &StoreAdvancementRequest,
        derived: &DerivedChainSubmission,
        retirement: ProxyRetirement,
        durable_state: Option<ChainSubmissionState>,
    ) -> Result<StoredChainSubmission, ChainSubmissionFailure> {
        let attach_state = |error: ChainSubmissionFailure| match durable_state {
            Some(durable_state) => error.or_durable_state(durable_state),
            None => {
                ChainSubmissionFailure::with_known_possible_dispatch(error.kind(), error.message())
            }
        };
        let now = self.clock.now_seconds().map_err(attach_state)?;
        self.store
            .retire_proxy_generation(request, derived.generation(), retirement, now)
            .map_err(attach_state)
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
